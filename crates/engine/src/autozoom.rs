//! Auto zooms (`clip.autoZoom`), as Jilo Studio adds them: on stressed phrases the picture punches
//! in and comes back after the phrase, as Scale keyframes of the clips' Motion effect.
//!
//! The moments come from the sequence transcript, one of two ways:
//! - `loudness` (the default, offline): cues at least 0.4 s long that are louder than the median
//!   cue (×1.02) in the sequence mix, loudest first, at least `minGap` apart;
//! - `meaning`: the text engine of Settings ▸ AI names the stressed phrases with a verbatim quote
//!   of 2–8 words; the quote is found in the transcript's words, so two stresses of one long cue
//!   get their own moments.
//!
//! A moment lasts 0.8–4 s. On every target clip (the selected video clips, else V1's) the Scale
//! keyframes are replaced: `cut` holds the zoom for the phrase (a jump), `smooth` eases in and out
//! over up to 0.35 s. `intensity` 1.02–1.6 (1.15) multiplies the clip's own scale; `minGap` 2–60 s
//! (6) also caps the count at the sequence length divided by it. One undo step.

use filmcraft_edit::transcript::SeqWord;
use filmcraft_project::{ClipId, Interpolation, Keyframe, ParamValue, TrackItem};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::ai_tasks::{Outcome, TaskWork, other, run, started};
use crate::commands::{CommandSpec, bad, f64_p, str_p};
use crate::highlights::{Cue, cue_spans};
use crate::{EngineError, Result, Session};

/// A stressed moment, sequence seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    pub start: f64,
    pub end: f64,
}

/// Shortest and longest zoom.
const MIN_MOMENT: f64 = 0.8;
const MAX_MOMENT: f64 = 4.0;

/// Moments by loudness (Jilo Studio's `pickLoudMoments`): cues of at least 0.4 s, louder than the
/// median cue ×1.02, loudest first, at least `min_gap` apart, each at most 4 s.
pub fn pick_loud_moments(cues: &[Cue], loudness: impl Fn(f64, f64) -> f64, min_gap: f64, max_count: usize) -> Vec<Moment> {
    let scored: Vec<(&Cue, f64)> = cues.iter().filter(|c| c.end - c.start >= 0.4).map(|c| (c, loudness(c.start, c.end))).collect();
    if scored.is_empty() {
        return Vec::new();
    }
    let mut levels: Vec<f64> = scored.iter().map(|s| s.1).collect();
    levels.sort_by(f64::total_cmp);
    let median = levels.get(levels.len() / 2).copied().filter(|m| *m > 0.0).unwrap_or(1e-6);
    let mut loud = scored.clone();
    loud.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut picked: Vec<Moment> = Vec::new();
    for (c, level) in loud {
        if picked.len() >= max_count || level < median * 1.02 {
            break;
        }
        if picked.iter().any(|m| (m.start - c.start).abs() < min_gap) {
            continue;
        }
        picked.push(Moment { start: c.start, end: c.end.min(c.start + MAX_MOMENT) });
    }
    picked.sort_by(|a, b| a.start.total_cmp(&b.start));
    picked
}

