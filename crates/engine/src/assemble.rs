//! `sequence.assemble`: build a whole sequence from an edit plan in one call and one undo step —
//! what an assistant (or a script) that has decided the cut needs, instead of writing a project
//! file by hand or sending hundreds of commands.
//!
//! The plan names a new sequence (size, rate), its video tracks bottom to top and its audio
//! tracks, each a list of clips; caption tracks with their cues; and markers (chapters). A clip
//! is a piece of a file (`media`: a path, imported if it is not in the project yet, or `item`)
//! from `in` to `out` seconds of the source, or a still for `duration` seconds; it goes at `at`
//! seconds, or right after the previous clip on its track. A video clip's sound goes on the audio
//! track of the same number, linked (`audio: false` leaves it out, `audioTrack` puts it
//! elsewhere). Framing is the Motion effect (`scale` %, `position` and `anchor` in pixels), plus
//! `opacity` %, `gainDb` for the sound, `name` and `label`.
//!
//! Everything is checked before anything is made; a step that still fails takes the whole
//! assembly back. Times land on the sequence's frames.

use std::collections::HashMap;

use filmcraft_project::{ClipId, ItemId, Label};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad};
use crate::{EngineError, Result, Session};

const CMD: &str = "sequence.assemble";

/// One clip of the plan, checked.
#[derive(Clone, Debug)]
struct Clip {
    source: Source,
    /// Source seconds the clip starts at (0 for a still).
    src_in: f64,
    /// Length in seconds.
    duration: f64,
    /// Where it goes (None: after the previous clip on the track).
    at: Option<f64>,
    name: Option<String>,
    label: Option<Label>,
    /// The sound goes too (video clips); on this audio track (1-based) if given.
    audio: bool,
    audio_track: Option<usize>,
    scale: Option<f64>,
    position: Option<[f64; 2]>,
    anchor: Option<[f64; 2]>,
    opacity: Option<f64>,
    gain_db: Option<f64>,
}

#[derive(Clone, Debug)]
enum Source {
    Path(String),
    Item(ItemId),
}

fn num(v: &Value, k: &str, at: &str) -> Result<Option<f64>> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(x) => x.as_f64().filter(|f| f.is_finite()).map(Some).ok_or_else(|| bad(CMD, format!("{at}: `{k}` must be a number"))),
    }
}

fn pair(v: &Value, k: &str, at: &str) -> Result<Option<[f64; 2]>> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) if a.len() == 2 && a.iter().all(|x| x.as_f64().is_some_and(f64::is_finite)) => {
            Ok(Some([a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)]))
        }
        Some(_) => Err(bad(CMD, format!("{at}: `{k}` must be [x, y] in pixels"))),
    }
}

fn parse_clip(v: &Value, at: &str, video: bool) -> Result<Clip> {
    if !v.is_object() {
        return Err(bad(CMD, format!("{at}: a clip is an object")));
    }
    let source = match (v.get("media").and_then(Value::as_str), v.get("item").and_then(Value::as_u64)) {
        (Some(p), None) => {
            if !std::path::Path::new(p).is_absolute() {
                return Err(bad(CMD, format!("{at}: `media` must be an absolute path, got `{p}`")));
            }
            if !std::path::Path::new(p).exists() {
                return Err(bad(CMD, format!("{at}: no file {p}")));
            }
            Source::Path(p.to_string())
        }
        (None, Some(i)) => Source::Item(ItemId(i)),
        _ => return Err(bad(CMD, format!("{at}: give `media` (a path) or `item` (an id)"))),
    };
    let (src_in, out, len) = (num(v, "in", at)?, num(v, "out", at)?, num(v, "duration", at)?);
    let src_in = src_in.unwrap_or(0.0);
    if src_in < 0.0 {
        return Err(bad(CMD, format!("{at}: `in` is before the start of the file")));
    }
    let duration = match (out, len) {
        (Some(o), None) => o - src_in,
        (None, Some(d)) => d,
        (Some(_), Some(_)) => return Err(bad(CMD, format!("{at}: give `out` or `duration`, not both"))),
        (None, None) => return Err(bad(CMD, format!("{at}: give `out` (source seconds) or `duration` (seconds)"))),
    };
    if duration <= 0.0 {
        return Err(bad(CMD, format!("{at}: the clip has no length ({duration} s)")));
    }
    let at_s = num(v, "at", at)?;
    if at_s.is_some_and(|a| a < 0.0) {
        return Err(bad(CMD, format!("{at}: `at` is before the start of the sequence")));
    }
    let label = match v.get("label") {
        None | Some(Value::Null) => None,
        Some(l) => Some(serde_json::from_value::<Label>(l.clone()).map_err(|_| bad(CMD, format!("{at}: unknown label {l}")))?),
    };
    let audio_track = match v.get("audioTrack") {
        None | Some(Value::Null) => None,
        Some(t) => Some(
            t.as_u64()
                .or_else(|| t.as_str().and_then(|s| s.strip_prefix('A').or(Some(s))).and_then(|s| s.parse().ok()))
                .filter(|n| *n >= 1)
                .ok_or_else(|| bad(CMD, format!("{at}: `audioTrack` is a track number (1 = A1) or \"A1\"")))? as usize,
        ),
    };
    Ok(Clip {
        source,
        src_in,
        duration,
        at: at_s,
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        label,
        audio: video && v.get("audio").and_then(Value::as_bool).unwrap_or(true),
        audio_track,
        scale: num(v, "scale", at)?,
        position: pair(v, "position", at)?,
        anchor: pair(v, "anchor", at)?,
        opacity: num(v, "opacity", at)?,
        gain_db: num(v, "gainDb", at)?,
    })
}

