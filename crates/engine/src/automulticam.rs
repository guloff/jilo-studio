//! Automatic multi-camera editing (`multicam.autoSwitch`): the angle of a multi-camera clip
//! follows whoever is speaking — by the speakers of the transcript (each speaker's camera), or by
//! loudness (the camera whose own microphone is loudest). A shot is at least `minShot` seconds;
//! with a `wide` camera, crosstalk and long silences go to it. The cut comes a moment before the
//! speech, as an editor cuts. One undo step; the clips are cut and their angles set as with the
//! Multi-Camera view, so every cut can be rolled or switched by hand afterwards.

use serde_json::{Value, json};

use filmcraft_edit as edit;
use filmcraft_project::{ClipId, ItemId, Sequence, TrackItem};
use filmcraft_time::Tick;

use crate::commands::{CommandSpec, bad, has_seq, str_p};
use crate::{EngineError, Result, Session};

/// Length of one analysis window, in seconds.
pub const WINDOW: f64 = 0.25;

/// Longest clip analysed by loudness, in seconds (three hours).
const MAX_SECONDS: f64 = 3.0 * 3600.0;

/// Which angle each stretch of windows shows: `(first window, angle)` in time order, from the
/// activity of each angle in each window (speech of its speaker, or its microphone's loudness).
/// A run shorter than `min_windows` joins its neighbour; with `wide`, windows where two angles are
/// about as active, and silences of `wide_after` windows or more, show the wide angle.
pub fn plan(activity: &[Vec<f64>], min_windows: usize, wide: Option<usize>, wide_after: usize) -> Vec<(usize, usize)> {
    let n = activity.iter().map(Vec::len).max().unwrap_or(0);
    if n == 0 || activity.is_empty() {
        return Vec::new();
    }
    // silence: below a fraction of the typical level of the active windows
    let mut peaks: Vec<f64> = (0..n).map(|w| activity.iter().map(|a| a.get(w).copied().unwrap_or(0.0)).fold(0.0, f64::max)).filter(|v| *v > 0.0).collect();
    peaks.sort_by(f64::total_cmp);
    let typical = peaks.get(peaks.len() * 9 / 10).copied().unwrap_or(0.0);
    let floor = typical * 0.15;
    let mut labels: Vec<Option<usize>> = (0..n)
        .map(|w| {
            let mut v: Vec<(usize, f64)> = activity.iter().enumerate().map(|(a, x)| (a, x.get(w).copied().unwrap_or(0.0))).collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            let (top, level) = v[0];
            if level <= floor || level <= 0.0 {
                return None;
            }
            let second = v.get(1).map_or(0.0, |x| x.1);
            match wide {
                Some(wa) if second > floor && second >= level * 0.7 => Some(wa),
                _ => Some(top),
            }
        })
        .collect();
    // long silences go wide; short ones keep the shot
    if let Some(wa) = wide {
        let mut w = 0;
        while w < n {
            if labels[w].is_none() {
                let end = (w..n).find(|&k| labels[k].is_some()).unwrap_or(n);
                if end - w >= wide_after.max(1) {
                    labels[w..end].iter_mut().for_each(|l| *l = Some(wa));
                }
                w = end;
            } else {
                w += 1;
            }
        }
    }
    let first = labels.iter().flatten().next().copied().or(wide).unwrap_or(0);
    let mut cur = first;
    let filled: Vec<usize> = labels
        .iter()
        .map(|l| {
            if let Some(a) = l {
                cur = *a;
            }
            cur
        })
        .collect();
    // runs, then the short ones join a neighbour (the longer one) until none is left
    let mut runs: Vec<(usize, usize, usize)> = Vec::new(); // (start, len, angle)
    for (w, a) in filled.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if r.2 == *a => r.1 += 1,
            _ => runs.push((w, 1, *a)),
        }
    }
    let min = min_windows.max(1);
    while runs.len() > 1 {
        let Some((i, _)) = runs.iter().enumerate().filter(|(_, r)| r.1 < min).min_by_key(|(_, r)| r.1) else { break };
        let into_prev = match (i.checked_sub(1).map(|p| runs[p].1), runs.get(i + 1).map(|r| r.1)) {
            (Some(p), Some(nx)) => p >= nx,
            (Some(_), None) => true,
            _ => false,
        };
        let r = runs.remove(i);
        if into_prev {
            runs[i - 1].1 += r.1;
        } else {
            runs[i].0 = r.0;
            runs[i].1 += r.1;
        }
        // neighbours that now show the same angle become one run
        let mut k = 1;
        while k < runs.len() {
            if runs[k].2 == runs[k - 1].2 {
                let r = runs.remove(k);
                runs[k - 1].1 += r.1;
            } else {
                k += 1;
            }
        }
    }
    runs.into_iter().map(|r| (r.0, r.2)).collect()
}