/// Lower-case letters and digits with single spaces (Cyrillic and Tajik letters kept).
fn comparable(s: &str) -> String {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Where `quote` is said within a cue: the time of its words (by the words' text, so a stress late
/// in a long cue is not its start). A quote whose tail the model changed is looked up by its first
/// three words. None when the cue does not hold it.
pub fn phrase_time(cue_words: &[SeqWord], quote: &str) -> Option<(f64, f64)> {
    let words: Vec<(String, &SeqWord)> = cue_words.iter().map(|w| (comparable(&w.text), w)).filter(|(t, _)| !t.is_empty()).collect();
    let target: Vec<String> = comparable(quote).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect();
    if words.is_empty() || target.is_empty() {
        return None;
    }
    // a word of the transcript may hold several comparable words ("well-known"): match on the flat list
    let flat: Vec<(&str, usize)> = words.iter().enumerate().flat_map(|(i, (t, _))| t.split(' ').map(move |p| (p, i))).collect();
    let find = |needle: &[String]| (0..flat.len()).find(|&i| needle.iter().enumerate().all(|(k, w)| flat.get(i + k).is_some_and(|f| f.0 == w)));
    let (at, len) = match find(&target) {
        Some(at) => (at, target.len()),
        None => {
            let head: Vec<String> = target.iter().take(3).cloned().collect();
            if head.join(" ").chars().count() < 4 {
                return None;
            }
            (find(&head)?, head.len())
        }
    };
    let first = words.get(flat.get(at)?.1)?.1;
    let last = words.get(flat.get(at + len - 1)?.1)?.1;
    Some((first.start.seconds(), first.end.seconds().max(last.end.seconds())))
}

/// The model's moments, placed in the transcript (Jilo Studio's `snapZoomMoments`): by the quote in
/// the cue the model's time falls in (else in any cue), else at the word starting by that time; the
/// length is kept to 0.8–4 s, moments closer than `min_gap` to an earlier one are dropped.
pub fn snap_zoom_moments(raw: &[Value], cues: &[(Cue, std::ops::Range<usize>)], words: &[SeqWord], min_gap: f64, max_count: usize) -> Vec<Moment> {
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
    let mut wanted: Vec<(f64, f64, String)> = raw
        .iter()
        .filter_map(|v| {
            let start = num(v, "start")?;
            Some((start, num(v, "end").unwrap_or(start + 2.0), v.get("quote").and_then(Value::as_str).unwrap_or_default().to_string()))
        })
        .collect();
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<Moment> = Vec::new();
    for (want_start, want_end, quote) in wanted {
        let cue_words = |r: &std::ops::Range<usize>| words.get(r.clone()).unwrap_or_default();
        let mut phrase = None;
        if !quote.trim().is_empty() {
            let home = cues.iter().position(|(c, _)| want_start >= c.start - 0.3 && want_start < c.end);
            let order = home.into_iter().chain((0..cues.len()).filter(|i| Some(*i) != home));
            for i in order {
                if let Some(p) = cues.get(i).and_then(|(_, r)| phrase_time(cue_words(r), &quote)) {
                    phrase = Some(p);
                    break;
                }
            }
        }
        let (start, end) = match phrase {
            Some(p) => p,
            None => {
                let Some(anchor) = words.iter().rev().find(|w| w.start.seconds() <= want_start + 0.3).or(words.first()) else { break };
                let end = words.iter().find(|w| w.end.seconds() >= want_end - 0.1).map_or(want_end, |w| w.end.seconds());
                (anchor.start.seconds(), end)
            }
        };
        let start = (start * 1000.0).round() / 1000.0;
        let end = ((end.max(start + MIN_MOMENT)).min(start + MAX_MOMENT) * 1000.0).round() / 1000.0;
        if out.iter().any(|m| (m.start - start).abs() < min_gap) {
            continue;
        }
        out.push(Moment { start, end });
        if out.len() >= max_count {
            break;
        }
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// How a zoom comes and goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A jump in and out, like a jump cut.
    Cut,
    /// Eased over up to 0.35 s.
    Smooth,
}

/// The Scale keyframes of a clip (Jilo Studio's `autoZoomKeys`), in its media time: `base` from
/// `clip_in`, `base × intensity` over each moment (`moments`: media-time ranges inside the clip).
pub fn zoom_keys(base: f64, zoomed: f64, moments: &[(Tick, Tick)], clip_in: Tick, clip_out: Tick, style: Style) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = Vec::new();
    let mut put = |t: Tick, v: f64, interp: Interpolation| {
        let mut k = Keyframe::new(t, ParamValue::Float(v));
        k.interp = interp;
        // a later key at the same time replaces the earlier one
        keys.retain(|o| o.time != t);
        keys.push(k);
    };
    put(clip_in, base, Interpolation::Hold);
    let ramp_max = Tick::from_seconds_f64(0.35);
    for (i, &(a, b)) in moments.iter().enumerate() {
        let next = moments.get(i + 1).map_or(clip_out, |m| m.0);
        match style {
            Style::Cut => {
                put(a, zoomed, Interpolation::Hold);
                put(b, base, Interpolation::Hold);
            }
            Style::Smooth => {
                let ramp = ramp_max.min(Tick((b - a).0 / 2));
                put(a, base, Interpolation::Bezier);
                put(a + ramp, zoomed, Interpolation::Hold);
                put(b, zoomed, Interpolation::Bezier);
                put((b + ramp).min(next), base, Interpolation::Hold);
            }
        }
    }
    keys.sort_by_key(|k| k.time);
    keys
}

/// The prompt for zooms by meaning (the transcript is the user message).
pub fn meaning_prompt(max_count: usize, min_gap: f64) -> String {
    [
        "You are a video editor adding punch-in zooms to a talking-head video for social media.".to_string(),
        format!("Choose up to {max_count} moments where the speaker stresses something: a punchline, a strong claim,"),
        "a surprising number, a turn of thought, an emotional peak. Skip filler and introductions.".into(),
        format!("Moments must be at least {min_gap} seconds apart."),
        "start is the start time of the stressed phrase, end is when it ends (at most 4 seconds later).".into(),
        "quote is the exact stressed phrase, 2-8 words copied verbatim from the transcript line —".into(),
        "a long line can hold several stressed phrases, give each its own moment and quote.".into(),
        "Use the timestamps from the transcript. Never invent content.".into(),
        String::new(),
        "The user message is the transcript, one cue per line as [start-end] text, times in seconds.".into(),
    ]
    .join("\n")
}

pub fn meaning_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"moments": {"type": "array", "items": {"type": "object", "properties": {
            "start": {"type": "number"}, "end": {"type": "number"}, "quote": {"type": "string"}
        }, "required": ["start", "end", "quote"]}}},
        "required": ["moments"]
    })
}

