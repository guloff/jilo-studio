//! Checking an edit without watching it: what an assistant (or a careful editor) looks for before
//! an export, as data, and a sheet of frames in one picture.
//!
//! | Command | |
//! |---|---|
//! | `sequence.check` | problems of the active sequence: black gaps, flash frames, offline media, clips past their media, linked sound out of sync, overlapping or hard-to-read captions, markers past the end |
//! | `program.renderFrames` | the Program frames at a list of times, as one PNG contact sheet (rendered by the engine: no window, the playhead stays) |
//!
//! Every problem has a `kind`, a `severity` (`error`: it shows in the export; `warning`: worth a
//! look), where it is (`at` seconds, `track`, `clip`) and a sentence. Kinds: `black`,
//! `flashFrame`, `offline`, `pastMediaEnd`, `outOfSync`, `captionOverlap`, `captionPastEnd`,
//! `captionTooFast`, `captionTooLong`, `markerPastEnd`.

use std::collections::{BTreeMap, HashSet};

use filmcraft_project::{ClipId, Sequence};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad};
use crate::{EngineError, Result, Session};

/// Clips shorter than this many frames are flash frames.
const FLASH_FRAMES: i64 = 3;
/// Reading speed above which a caption is too fast (characters a second, Netflix's adult limit).
const MAX_CPS: f64 = 20.0;
/// Characters a caption line holds comfortably.
const MAX_LINE: usize = 42;

fn problem(kind: &str, severity: &str, at: Tick, track: Option<String>, clip: Option<ClipId>, message: String) -> Value {
    json!({"kind": kind, "severity": severity, "at": (at.seconds() * 1000.0).round() / 1000.0, "track": track, "clip": clip.map(|c| c.0), "message": message})
}

fn tc(t: Tick) -> String {
    let s = t.seconds().max(0.0);
    format!("{}:{:05.2}", (s / 60.0).floor() as i64, s % 60.0)
}

/// The ranges no enabled video clip covers between 0 and the end of the picture.
fn black_gaps(q: &Sequence) -> Vec<(Tick, Tick)> {
    let mut spans: Vec<(Tick, Tick)> =
        q.video_tracks.iter().filter(|t| t.enabled).flat_map(|t| t.items.iter().filter(|i| i.enabled).map(|i| (i.start, i.end()))).collect();
    spans.sort();
    let mut gaps = Vec::new();
    let mut covered = Tick::ZERO;
    for (a, b) in spans {
        if a > covered {
            gaps.push((covered, a));
        }
        covered = covered.max(b);
    }
    gaps
}