fn parse_tracks(p: &Value, key: &str, video: bool) -> Result<Vec<Vec<Clip>>> {
    let Some(v) = p.get(key) else { return Ok(Vec::new()) };
    let tracks = v.as_array().ok_or_else(|| bad(CMD, format!("`{key}` is a list of tracks, each a list of clips")))?;
    let tag = if video { "V" } else { "A" };
    tracks
        .iter()
        .enumerate()
        .map(|(t, clips)| {
            let clips = clips.as_array().ok_or_else(|| bad(CMD, format!("{tag}{}: a track is a list of clips", t + 1)))?;
            clips.iter().enumerate().map(|(k, c)| parse_clip(c, &format!("{tag}{} clip {}", t + 1, k + 1), video)).collect()
        })
        .collect()
}

/// The project item of a source, importing a path once.
fn item_of(s: &mut Session, src: &Source, cache: &mut HashMap<String, ItemId>) -> Result<ItemId> {
    match src {
        Source::Item(i) => s.project.item(*i).map(|_| *i).ok_or_else(|| bad(CMD, format!("no item {}", i.0))),
        Source::Path(p) => {
            if let Some(i) = cache.get(p) {
                return Ok(*i);
            }
            let r = s.execute("file.import", json!({"paths": [p]}))?;
            let id = r["items"][0]
                .as_u64()
                .or_else(|| r["duplicates"][0]["item"].as_u64())
                .map(ItemId)
                .ok_or_else(|| EngineError::Other(format!("{p}: could not be imported")))?;
            cache.insert(p.clone(), id);
            Ok(id)
        }
    }
}