/// The video clips a zoom goes on: `clips`, else the selected video clips, else V1's.
fn targets(s: &Session, p: &Value) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let is_video = |c: &ClipId| q.find_item(*c).is_some_and(|(tid, _)| q.video_tracks.iter().any(|t| t.id == tid));
    if let Some(a) = p.get("clips").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).map(ClipId).filter(is_video).collect();
    }
    let selected: Vec<ClipId> = s.state.selection.iter().copied().filter(is_video).collect();
    if !selected.is_empty() {
        return selected;
    }
    q.video_tracks.first().map(|t| t.items.iter().filter(|i| i.enabled).map(|i| i.id).collect()).unwrap_or_default()
}

/// The mean RMS of the sequence mix over each `[start, end)` (seconds).
fn mix_loudness(s: &Session, ranges: &[(f64, f64)]) -> Result<Vec<f64>> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.sample_rate.max(8000);
    let project = s.project.clone();
    let provider = s.media.full_res_provider(project.clone(), s.services.clone());
    Ok(ranges
        .iter()
        .map(|&(a, b)| {
            let start = (a.max(0.0) * f64::from(rate)) as i64;
            let n = (((b - a).max(0.0) * f64::from(rate)) as usize).min(rate as usize * 30);
            if n == 0 {
                return 0.0;
            }
            let buf = filmcraft_render::audio::mix_sequence(&project, q, start, n, &provider);
            let mono = filmcraft_speech::downmix(&buf.channels);
            let sum: f64 = mono.iter().map(|x| f64::from(*x) * f64::from(*x)).sum();
            (sum / mono.len().max(1) as f64).sqrt()
        })
        .collect())
}

/// The keys of one clip for `moments` (sequence seconds); None when no moment falls in it.
fn clip_keys(it: &TrackItem, moments: &[Moment], intensity: f64, style: Style) -> Option<(f64, Vec<Keyframe>)> {
    let base = it.effect("motion")?.params.get("scale")?.f64_at(it.effect_time_at(it.start));
    let zoomed = ((base * intensity).clamp(10.0, 800.0) * 10.0).round() / 10.0;
    let last = it.end() - Tick(1);
    let inside: Vec<(Tick, Tick)> = moments
        .iter()
        .filter_map(|m| {
            let (a, b) = (Tick::from_seconds_f64(m.start).max(it.start), Tick::from_seconds_f64(m.end).min(last));
            ((b - a).seconds() >= 0.2).then(|| (it.effect_time_at(a), it.effect_time_at(b)))
        })
        .collect();
    if inside.is_empty() {
        return None;
    }
    Some((base, zoom_keys(base, zoomed, &inside, it.effect_time_at(it.start), it.effect_time_at(last), style)))
}