/// The multi-camera clips to switch: those selected, else every one in the open sequence.
fn targets(s: &Session) -> Vec<TrackItem> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let all: Vec<&TrackItem> =
        q.video_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.multicam.is_some_and(|m| m.enabled) && s.project.sequence(i.item).is_some()).collect();
    let chosen: Vec<&TrackItem> = all.iter().copied().filter(|i| s.state.selection.contains(&i.id)).collect();
    if chosen.is_empty() { all } else { chosen }.into_iter().cloned().collect()
}

/// The speakers' words in the source sequence, as (speaker, start, end) in source seconds.
fn speaker_words(s: &Session, src: &Sequence) -> Vec<(u32, f64, f64)> {
    let mut out = Vec::new();
    for it in src.audio_tracks.iter().flat_map(|t| t.items.iter()) {
        let Some(tr) = s.project.transcripts.get(&it.item) else { continue };
        for w in &tr.words {
            let Some(sp) = w.speaker else { continue };
            if w.end <= it.source_in || w.start >= it.source_in + it.duration {
                continue;
            }
            let a = (it.start + (w.start.max(it.source_in) - it.source_in)).seconds();
            let b = (it.start + (w.end.min(it.source_in + it.duration) - it.source_in)).seconds();
            if !out.contains(&(sp, a, b)) {
                out.push((sp, a, b));
            }
        }
    }
    out
}

/// Activity of each video angle in each window of `[from, from + n·WINDOW)` (source seconds).
fn activity(s: &Session, source: ItemId, from: f64, n: usize, method: &str, speaker_cams: &[usize]) -> Result<Vec<Vec<f64>>> {
    let src = s.project.sequence(source).ok_or_else(|| EngineError::Other("not a multi-camera clip".into()))?;
    let cams = src.cameras();
    let angles: Vec<usize> = cams.shown_angles();
    let mut act = vec![vec![0.0; n]; cams.cameras.len()];
    match method {
        "speakers" => {
            for (sp, a, b) in speaker_words(s, src) {
                let Some(&angle) = speaker_cams.get(sp as usize).or_else(|| angles.get(sp as usize % angles.len().max(1))) else { continue };
                let (w0, w1) = (((a - from) / WINDOW).floor().max(0.0) as usize, ((b - from) / WINDOW).ceil().max(0.0) as usize);
                for w in w0..w1.min(n) {
                    act[angle][w] = 1.0;
                }
            }
        }
        _ => {
            // each camera's own microphones, mixed alone at a low rate
            let project = s.project.clone();
            let provider = s.media.full_res_provider(project.clone(), s.services.clone());
            let rate = 8000u32;
            let per = (WINDOW * f64::from(rate)) as usize;
            for &angle in &angles {
                let mine = &cams.cameras[angle].audio_tracks;
                if mine.is_empty() {
                    continue;
                }
                let mut solo = src.clone();
                solo.settings.sample_rate = rate;
                for t in &mut solo.audio_tracks {
                    t.muted = !mine.contains(&t.id);
                    t.solo = false;
                }
                // in chunks of a minute
                let chunk = 240usize;
                let mut w = 0;
                while w < n {
                    let k = chunk.min(n - w);
                    let start = ((from + w as f64 * WINDOW) * f64::from(rate)) as i64;
                    let buf = filmcraft_render::audio::mix_sequence(&project, &solo, start, k * per, &provider);
                    let mono = filmcraft_speech::downmix(&buf.channels);
                    for j in 0..k {
                        let part = mono.get(j * per..((j + 1) * per).min(mono.len())).unwrap_or(&[]);
                        let sum: f64 = part.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
                        act[angle][w + j] = (sum / part.len().max(1) as f64).sqrt();
                    }
                    w += k;
                }
            }
        }
    }
    Ok(act)
}

