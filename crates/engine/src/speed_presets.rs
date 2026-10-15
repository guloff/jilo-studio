//! Speed in one click (the quick actions' Speed menu, as phone editors have it): the selected clip
//! at ×0.25 to ×4 (later clips follow), a slow-motion moment around the playhead, or a freeze
//! frame there. One undo step each.
//!
//! | Command | |
//! |---|---|
//! | `clip.speedPreset` | `{preset: "0.25"…"4" \| "slowMoment" \| "freeze", clip?, seconds?}` |

use filmcraft_project::ClipId;
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// The presets: (id, name).
pub const PRESETS: [(&str, &str); 8] = [
    ("0.25", "×0.25 Slow Motion"),
    ("0.5", "×0.5"),
    ("1", "×1 Normal"),
    ("1.5", "×1.5"),
    ("2", "×2"),
    ("4", "×4 Fast"),
    ("slowMoment", "Slow-Motion Moment"),
    ("freeze", "Freeze Frame"),
];

fn target(s: &Session, p: &Value) -> Result<ClipId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let ok = |c: ClipId| q.find_item(c).is_some();
    match u64_p(p, "clip").map(ClipId) {
        Some(c) if ok(c) => Ok(c),
        Some(_) => Err(bad("clip.speedPreset", "no such clip")),
        None => s.state.selection.first().copied().filter(|c| ok(*c)).ok_or_else(|| bad("clip.speedPreset", "select a clip")),
    }
}

fn preset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.speedPreset";
    let id = str_p(p, "preset").ok_or_else(|| bad(cmd, "`preset`: 0.25, 0.5, 1, 1.5, 2, 4, slowMoment or freeze"))?.to_string();
    let clip = target(s, p)?;
    match id.as_str() {
        "freeze" => {
            let secs = f64_p(p, "seconds").unwrap_or(2.0).clamp(0.2, 30.0);
            // at the playhead when it is on the clip, else in the clip's middle
            let it = s.active_sequence().and_then(|q| q.find_item(clip)).map(|x| x.1.clone()).ok_or_else(|| bad(cmd, "no such clip"))?;
            let t = s.playhead();
            let at = if it.range().contains(t) { t } else { s.sequence_rate().snap(it.start + Tick(it.duration.0 / 2)) };
            s.state.selection = vec![clip];
            s.execute("clip.insertFrameHoldSegment", json!({"clip": clip.0, "time": at.0, "seconds": secs}))
        }
        "slowMoment" => slow_moment(s, clip, f64_p(p, "seconds").unwrap_or(1.0).clamp(0.2, 10.0)),
        x => {
            let k: f64 =
                x.parse().ok().filter(|k: &f64| [0.25, 0.5, 1.0, 1.5, 2.0, 4.0].contains(k)).ok_or_else(|| bad(cmd, format!("unknown preset `{x}`")))?;
            s.execute("clip.speedDuration", json!({"clips": [clip.0], "speed": k * 100.0, "ripple": true}))
        }
    }
}

/// `seconds` of the clip around the playhead at 40 %: cut there, slowed, later clips follow.
fn slow_moment(s: &mut Session, clip: ClipId, seconds: f64) -> Result<Value> {
    let cmd = "clip.speedPreset";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let (_, it) = q.find_item(clip).ok_or_else(|| bad(cmd, "no such clip"))?;
    let t = s.playhead();
    if !it.range().contains(t) {
        return Err(bad(cmd, "put the playhead on the clip, at the moment to slow down"));
    }
    let half = rate.snap_nearest(Tick::from_seconds_f64(seconds / 2.0));
    let (a, b) = ((t - half).max(it.start + rate.frame_duration()), (t + half).min(it.end() - rate.frame_duration()));
    if b <= a {
        return Err(bad(cmd, "the clip is too short for a slow-motion moment"));
    }
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let selection = s.state.selection.clone();
    let r = (|| -> Result<Value> {
        let track = s.active_sequence().and_then(|q| q.find_item(clip)).map(|x| x.0).ok_or_else(|| bad(cmd, "no such clip"))?;
        let piece_at = |s: &Session, at: Tick| {
            s.active_sequence().and_then(|q| q.track(track)).and_then(|t| t.items.iter().find(|i| i.range().contains(at)).map(|i| i.id))
        };
        s.execute("timeline.razor", json!({"clip": clip.0, "time": a.0}))?;
        let middle = piece_at(s, a).ok_or_else(|| EngineError::Other("the cut was not made".into()))?;
        s.execute("timeline.razor", json!({"clip": middle.0, "time": b.0}))?;
        let mid = piece_at(s, a).ok_or_else(|| EngineError::Other("the cut was not made".into()))?;
        s.execute("clip.speedDuration", json!({"clips": [mid.0], "speed": 40.0, "ripple": true}))?;
        Ok(json!({"clip": mid.0}))
    })();
    match r {
        Ok(v) => {
            crate::recipes::collapse_undo(s, before, "Slow-Motion Moment");
            Ok(v)
        }
        Err(e) => {
            s.revert_to(mark);
            s.state.selection = selection;
            Err(e)
        }
    }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "clip.speedPreset",
        label: "Speed Preset",
        menu: &[],
        shortcut: None,
        params: r#"{"preset":"0.25|0.5|1|1.5|2|4|slowMoment|freeze","clip":id?,"seconds":f64? (freeze: its length, 2; slowMoment: the slowed span, 1)}"#,
        enabled: has_seq,
        run: preset,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_a_slow_moment_and_a_freeze() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let q = s.active_sequence().unwrap().clone();
        let it = q.video_tracks[0].items[1].clone();
        s.state.selection = vec![it.id];
        let total = q.duration();
        s.execute("clip.speedPreset", json!({"preset": "2"})).unwrap();
        let after = s.active_sequence().unwrap().find_item(it.id).unwrap().1.clone();
        assert!((after.speed - 2.0).abs() < 1e-9 && after.duration < it.duration, "twice as fast and shorter");
        s.execute("edit.undo", json!({})).unwrap();
        // a slow moment at the clip's middle: three pieces, the middle at 40 %, the edit longer
        s.set_playhead(it.start + Tick((it.duration.0) / 2));
        let undo = s.history.undo.len();
        let r = s.execute("clip.speedPreset", json!({"preset": "slowMoment"})).unwrap();
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
        let mid = ClipId(r["clip"].as_u64().unwrap());
        let m = s.active_sequence().unwrap().find_item(mid).unwrap().1.clone();
        assert!((m.speed - 0.4).abs() < 1e-9, "{}", m.speed);
        assert!(s.active_sequence().unwrap().video_tracks[0].items.len() >= q.video_tracks[0].items.len() + 2);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.active_sequence().unwrap().duration(), total);
        assert!(s.execute("clip.speedPreset", json!({"preset": "3"})).is_err());
        // a freeze frame at the playhead
        s.state.selection = vec![it.id];
        s.execute("clip.speedPreset", json!({"preset": "freeze"})).unwrap();
        assert!(s.active_sequence().unwrap().video_tracks[0].items.iter().any(|i| i.frame_hold.is_some()), "a held frame");
    }
}