fn auto_zoom(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.autoZoom";
    let meaning = match str_p(p, "method").unwrap_or("loudness") {
        "loudness" => false,
        "meaning" => true,
        other => return Err(bad(cmd, format!("`method`: loudness or meaning, not `{other}`"))),
    };
    let style = match str_p(p, "style").unwrap_or("cut") {
        "cut" => Style::Cut,
        "smooth" => Style::Smooth,
        other => return Err(bad(cmd, format!("`style`: cut or smooth, not `{other}`"))),
    };
    let intensity = f64_p(p, "intensity").filter(|x| x.is_finite()).unwrap_or(1.15).clamp(1.02, 1.6);
    let min_gap = f64_p(p, "minGap").filter(|x| x.is_finite()).unwrap_or(6.0).clamp(2.0, 60.0);
    let clips = targets(s, p);
    if clips.is_empty() {
        return Err(bad(cmd, "no video clips to zoom (select some, or put the picture on V1)"));
    }
    let words = crate::transcript::sequence_words(s);
    let spans = cue_spans(&words);
    if spans.is_empty() {
        return Err(bad(cmd, "the sequence has no transcript (Transcribe first)"));
    }
    let cues: Vec<Cue> = spans.iter().map(|(c, _)| c.clone()).collect();
    let seconds = s.active_sequence().map_or(0.0, |q| q.duration().seconds());
    let max_count = ((seconds / min_gap).floor() as usize).max(1);
    let moments = if meaning {
        // the transcript with two decimals, as Jilo Studio sends it
        let transcript: String = cues
            .iter()
            .map(|c| format!("[{:.2}-{:.2}] {}", c.start, c.end, c.text.split_whitespace().collect::<Vec<_>>().join(" ")))
            .collect::<Vec<_>>()
            .join("\n");
        let inputs = json!({"transcript": transcript, "minGap": min_gap, "maxCount": max_count});
        let system = meaning_prompt(max_count, min_gap);
        let work: TaskWork = Box::new(move |engine, ctx, progress| {
            if !progress(0.1, "Finding stressed phrases") {
                return Err(filmcraft_ai::AiError::Cancelled);
            }
            let v = engine.json(ctx, &system, &transcript, Some(&meaning_schema()), 0.3)?;
            let _ = progress(1.0, "Stressed phrases found");
            Ok(v)
        });
        let answer = match run(s, p, cmd, "Finding zoom moments", &inputs, "the transcript", work)? {
            Outcome::Started(id) => return Ok(started(id)),
            Outcome::Ready(v) => v,
        };
        let Some(raw) = answer.get("moments").and_then(Value::as_array) else {
            return Err(other("the engine did not answer with a list of moments; try again or another engine in Settings ▸ AI"));
        };
        snap_zoom_moments(raw, &spans, &words, min_gap, max_count)
    } else {
        let ranges: Vec<(f64, f64)> = cues.iter().map(|c| (c.start, c.end)).collect();
        let levels = mix_loudness(s, &ranges)?;
        pick_loud_moments(&cues, |a, b| ranges.iter().position(|r| *r == (a, b)).and_then(|i| levels.get(i).copied()).unwrap_or(0.0), min_gap, max_count)
    };
    if moments.is_empty() {
        return Err(other(if meaning { "the engine found no stressed phrases" } else { "no phrase stands out by loudness" }));
    }
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let zoomed = s.edit("Auto Zoom", |pr, _| {
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let mut n = 0;
        for c in &clips {
            let Some((_, it)) = q.find_item_mut(*c) else { continue };
            let Some((base, keys)) = clip_keys(it, &moments, intensity, style) else { continue };
            let Some(scale) = it.effect_mut("motion").and_then(|e| e.params.get_mut("scale")) else { continue };
            scale.value = ParamValue::Float(base);
            // one key is a still frame: no animation needed
            scale.keyframes = if keys.len() > 1 { keys } else { Vec::new() };
            n += 1;
        }
        if n == 0 {
            return Err(EngineError::Other("no stressed phrase falls on the chosen clips".into()));
        }
        Ok(n)
    })?;
    Ok(json!({
        "method": if meaning { "meaning" } else { "loudness" },
        "moments": moments.iter().map(|m| json!({"start": m.start, "end": m.end})).collect::<Vec<_>>(),
        "clips": zoomed,
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "clip.autoZoom",
        label: "Auto Zoom…",
        menu: &["Clip"],
        shortcut: None,
        params: r#"{"method":"loudness|meaning"?,"intensity":1.02-1.6?,"style":"cut|smooth"?,"minGap":2-60?,"clips":[id]?,"engine":"gemini|openai"?,"model":str?,"confirm":bool?,"background":bool?}"#,
        enabled: crate::transcript::has_transcript,
        run: auto_zoom,
        journal: true,
    }]
}