fn auto_switch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "multicam.autoSwitch";
    let clips = targets(s);
    if clips.is_empty() {
        return Err(bad(cmd, "no multi-camera clip in the sequence (Clip ▸ Create Multi-Camera Source Sequence…)"));
    }
    let min_shot = p.get("minShot").and_then(Value::as_f64).unwrap_or(2.0).clamp(0.5, 30.0);
    let lead = p.get("lead").and_then(Value::as_f64).unwrap_or(WINDOW).clamp(0.0, 1.0);
    let rate = s.active_sequence().ok_or(EngineError::NoSequence)?.settings.frame_rate;
    let follows = s.state.multicam_audio_follows_video;
    let mut report = Vec::new();
    let mut work: Vec<(Vec<ClipId>, Vec<(Tick, u32)>)> = Vec::new();
    for it in &clips {
        let src = s.project.sequence(it.item).ok_or_else(|| bad(cmd, "not a multi-camera clip"))?;
        let cams = src.cameras();
        let shown = cams.shown_angles();
        if shown.len() < 2 {
            report.push(json!({"clip": it.id.0, "skipped": "fewer than two cameras"}));
            continue;
        }
        if (it.speed - 1.0).abs() > 1e-6 || it.reverse {
            report.push(json!({"clip": it.id.0, "skipped": "only clips at normal speed"}));
            continue;
        }
        // speakers when the transcript knows two or more, else loudness
        let has_speakers = speaker_words(s, src).iter().map(|w| w.0).collect::<std::collections::BTreeSet<_>>().len() >= 2;
        let method = match str_p(p, "method") {
            Some("speakers") if !has_speakers => {
                return Err(bad(cmd, "the transcript has no speakers: transcribe with speaker labels, or use method \"loudness\""));
            }
            Some(m @ ("speakers" | "loudness")) => m,
            Some(m) => return Err(bad(cmd, format!("`method`: speakers or loudness, not {m}"))),
            None if has_speakers => "speakers",
            None => "loudness",
        };
        let camera = |n: u64| shown.get((n as usize).saturating_sub(1)).copied();
        let wide = match p.get("wide").and_then(Value::as_u64) {
            Some(n) => Some(camera(n).ok_or_else(|| bad(cmd, format!("`wide`: camera 1–{}", shown.len())))?),
            None => None,
        };
        let speaker_cams: Vec<usize> =
            p.get("speakerCameras").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).filter_map(camera).collect();
        let dur = it.duration.seconds();
        if method == "loudness" && dur > MAX_SECONDS {
            return Err(bad(cmd, "the clip is longer than three hours: split it first"));
        }
        let n = (dur / WINDOW).ceil() as usize;
        let act = activity(s, it.item, it.source_in.seconds(), n, method, &speaker_cams)?;
        let runs = plan(&act, (min_shot / WINDOW).round() as usize, wide, (4.0 / WINDOW) as usize);
        let lead_w = (lead / WINDOW).round() as usize;
        let switches: Vec<(Tick, u32)> = runs
            .iter()
            .enumerate()
            .map(|(k, &(w, angle))| {
                let w = if k == 0 { 0 } else { w.saturating_sub(lead_w) };
                (rate.snap_nearest(it.start + Tick::from_seconds_f64(w as f64 * WINDOW)), angle as u32)
            })
            .filter(|(t, _)| *t < it.end())
            .collect();
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let mut ids = vec![it.id];
        if follows {
            ids.extend(crate::multicam::audio_partners(q, it));
        }
        report.push(json!({"clip": it.id.0, "method": method, "cuts": switches.len().saturating_sub(1)}));
        work.push((ids, switches));
    }
    let cuts: usize = work.iter().map(|w| w.1.len().saturating_sub(1)).sum();
    if !work.is_empty() {
        s.edit_sequence("Auto Multi-Camera", move |seq, ctx, _| {
            for (ids, switches) in work {
                let mut cur = ids;
                for (k, (t, angle)) in switches.into_iter().enumerate() {
                    if k > 0 {
                        let new = edit::razor_items(seq, &cur, t, ctx);
                        // the pieces from t on carry the next angle and the next cuts
                        cur = seq
                            .all_tracks()
                            .flat_map(|tr| tr.items.iter())
                            .filter(|i| i.start == t && (new.contains(&i.id) || cur.contains(&i.id)))
                            .map(|i| i.id)
                            .collect();
                    }
                    edit::multicam::set_angle(seq, &cur, angle);
                }
            }
            Ok(())
        })?;
    }
    Ok(json!({"clips": report, "cuts": cuts}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "multicam.autoSwitch",
        label: "Auto Multi-Camera by Speaker…",
        menu: crate::ai_menu::EDITING,
        shortcut: None,
        params: r#"{"method":"speakers|loudness"? (default: speakers when the transcript has them),"minShot":seconds=2,"wide":camera?,"speakerCameras":[camera per speaker]?,"lead":seconds=0.25}"#,
        enabled: has_seq,
        run: auto_switch,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(angles: &[&str]) -> Vec<Vec<f64>> {
        // one string per angle: '#' speaking, '.' silent, one character a window
        angles.iter().map(|a| a.chars().map(|c| if c == '#' { 1.0 } else { 0.0 }).collect()).collect()
    }

    #[test]
    fn the_speaker_gets_the_shot_and_short_shots_join_a_neighbour() {
        let a = act(&["########..........##########", "........##########..#......."]);
        // min 4 windows: the single window of B at 20 is too short
        assert_eq!(plan(&a, 4, None, 16), vec![(0, 0), (8, 1), (18, 0)]);
        // a short pause keeps the shot
        let a = act(&["####...####", "..........."]);
        assert_eq!(plan(&a, 2, None, 16), vec![(0, 0)]);
    }

    #[test]
    fn crosstalk_and_long_silence_go_wide() {
        let a = act(&["######......######", "...###......######", "..................", "..................."]);
        // angle 2 is the wide shot
        let p = plan(&a, 2, Some(2), 4);
        assert_eq!(p, vec![(0, 0), (3, 2)], "{p:?}");
        assert_eq!(plan(&[], 2, None, 4), vec![]);
        assert_eq!(plan(&act(&["....", "...."]), 2, None, 4), vec![(0, 0)], "silence alone: one shot");
    }
}