fn check(s: &mut Session, _p: &Value) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let rate = q.settings.frame_rate;
    let frame = rate.frame_duration();
    // the end of the picture and sound: a caption or a marker past it lengthens the sequence
    let end = q.all_tracks().flat_map(|t| t.items.iter().map(|i| i.end())).max().unwrap_or(Tick::ZERO);
    let mut out = Vec::new();
    let vlabel = |i: usize| format!("V{}", i + 1);
    let alabel = |i: usize| format!("A{}", i + 1);
    // the picture: black between clips, flash frames
    for (a, b) in black_gaps(&q) {
        let frames = (b - a).0 / frame.0.max(1);
        out.push(problem(
            "black",
            "error",
            a,
            None,
            None,
            format!("{} of black at {} (nothing on any video track)", if frames <= 1 { "1 frame".to_string() } else { format!("{frames} frames") }, tc(a)),
        ));
    }
    for (ti, t) in q.video_tracks.iter().enumerate() {
        for i in t.items.iter().filter(|i| i.enabled && i.duration < Tick(frame.0 * FLASH_FRAMES)) {
            let n = (i.duration.0 / frame.0.max(1)).max(1);
            out.push(problem(
                "flashFrame",
                "warning",
                i.start,
                Some(vlabel(ti)),
                Some(i.id),
                format!("{} is only {n} frame(s) long at {}", i.name, tc(i.start)),
            ));
        }
    }
    // the media: offline, or used past its end
    let missing: HashSet<u64> =
        s.execute("media.findMissing", json!({}))?["missing"].as_array().into_iter().flatten().filter_map(|m| m["item"].as_u64()).collect();
    let tracks: Vec<(String, &filmcraft_project::Track)> =
        q.video_tracks.iter().enumerate().map(|(i, t)| (vlabel(i), t)).chain(q.audio_tracks.iter().enumerate().map(|(i, t)| (alabel(i), t))).collect();
    let mut offline_items: BTreeMap<u64, usize> = BTreeMap::new();
    for (label, t) in &tracks {
        for i in &t.items {
            if missing.contains(&i.item.0) {
                *offline_items.entry(i.item.0).or_default() += 1;
                if offline_items[&i.item.0] == 1 {
                    out.push(problem("offline", "error", i.start, Some(label.clone()), Some(i.id), format!("{} is offline: its file is missing", i.name)));
                }
                continue;
            }
            if i.frame_hold.is_some() {
                continue;
            }
            if let Some(len) = crate::media_duration(&s.project, &s.media, i.item)
                && !i.reverse
                && i.source_out() > len + frame
            {
                let over = i.source_out() - len;
                out.push(problem(
                    "pastMediaEnd",
                    "error",
                    i.end() - over,
                    Some(label.clone()),
                    Some(i.id),
                    format!("{} runs {} frame(s) past the end of its media", i.name, (over.0 / frame.0.max(1)).max(1)),
                ));
            }
        }
    }
    // linked picture and sound that no longer line up
    for (vi, vt) in q.video_tracks.iter().enumerate() {
        for v in &vt.items {
            let Some(link) = v.link else { continue };
            for (ai, at) in q.audio_tracks.iter().enumerate() {
                for a in at.items.iter().filter(|a| a.link == Some(link) && a.item == v.item) {
                    let off = (a.start - v.start) - (a.source_in - v.source_in);
                    if off.0.abs() >= frame.0 {
                        out.push(problem(
                            "outOfSync",
                            "warning",
                            v.start,
                            Some(format!("{}/{}", vlabel(vi), alabel(ai))),
                            Some(v.id),
                            format!(
                                "the sound of {} is {} frame(s) {} its picture",
                                v.name,
                                off.0.abs() / frame.0.max(1),
                                if off.0 > 0 { "behind" } else { "ahead of" }
                            ),
                        ));
                    }
                }
            }
        }
    }
    // captions
    for (ci, ct) in q.caption_tracks.iter().enumerate() {
        let label = format!("C{}", ci + 1);
        let mut caps: Vec<&filmcraft_project::Caption> = ct.captions.iter().collect();
        caps.sort_by_key(|c| c.start);
        for w in caps.windows(2) {
            if w[1].start < w[0].start + w[0].duration {
                out.push(problem(
                    "captionOverlap",
                    "error",
                    w[1].start,
                    Some(label.clone()),
                    Some(w[1].id),
                    format!("\"{}\" starts before \"{}\" ends", w[1].text, w[0].text),
                ));
            }
        }
        for c in caps {
            if c.start + c.duration > end && end > Tick::ZERO {
                out.push(problem(
                    "captionPastEnd",
                    "warning",
                    c.start,
                    Some(label.clone()),
                    Some(c.id),
                    format!("\"{}\" runs past the end of the picture and sound", c.text),
                ));
            }
            let chars = c.text.chars().filter(|ch| !ch.is_whitespace() || *ch == ' ').count() as f64;
            let secs = c.duration.seconds();
            if secs > 0.0 && chars / secs > MAX_CPS {
                out.push(problem(
                    "captionTooFast",
                    "warning",
                    c.start,
                    Some(label.clone()),
                    Some(c.id),
                    format!("\"{}\" asks {:.0} characters a second (more than {MAX_CPS:.0} is hard to read)", c.text, chars / secs),
                ));
            }
            // the renderer wraps a long line: count the lines it will take
            let lines: usize = c.text.lines().map(|l| l.chars().count().div_ceil(MAX_LINE).max(1)).sum();
            if lines > 2 {
                out.push(problem(
                    "captionTooLong",
                    "warning",
                    c.start,
                    Some(label.clone()),
                    Some(c.id),
                    format!("\"{}\" is more than two lines of {MAX_LINE} characters", c.text),
                ));
            }
        }
    }
    for m in q.markers.iter().filter(|m| m.start > end) {
        out.push(problem("markerPastEnd", "warning", m.start, None, None, format!("marker \"{}\" is past the end of the picture and sound", m.name)));
    }
    out.sort_by(|a, b| a["at"].as_f64().partial_cmp(&b["at"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    let errors = out.iter().filter(|p| p["severity"] == "error").count();
    let mut counts = serde_json::Map::new();
    for p in &out {
        let k = p["kind"].as_str().unwrap_or_default().to_string();
        let n = counts.get(&k).and_then(Value::as_u64).unwrap_or(0);
        counts.insert(k, json!(n + 1));
    }
    Ok(json!({
        "sequence": s.state.active_sequence.map(|i| i.0),
        "duration": q.duration().seconds(),
        "contentEnd": end.seconds(),
        "ok": errors == 0,
        "errors": errors,
        "warnings": out.len() - errors,
        "counts": counts,
        "problems": out,
    }))
}

/// The Program frames at `seconds`, `columns` to a row, each at most `maxSide` points on its longer
/// side, as one PNG written to `path` (default: a new file in the temporary folder).
fn render_frames(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "program.renderFrames";
    let times: Vec<f64> = p
        .get("seconds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).filter(|t| t.is_finite() && *t >= 0.0).collect())
        .ok_or_else(|| bad(cmd, "`seconds`: a list of times in seconds"))?;
    if times.is_empty() || times.len() > 64 {
        return Err(bad(cmd, "`seconds`: 1 to 64 times"));
    }
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (w, h) = (f64::from(q.settings.width.max(1)), f64::from(q.settings.height.max(1)));
    let side = p.get("maxSide").and_then(Value::as_u64).unwrap_or(if times.len() == 1 { 960 } else { 480 }).clamp(64, 3840) as f64;
    let scale = (side / w.max(h)).min(1.0) as f32;
    let cols = p.get("columns").and_then(Value::as_u64).map_or_else(|| (times.len() as f64).sqrt().ceil() as usize, |c| c as usize).clamp(1, times.len());
    let rows = times.len().div_ceil(cols);
    let mut frames = Vec::new();
    for t in &times {
        let img = s.try_render_program_at(scale, Tick::from_seconds_f64(*t))?;
        frames.push((img.w as u32, img.h as u32, img.over_black_rgba8()));
    }
    let (fw, fh) = (frames.iter().map(|f| f.0).max().unwrap_or(1), frames.iter().map(|f| f.1).max().unwrap_or(1));
    let gap = 4u32;
    let (sw, sh) = (cols as u32 * (fw + gap) - gap, rows as u32 * (fh + gap) - gap);
    let mut sheet = [24u8, 24, 24, 255].repeat((sw * sh) as usize);
    let mut placed = Vec::new();
    for (k, ((iw, ih, rgba), t)) in frames.into_iter().zip(&times).enumerate() {
        let (x, y) = ((k % cols) as u32 * (fw + gap), (k / cols) as u32 * (fh + gap));
        for row in 0..ih.min(sh - y) {
            let src = (row * iw * 4) as usize;
            let dst = (((y + row) * sw + x) * 4) as usize;
            let n = (iw.min(sw - x) * 4) as usize;
            if let (Some(from), Some(to)) = (rgba.get(src..src + n), sheet.get_mut(dst..dst + n)) {
                to.copy_from_slice(from);
            }
        }
        placed.push(json!({"seconds": t, "x": x, "y": y, "w": iw, "h": ih}));
    }
    let path = match p.get("path").and_then(Value::as_str) {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::env::temp_dir().join(format!("filmcraft-frames-{}-{n}.png", std::process::id()))
        }
    };
    let png = filmcraft_export::encode_png(sheet, sw, sh).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::write(&path, png).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(json!({"path": path.to_string_lossy(), "width": sw, "height": sh, "columns": cols, "frames": placed}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec("sequence.check", "Check Sequence", "{}", check as fn(&mut Session, &Value) -> Result<Value>),
        spec("program.renderFrames", "Render Frames", r#"{"seconds":[f64],"maxSide":u32?,"columns":u32?,"path":str?}"#, render_frames),
    ]
}

#[cfg(test)]
#[path = "review_tests.rs"]
mod tests;
