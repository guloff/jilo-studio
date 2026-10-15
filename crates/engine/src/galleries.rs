//! A look on the whole edit at once (the Library's transitions, effects and filters):
//! a transition on every cut, an effect or a Lumetri look on every picture clip. One undo step.
//!
//! | Command | |
//! |---|---|
//! | `sequence.transitionAllCuts` | `{effect?, frames?, track?: id\|"V1"\|"all"}`: centred on each cut between two clips that has none (default: the default video transition, the lowest video track with clips) |
//! | `effects.applyToAll` | `{effect}` or `{preset}` (a Lumetri look): every picture clip on unlocked video tracks (not graphics) |

use filmcraft_project::{ClipId, ItemKind, Transition, TransitionId};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, has_seq, str_p};
use crate::{EngineError, Result, Session};

fn all_cuts(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sequence.transitionAllCuts";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let eff = str_p(p, "effect").map(str::to_string).unwrap_or_else(|| s.state.default_video_transition.clone());
    let def = filmcraft_project::vtransition::find_transition(&eff, filmcraft_project::EffectKind::VideoTransition)
        .ok_or_else(|| bad(cmd, format!("unknown transition `{eff}`")))?;
    let rate = q.settings.frame_rate;
    let dur = p.get("frames").and_then(Value::as_i64).map_or_else(|| s.prefs.timeline.video_transition_duration(rate), |f| rate.tick_of(f.max(2)));
    let tracks: Vec<usize> = match p.get("track") {
        Some(Value::String(a)) if a == "all" => (0..q.video_tracks.len()).collect(),
        Some(v) => {
            let id = v.as_u64().or_else(|| v.as_str().and_then(|x| x.strip_prefix('V')).and_then(|n| n.parse::<u64>().ok()).map(|n| n.saturating_sub(1)));
            let i = match v {
                Value::Number(_) => q.video_tracks.iter().position(|t| Some(t.id.0) == id),
                _ => id.map(|n| n as usize).filter(|n| *n < q.video_tracks.len()),
            };
            vec![i.ok_or_else(|| bad(cmd, "`track`: a video track id, \"V1\"…, or \"all\""))?]
        }
        None => q.video_tracks.iter().position(|t| !t.items.is_empty()).into_iter().collect(),
    };
    let mut plan: Vec<(filmcraft_project::TrackId, Transition)> = Vec::new();
    for ti in tracks {
        let tr = &q.video_tracks[ti];
        if tr.locked {
            continue;
        }
        for w in tr.items.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if a.end() != b.start || tr.transitions.iter().any(|t| t.range().contains(a.end()) || t.start == a.end()) {
                continue;
            }
            // as long as the shorter clip allows, at least two frames
            let d = dur.min(a.duration.min(b.duration));
            if d < rate.tick_of(2) {
                continue;
            }
            let start = rate.snap(a.end() - d.mul_ratio(1, 2));
            plan.push((
                tr.id,
                Transition {
                    id: TransitionId(0),
                    effect: def.instance(),
                    start,
                    duration: d,
                    from: Some(a.id),
                    to: Some(b.id),
                    align: Default::default(),
                    reverse: false,
                },
            ));
        }
    }
    if plan.is_empty() {
        return Err(bad(cmd, "no cut without a transition on the track"));
    }
    let n = plan.len();
    s.edit_sequence(&format!("Apply {} to All Cuts", def.name), |q, ctx, _| {
        for (track, t) in plan {
            filmcraft_edit::add_transition(q, track, t, ctx)?;
        }
        Ok(())
    })?;
    Ok(json!({"transitions": n, "effect": def.id}))
}

/// The picture clips on unlocked video tracks: media with video (not graphics, not adjustment layers).
fn picture_clips(s: &Session) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    q.video_tracks
        .iter()
        .filter(|t| !t.locked)
        .flat_map(|t| t.items.iter())
        .filter(|it| s.project.item(it.item).is_some_and(|i| matches!(i.kind, ItemKind::Media(_)) && i.has_video()))
        .map(|it| it.id)
        .collect()
}

fn to_all(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "effects.applyToAll";
    let clips: Vec<u64> = picture_clips(s).iter().map(|c| c.0).collect();
    if clips.is_empty() {
        return Err(bad(cmd, "the sequence has no picture clips"));
    }
    let n = clips.len();
    match (str_p(p, "effect"), str_p(p, "preset")) {
        (Some(e), _) => s.execute("effects.apply", json!({"clips": clips, "effect": e}))?,
        (None, Some(name)) => s.execute("lumetri.applyPreset", json!({"clips": clips, "name": name}))?,
        _ => return Err(bad(cmd, "`effect` or `preset` (a Lumetri look)")),
    };
    Ok(json!({"clips": n}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "sequence.transitionAllCuts",
            label: "Apply Transition to All Cuts",
            menu: &[],
            shortcut: None,
            params: r#"{"effect":str?,"frames":n?,"track":id|"V1"|"all"?}"#,
            enabled: has_seq,
            run: all_cuts,
            journal: true,
        },
        CommandSpec {
            id: "effects.applyToAll",
            label: "Apply to All Clips",
            menu: &[],
            shortcut: None,
            params: r#"{"effect":str?,"preset":str? (a Lumetri look)}"#,
            enabled: has_seq,
            run: to_all,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Session {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s
    }

    #[test]
    fn a_transition_goes_on_every_cut_and_a_look_on_every_clip() {
        let mut s = demo();
        let q = s.active_sequence().unwrap().clone();
        let v1 = &q.video_tracks[0];
        let cuts = v1.items.windows(2).filter(|w| w[0].end() == w[1].start).count();
        let had = v1.transitions.len();
        let undo = s.history.undo.len();
        let r = s.execute("sequence.transitionAllCuts", json!({"effect": "cross_dissolve"})).unwrap();
        assert!(r["transitions"].as_u64().unwrap() >= 1, "{r}");
        assert_eq!(s.history.undo.len(), undo + 1);
        let t = &s.active_sequence().unwrap().video_tracks[0];
        assert!(t.transitions.len() > had && t.transitions.len() <= had + cuts, "{} → {}", had, t.transitions.len());
        // every cut has one now: a second time finds none
        assert!(s.execute("sequence.transitionAllCuts", json!({"effect": "cross_dissolve"})).is_err());
        let r = s.execute("effects.applyToAll", json!({"effect": "gaussian_blur"})).unwrap();
        let n = r["clips"].as_u64().unwrap() as usize;
        let q = s.active_sequence().unwrap();
        let blurred = q.video_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.effects.iter().any(|e| e.effect == "gaussian_blur")).count();
        assert_eq!(blurred, n);
        let look = filmcraft_render::lumetri_presets::presets()[0].name;
        s.execute("effects.applyToAll", json!({"preset": look})).unwrap();
        assert!(s.execute("effects.applyToAll", json!({})).is_err());
    }
}
