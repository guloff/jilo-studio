//! Pauses by sound: the silent stretches of the active sequence's mix, for footage without a
//! transcript (Sequence ▸ Remove Silent Pauses). The transcript-based pauses are `transcript.removePauses`.
//!
//! | command | does |
//! |---|---|
//! | `audio.findSilences` | the silences of the sequence mix (sequence seconds), without changing anything |
//! | `audio.removeSilences` | ripple-delete them, keeping `keepSeconds` of each pause on both sides (one undo step) |
//!
//! The mix (the full mixer graph: mutes, solos, levels) is measured in 10-second blocks as 10 ms
//! RMS frames ([`filmcraft_audio_dsp::silence`]); a pause is a run below `thresholdDb` (default −35
//! dBFS, −70…−10) lasting at least `minSeconds` (default 0.6, 0.2…5), the defaults of Jilo Studio.
//! Cut points are snapped to frames.

use filmcraft_audio_dsp::silence;
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, f64_p, has_seq};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

const BLOCK_SECONDS: u32 = 10;

/// The pauses `p` asks for, in sequence time, before padding.
pub fn find(s: &Session, p: &Value) -> Result<(Vec<TimeRange>, f64)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if !q.audio_tracks.iter().any(|t| t.items.iter().any(|i| i.enabled)) {
        return Err(bad("audio.findSilences", "the sequence has no audio clips"));
    }
    let threshold = f64_p(p, "thresholdDb").unwrap_or(-35.0).clamp(-70.0, -10.0) as f32;
    let min = f64_p(p, "minSeconds").unwrap_or(0.6).clamp(0.2, 5.0);
    let dur = q.duration();
    let rate = q.settings.sample_rate.max(8000);
    let total = dur.to_units_floor(rate as i64).max(0) as usize;
    let project = s.project.clone();
    let provider = s.media.full_res_provider(project.clone(), s.services.clone());
    let block = (BLOCK_SECONDS * rate) as usize;
    let mut levels = Vec::with_capacity(total / (rate as usize / 100).max(1) + 1);
    let mut start = 0usize;
    while start < total {
        let n = block.min(total - start);
        let buf = filmcraft_render::audio::mix_sequence(&project, q, start as i64, n, &provider);
        let mono: Vec<f32> = filmcraft_speech::downmix(&buf.channels);
        levels.extend(silence::frame_levels_db(&mono, rate));
        start += n;
    }
    let seconds = dur.seconds();
    let spans = silence::silences_in_levels(&levels, seconds, threshold, min);
    Ok((spans.into_iter().map(|sp| TimeRange::from_bounds(Tick::from_seconds_f64(sp.start), Tick::from_seconds_f64(sp.end))).collect(), seconds))
}

/// `ranges` padded by `keep` on the sides that touch sound (a pause at the very start or end of the
/// sequence is cut to the edge) and snapped inward to frames.
fn padded(s: &Session, ranges: &[TimeRange], keep: Tick, dur: Tick) -> Vec<TimeRange> {
    let rate = s.sequence_rate();
    ranges
        .iter()
        .filter_map(|r| {
            let a = if r.start <= Tick::ZERO { Tick::ZERO } else { r.start + keep };
            let b = if r.end() >= dur { dur } else { r.end() - keep };
            let mut a2 = rate.snap(a);
            if a2 < a {
                a2 += rate.frame_duration();
            }
            let b2 = if b >= dur { dur } else { rate.snap(b) };
            (b2 > a2).then(|| TimeRange::from_bounds(a2, b2))
        })
        .collect()
}

fn find_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (ranges, seconds) = find(s, p)?;
    let total: f64 = ranges.iter().map(|r| r.duration.seconds()).sum();
    Ok(json!({
        "silences": ranges.iter().map(|r| json!({"start": r.start.seconds(), "end": r.end().seconds()})).collect::<Vec<_>>(),
        "silentSeconds": total,
        "sequenceSeconds": seconds,
    }))
}

fn remove_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let (ranges, _) = find(s, p)?;
    let dur = s.active_sequence().map(|q| q.duration()).unwrap_or_default();
    let keep = Tick::from_seconds_f64(f64_p(p, "keepSeconds").unwrap_or(0.15).clamp(0.0, 2.0));
    let cut = padded(s, &ranges, keep, dur);
    if cut.len() == 1 && cut[0].start == Tick::ZERO && cut[0].end() >= dur {
        return Err(bad("audio.removeSilences", "the whole sequence is silent at this threshold; nothing was removed"));
    }
    let n = cut.len();
    if n == 0 {
        return Ok(json!({"removed": 0, "seconds": 0.0}));
    }
    let total = s.edit_sequence("Remove Silent Pauses", |q, ctx, _| Ok(filmcraft_edit::transcript::ripple_delete_ranges(q, cut, ctx)))?;
    Ok(json!({"removed": n, "seconds": total.seconds()}))
}

const PARAMS: &str = r#"{"thresholdDb":-35?,"minSeconds":0.6?,"keepSeconds":0.15?}"#;

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "audio.findSilences",
            label: "Find Silent Pauses",
            menu: &[],
            shortcut: None,
            params: PARAMS,
            enabled: has_seq,
            run: find_cmd as Run,
            journal: false,
        },
        CommandSpec {
            id: "audio.removeSilences",
            label: "Remove Silent Pauses",
            menu: &["Sequence"],
            shortcut: None,
            params: PARAMS,
            enabled: has_seq,
            run: remove_cmd as Run,
            journal: true,
        },
    ]
}