fn assemble(s: &mut Session, p: &Value) -> Result<Value> {
    let name = p.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()).ok_or_else(|| bad(CMD, "`name`: the new sequence's name"))?;
    let video = parse_tracks(p, "video", true)?;
    let audio = parse_tracks(p, "audio", false)?;
    if video.iter().chain(&audio).all(Vec::is_empty) {
        return Err(bad(CMD, "the plan has no clips: `video` and `audio` are lists of tracks, each a list of clips"));
    }
    let captions = match p.get("captions") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err(bad(CMD, "`captions` is a list of caption tracks {name?, language?, cues: [{start, end, text}], style?}")),
    };
    for (k, c) in captions.iter().enumerate() {
        let cues = c.get("cues").and_then(Value::as_array).ok_or_else(|| bad(CMD, format!("caption track {}: `cues` is a list", k + 1)))?;
        for (j, q) in cues.iter().enumerate() {
            let at = format!("caption track {} cue {}", k + 1, j + 1);
            let (a, b) = (num(q, "start", &at)?, num(q, "end", &at)?);
            match (a, b, q.get("text").and_then(Value::as_str)) {
                (Some(a), Some(b), Some(_)) if a >= 0.0 && b > a => {}
                _ => return Err(bad(CMD, format!("{at}: needs `start` < `end` (seconds, from 0) and `text`"))),
            }
        }
    }
    let markers = match p.get("markers") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err(bad(CMD, "`markers` is a list of {at, name, comment?, kind?: chapter|comment, color?, duration?}")),
    };
    for (k, m) in markers.iter().enumerate() {
        let at = format!("marker {}", k + 1);
        if num(m, "at", &at)?.is_none_or(|a| a < 0.0) {
            return Err(bad(CMD, format!("{at}: `at` (seconds, from 0)")));
        }
        if let Some(kind) = m.get("kind").and_then(Value::as_str)
            && !matches!(kind, "chapter" | "comment")
        {
            return Err(bad(CMD, format!("{at}: `kind` is chapter or comment, got `{kind}`")));
        }
    }
    let n_audio = audio
        .len()
        .max(video.iter().enumerate().flat_map(|(t, cs)| cs.iter().filter(|c| c.audio).map(move |c| c.audio_track.unwrap_or(t + 1))).max().unwrap_or(0));
    let label = format!("Assemble Sequence: {name}");
    crate::brand_kit::as_one_step(s, &label, |s| {
        let mut settings = json!({"name": name, "video": video.len().max(1), "audio": n_audio.max(1)});
        for k in ["width", "height", "fps", "sampleRate"] {
            if let Some(v) = p.get(k) {
                settings[k] = v.clone();
            }
        }
        let seq = s.execute("file.newSequence", settings)?["sequence"].as_u64().ok_or_else(|| EngineError::Other("no sequence was made".into()))?;
        s.execute("sequence.open", json!({"item": seq}))?;
        let rate = s.active_sequence().ok_or(EngineError::NoSequence)?.settings.frame_rate;
        let frame = |sec: f64| rate.snap_nearest(Tick::from_seconds_f64(sec));
        let mut cache = HashMap::new();
        let mut placed: Vec<(ClipId, Option<String>, Option<Label>)> = Vec::new();
        let mut out_tracks = serde_json::Map::new();
        for (kind, tracks) in [("V", &video), ("A", &audio)] {
            for (t, clips) in tracks.iter().enumerate() {
                let mut cursor = Tick::ZERO;
                let mut ids = Vec::new();
                for c in clips {
                    let item = item_of(s, &c.source, &mut cache)?;
                    let start = c.at.map_or(cursor, frame);
                    let dur = frame(c.duration).max(rate.frame_duration());
                    let mut q = json!({"item": item.0, "time": start.0, "sourceIn": Tick::from_seconds_f64(c.src_in).0, "duration": dur.0});
                    if kind == "V" {
                        q["track"] = json!(format!("V{}", t + 1));
                        let has_audio = s.project.item(item).is_some_and(|i| i.has_audio());
                        if c.audio && has_audio {
                            q["audioTrack"] = json!(format!("A{}", c.audio_track.unwrap_or(t + 1)));
                        } else {
                            q["audio"] = json!(false);
                        }
                    } else {
                        q["track"] = json!(format!("A{}", t + 1));
                    }
                    let r = s.execute("timeline.place", q)?;
                    let clip_ids: Vec<ClipId> = r["clips"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(ClipId).collect();
                    let Some(&first) = clip_ids.first() else {
                        return Err(EngineError::Other(format!("{kind}{}: a clip was not placed", t + 1)));
                    };
                    if kind == "V" {
                        for (param, v) in
                            [("scale", c.scale.map(|x| json!(x))), ("position", c.position.map(|x| json!(x))), ("anchor", c.anchor.map(|x| json!(x)))]
                        {
                            if let Some(v) = v {
                                s.execute("effects.setParam", json!({"clip": first.0, "effect": "motion", "param": param, "value": v}))?;
                            }
                        }
                        if let Some(o) = c.opacity {
                            s.execute("effects.setParam", json!({"clip": first.0, "effect": "opacity", "param": "opacity", "value": o.clamp(0.0, 100.0)}))?;
                        }
                    }
                    if let Some(db) = c.gain_db {
                        let sound: Vec<u64> = clip_ids
                            .iter()
                            .filter(|id| s.active_sequence().is_some_and(|q| q.audio_tracks.iter().any(|tr| tr.item(**id).is_some())))
                            .map(|c| c.0)
                            .collect();
                        if !sound.is_empty() {
                            s.execute("clip.audioGain", json!({"clips": sound, "mode": "set", "db": db}))?;
                        }
                    }
                    for id in &clip_ids {
                        placed.push((*id, c.name.clone(), c.label));
                    }
                    cursor = start + dur;
                    ids.push(json!(clip_ids.iter().map(|c| c.0).collect::<Vec<_>>()));
                }
                out_tracks.insert(format!("{kind}{}", t + 1), Value::Array(ids));
            }
        }
        // names and labels, in one edit
        if placed.iter().any(|(_, n, l)| n.is_some() || l.is_some()) {
            s.edit_sequence("Name Clips", |q, _, _| {
                for (id, n, l) in &placed {
                    if let Some((_, it)) = q.find_item_mut(*id) {
                        if let Some(n) = n {
                            it.name = n.clone();
                        }
                        if let Some(l) = l {
                            it.label = *l;
                        }
                    }
                }
                Ok(())
            })?;
        }
        let mut cues = 0;
        for c in &captions {
            let mut t = json!({"format": "Subtitle"});
            for k in ["name", "language"] {
                if let Some(v) = c.get(k).and_then(Value::as_str) {
                    t[k] = json!(v);
                }
            }
            let tr = s.execute("captions.newTrack", t)?;
            let track = tr["track"].as_u64().ok_or_else(|| EngineError::Other("no caption track was made".into()))?;
            for q in c["cues"].as_array().into_iter().flatten() {
                let (a, b) = (q["start"].as_f64().unwrap_or(0.0), q["end"].as_f64().unwrap_or(0.0));
                let (a, b) = (frame(a), frame(b).max(frame(a) + rate.frame_duration()));
                s.execute(
                    "captions.add",
                    json!({"track": track, "text": q["text"], "seconds": a.seconds(), "durationSeconds": (b - a).seconds(), "split": q.get("split").and_then(Value::as_bool).unwrap_or(false)}),
                )?;
                cues += 1;
            }
            if let Some(style) = c.get("style").and_then(Value::as_object) {
                let mut st = Value::Object(style.clone());
                st["track"] = json!(track);
                s.execute("captions.setStyle", st)?;
            }
        }
        for m in &markers {
            let mut q = json!({"time": frame(m["at"].as_f64().unwrap_or(0.0)).0});
            for k in ["name", "comment", "color"] {
                if let Some(v) = m.get(k).filter(|v| !v.is_null()) {
                    q[k] = v.clone();
                }
            }
            if m.get("kind").and_then(Value::as_str) == Some("chapter") {
                s.execute("markers.addChapter", q)?;
            } else {
                q["target"] = json!("program");
                if let Some(d) = m.get("duration").and_then(Value::as_f64).filter(|d| *d > 0.0) {
                    q["durationFrames"] = json!((frame(d).0 / rate.frame_duration().0.max(1)).max(1));
                }
                s.execute("markers.add", q)?;
            }
        }
        let duration = s.active_sequence().map_or(Tick::ZERO, |q| q.duration());
        Ok(json!({
            "sequence": seq,
            "duration": duration.seconds(),
            "tracks": out_tracks,
            "captions": cues,
            "markers": markers.len(),
            "imported": cache.len(),
        }))
    })
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Assemble Sequence from a Plan",
        menu: &[],
        shortcut: None,
        params: r#"{"name":str,"width":u32?,"height":u32?,"fps":f64?,"sampleRate":u32?,"video":[[clip]] (V1, V2…),"audio":[[clip]]? (A1, A2…),"captions":[{"name":str?,"language":str?,"cues":[{"start":s,"end":s,"text":str}],"style":{captions.setStyle}?}]?,"markers":[{"at":s,"name":str,"comment":str?,"kind":"chapter|comment"?,"color":label?,"duration":s?}]?} where clip = {"media":abs path|"item":id,"in":s?,"out":s|"duration":s,"at":s? (default: after the previous clip),"name":str?,"label":label?,"audio":bool=true,"audioTrack":n?,"scale":%?,"position":[x,y]?,"anchor":[x,y]?,"opacity":%?,"gainDb":dB?}"#,
        enabled: always,
        run: assemble,
        journal: true,
    }]
}

#[cfg(test)]
#[path = "assemble_tests.rs"]
mod tests;
