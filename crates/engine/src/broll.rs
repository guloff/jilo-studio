//! Automatic B-roll (`broll.auto`): footage of the project that fits what is being said goes on
//! the track above the speech, at the phrase that mentions it.
//!
//! What a clip shows comes from its name, its description and keywords (Metadata), and — where
//! Apple Vision is available — scene labels of a few of its frames (`broll.labels`, kept in the
//! item's metadata as "Vision Labels"). Matching is by words on this computer (`keywords`: shared
//! words, Russian and English stems), or by the text engine (`ai`: the phrases and the clips'
//! descriptions go to the cloud, after consent; it understands any language). With `generate`,
//! phrases left without footage get a picture drawn by the image engine, placed when it is ready.
//!
//! | Command | |
//! |---|---|
//! | `broll.auto` | `{method?: "keywords"\|"ai", track?, maxClips?, seconds?, minGap?, generate?, items?}` |
//! | `broll.labels` | `{items?}`: Vision labels of the footage (stored, one undo step) |
//!
//! The footage is the items selected in the Project panel, else every video or still in the
//! project that the sequence does not use yet. Each is used once. One undo step for the footage.

use std::sync::Arc;

use serde_json::{Value, json};

use filmcraft_project::{ItemId, ItemKind, TrackId};
use filmcraft_time::Tick;

use crate::ai_tasks::{Outcome, TaskWork, other, run, started};
use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, str_p, u64_p};
use crate::highlights::{Cue, cues_from_words};
use crate::{EngineError, Result, Session};

/// Scene labels of an 8-bit RGBA picture (`w`×`h`): (label, confidence 0–1).
pub type ImageClassifier = Arc<dyn Fn(&[u8], usize, usize) -> std::result::Result<Vec<(String, f32)>, String> + Send + Sync>;

/// Metadata key of an item's Vision labels.
pub const LABELS_KEY: &str = "Vision Labels";

/// Words that say nothing about a picture.
const STOP: [&str; 44] = [
    "the", "and", "with", "this", "that", "from", "have", "what", "your", "about", "there", "here", "they", "were", "will", "would", "just", "like", "into",
    "это", "как", "что", "для", "или", "так", "вот", "все", "всё", "его", "она", "они", "оно", "мне", "нас", "вас", "был", "была", "было", "есть", "тут",
    "clip", "video", "final", "copy",
];

/// Lower-case words of a text, without stop words, numbers and file noise.
pub fn tokens(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit()) && !STOP.contains(&w.as_str()))
        .filter(|w| !matches!(w.as_str(), "mov" | "mp4" | "m4v" | "jpg" | "jpeg" | "png" | "heic" | "img" | "dsc" | "dji" | "gopr" | "mts"))
        .collect();
    v.dedup();
    v
}

/// Whether two words are the same word: equal, or (both long) sharing their first five letters —
/// a rough stem for Russian and English endings ("горы" / "горах", "mountain" / "mountains").
pub fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (ca, cb) = (a.chars().count(), b.chars().count());
    if ca < 5 || cb < 5 {
        // short words: one may be the other plus a short ending
        let (short, long) = if ca <= cb { (a, b) } else { (b, a) };
        return short.chars().count() >= 4 && long.starts_with(short) && long.chars().count() - short.chars().count() <= 2;
    }
    a.chars().take(5).eq(b.chars().take(5))
}

/// How well a phrase fits a clip: the number of the phrase's words the clip's words match.
pub fn score(phrase: &[String], clip: &[String]) -> usize {
    phrase.iter().filter(|w| clip.iter().any(|c| same_word(w, c))).count()
}

/// A clip the B-roll can come from.
#[derive(Clone, Debug)]
struct Footage {
    item: ItemId,
    name: String,
    /// Name, description, keywords and labels.
    words: Vec<String>,
    labels: Vec<String>,
    /// Media length (None: a still, any length).
    duration: Option<Tick>,
}

fn is_footage(s: &Session, id: ItemId) -> bool {
    s.project.item(id).is_some_and(|i| matches!(i.kind, ItemKind::Media(_)) && i.has_video())
}

/// The pool: the Project panel's selection, else every video or still the sequence does not use.
fn pool(s: &Session, p: &Value) -> Vec<ItemId> {
    if let Some(a) = p.get("items").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).map(ItemId).filter(|i| is_footage(s, *i)).collect();
    }
    let chosen: Vec<ItemId> = s.state.project_selection.iter().copied().filter(|i| is_footage(s, *i)).collect();
    if !chosen.is_empty() {
        return chosen;
    }
    let used: std::collections::HashSet<ItemId> =
        s.active_sequence().map(|q| q.all_tracks().flat_map(|t| t.items.iter()).map(|i| i.item).collect()).unwrap_or_default();
    let mut v: Vec<ItemId> = s.project.items.keys().copied().filter(|i| !used.contains(i) && is_footage(s, *i)).collect();
    v.sort();
    v
}

/// Vision labels of an item's frames (a still: itself; a video: three frames), most confident first.
fn classify(s: &Session, item: ItemId, classifier: &ImageClassifier) -> Vec<String> {
    let Some(src) = s.source(item) else { return Vec::new() };
    let info = src.info();
    let Some(v) = info.video.as_ref() else { return Vec::new() };
    let scale = (480.0 / v.width.max(1) as f32).min(1.0);
    let dur = s.project.item(item).map(|i| i.duration()).unwrap_or(Tick::ZERO);
    let times: Vec<Tick> = if dur > Tick::ZERO { [0.2, 0.5, 0.8].iter().map(|f| Tick((dur.0 as f64 * f) as i64)).collect() } else { vec![Tick::ZERO] };
    let mut found: Vec<(String, f32)> = Vec::new();
    for t in times {
        let Ok(f) = src.video_frame(filmcraft_media::FrameRequest { time: t, scale }) else { continue };
        let Ok(labels) = classifier(&f.to_rgba8(), f.width as usize, f.height as usize) else { continue };
        for (l, c) in labels {
            let l = l.replace('_', " ");
            match found.iter_mut().find(|x| x.0 == l) {
                Some(x) => x.1 = x.1.max(c),
                None => found.push((l, c)),
            }
        }
    }
    found.sort_by(|a, b| b.1.total_cmp(&a.1));
    found.into_iter().filter(|x| x.1 >= 0.2).take(8).map(|x| x.0).collect()
}

/// The footage with its words; labels missing from the metadata are found now when Vision is
/// there (returned for storing).
fn footage(s: &Session, ids: &[ItemId]) -> (Vec<Footage>, Vec<(ItemId, Vec<String>)>) {
    let mut fresh = Vec::new();
    let out = ids
        .iter()
        .filter_map(|id| {
            let it = s.project.item(*id)?;
            let mut labels: Vec<String> =
                it.metadata.get(LABELS_KEY).map(|l| l.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            if !it.metadata.contains_key(LABELS_KEY)
                && let Some(c) = &s.image_classifier
            {
                labels = classify(s, *id, c);
                fresh.push((*id, labels.clone()));
            }
            let stem = it.name.rsplit_once('.').map_or(it.name.as_str(), |x| x.0);
            let mut text = format!("{stem} {}", labels.join(" "));
            for k in ["Description", "Keywords", "Log Note", "Scene", "Shot"] {
                if let Some(v) = it.metadata.get(k) {
                    text.push(' ');
                    text.push_str(v);
                }
            }
            let still =
                ["png", "jpg", "jpeg", "heic", "tif", "tiff", "webp", "gif", "bmp"].iter().any(|e| it.name.to_ascii_lowercase().ends_with(&format!(".{e}")));
            let duration = (!still && it.duration() > Tick::ZERO).then(|| it.duration());
            Some(Footage { item: *id, name: it.name.clone(), words: tokens(&text), labels, duration })
        })
        .collect();
    (out, fresh)
}

fn store_labels(s: &mut Session, fresh: Vec<(ItemId, Vec<String>)>) -> Result<()> {
    if fresh.is_empty() {
        return Ok(());
    }
    s.edit("Label Footage", move |p, _| {
        for (id, labels) in fresh {
            if let Some(it) = p.items.get_mut(&id) {
                it.metadata.insert(LABELS_KEY.to_string(), labels.join(", "));
            }
        }
        Ok(())
    })
}

/// (cue index, item) picks by shared words: each cue its best clip, each clip once, the
/// strongest matches first.
pub fn keyword_picks(cues: &[Vec<String>], clips: &[Vec<String>]) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize, usize)> = Vec::new();
    for (ci, cw) in cues.iter().enumerate() {
        for (k, kw) in clips.iter().enumerate() {
            let n = score(cw, kw);
            if n > 0 {
                all.push((n, ci, k));
            }
        }
    }
    all.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut picks: Vec<(usize, usize)> = Vec::new();
    for (_, ci, k) in all {
        if !picks.iter().any(|p| p.0 == ci || p.1 == k) {
            picks.push((ci, k));
        }
    }
    picks.sort();
    picks
}

/// Placement of picks: (start, length) in sequence seconds for each kept pick, at least `min_gap`
/// apart, `seconds` long (shorter clips, shorter), at most `max` of them.
pub fn place_plan(picks: &[(usize, usize)], cues: &[Cue], lengths: &[Option<f64>], seconds: f64, min_gap: f64, max: usize) -> Vec<(usize, usize, f64, f64)> {
    let mut out: Vec<(usize, usize, f64, f64)> = Vec::new();
    let mut free_from = f64::MIN;
    for &(ci, k) in picks {
        let Some(c) = cues.get(ci) else { continue };
        if out.len() >= max || c.start < free_from {
            continue;
        }
        let len = seconds.min(lengths.get(k).copied().flatten().unwrap_or(f64::MAX)).max(0.5);
        out.push((ci, k, c.start, len));
        free_from = c.start + len + min_gap;
    }
    out
}

/// The track B-roll goes on: `track` ("V2", a number or an id), else the one above V1 (added when
/// there is none).
fn broll_track(s: &mut Session, p: &Value) -> Result<TrackId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    if let Some(t) = str_p(p, "track") {
        let n = t.trim_start_matches(['V', 'v']).parse::<usize>().ok();
        return n.and_then(|n| q.video_tracks.get(n.wrapping_sub(1))).map(|t| t.id).ok_or_else(|| bad("broll.auto", format!("no video track `{t}`")));
    }
    if let Some(t) = u64_p(p, "track") {
        return q.video_tracks.iter().find(|x| x.id.0 == t).map(|x| x.id).ok_or_else(|| bad("broll.auto", format!("no video track {t}")));
    }
    if let Some(t) = q.video_tracks.get(1) {
        return Ok(t.id);
    }
    let r = s.execute("sequence.addTracks", json!({"video": 1}))?;
    r["video"][0].as_u64().map(TrackId).ok_or_else(|| EngineError::Other("no track was added".into()))
}

fn prompt(language: &str) -> String {
    format!(
        "You are a video editor choosing B-roll. You get the numbered phrases of a talk and a list of clips with what they show. \
         For phrases where a clip clearly illustrates what is said, pick that clip. Use each clip at most once; leave phrases without a fitting clip out. \
         Prefer concrete things (places, objects, actions) over abstract talk. The talk is in {language}. Answer with JSON only."
    )
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"picks": {"type": "array", "items": {"type": "object", "properties": {
            "phrase": {"type": "integer"}, "clip": {"type": "integer"}, "why": {"type": "string"}
        }, "required": ["phrase", "clip", "why"]}}},
        "required": ["picks"]
    })
}

/// Pending generated pictures: (the `ai.generateImage` parameters, start and length in seconds, track).
pub type Pending = Vec<(Value, f64, f64, TrackId, u64)>;

fn auto(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "broll.auto";
    let words = crate::transcript::sequence_words(s);
    let cues = cues_from_words(&words);
    if cues.is_empty() {
        return Err(bad(cmd, "the sequence has no transcript (Transcribe first)"));
    }
    let method = str_p(p, "method").unwrap_or("keywords");
    if !matches!(method, "keywords" | "ai") {
        return Err(bad(cmd, format!("`method`: keywords or ai, not {method}")));
    }
    let seconds = f64_p(p, "seconds").unwrap_or(3.0).clamp(1.0, 15.0);
    let min_gap = f64_p(p, "minGap").unwrap_or(4.0).clamp(0.0, 60.0);
    let max = u64_p(p, "maxClips").unwrap_or(12).clamp(1, 100) as usize;
    let ids = pool(s, p);
    let (clips, fresh) = footage(s, &ids);
    let cue_words: Vec<Vec<String>> = cues.iter().map(|c| tokens(&c.text)).collect();
    let picks: Vec<(usize, usize)> = match method {
        "ai" if !clips.is_empty() => {
            let spoken = words.first().and_then(|w| s.project.transcripts.get(&w.item)).map(|t| t.language.clone()).unwrap_or_else(|| "en".into());
            let language = filmcraft_ai::speech::language_name(&spoken).split(" (").next().unwrap_or("English").to_string();
            let phrases: String = cues.iter().enumerate().map(|(i, c)| format!("{i}. {}\n", c.text)).collect();
            let list: String = clips
                .iter()
                .enumerate()
                .map(|(k, c)| format!("{k}. {} — {}\n", c.name, if c.labels.is_empty() { "-".to_string() } else { c.labels.join(", ") }))
                .collect();
            let user = format!("Phrases:\n{phrases}\nClips:\n{list}");
            let inputs = json!({"user": user});
            let system = prompt(&language);
            let work: TaskWork = Box::new(move |engine, ctx, progress| {
                if !progress(0.1, "Reading the talk") {
                    return Err(filmcraft_ai::AiError::Cancelled);
                }
                let v = engine.json(ctx, &system, &user, Some(&schema()), 0.2)?;
                let _ = progress(1.0, "B-roll chosen");
                Ok(v)
            });
            let answer = match run(s, p, cmd, "Choosing B-roll", &inputs, "the phrases and the clips' descriptions", work)? {
                Outcome::Started(id) => {
                    store_labels(s, fresh)?;
                    return Ok(started(id));
                }
                Outcome::Ready(v) => v,
            };
            let raw = answer.get("picks").and_then(Value::as_array).ok_or_else(|| other("the engine did not answer with a list of picks; try again"))?;
            let mut v: Vec<(usize, usize)> = Vec::new();
            for x in raw {
                let (Some(ci), Some(k)) = (x["phrase"].as_u64().map(|v| v as usize), x["clip"].as_u64().map(|v| v as usize)) else { continue };
                if ci < cues.len() && k < clips.len() && !v.iter().any(|p| p.0 == ci || p.1 == k) {
                    v.push((ci, k));
                }
            }
            v.sort();
            v
        }
        _ => keyword_picks(&cue_words, &clips.iter().map(|c| c.words.clone()).collect::<Vec<_>>()),
    };
    let lengths: Vec<Option<f64>> = clips.iter().map(|c| c.duration.map(Tick::seconds)).collect();
    let plan = place_plan(&picks, &cues, &lengths, seconds, min_gap, max);
    let generate = bool_p(p, "generate").unwrap_or(false);
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let r = (|| -> Result<(Vec<Value>, TrackId)> {
        store_labels(s, fresh)?;
        let track = broll_track(s, p)?;
        let rate = s.active_sequence().ok_or(EngineError::NoSequence)?.settings.frame_rate;
        let mut placed = Vec::new();
        for &(ci, k, start, len) in &plan {
            let c = &clips[k];
            let at = rate.snap_nearest(Tick::from_seconds_f64(start));
            let dur = rate.snap_nearest(Tick::from_seconds_f64(len));
            let busy = s.active_sequence().and_then(|q| q.track(track)).is_some_and(|t| t.items.iter().any(|i| i.start < at + dur && at < i.end()));
            if busy {
                continue;
            }
            // a long take starts a little in, past its first moments
            let source_in = match c.duration {
                Some(d) if d.seconds() > len + 2.0 => Tick::from_seconds_f64(1.0),
                _ => Tick::ZERO,
            };
            let r = s.execute(
                "timeline.place",
                json!({"item": c.item.0, "track": track.0, "time": at.0, "duration": dur.0, "sourceIn": source_in.0, "audio": false}),
            )?;
            if let Some(clip) = r["clips"][0].as_u64() {
                let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
                // footage of another shape fills the frame
                if q.find_item(filmcraft_project::ClipId(clip)).is_some_and(|(_, it)| !it.scale_to_frame) {
                    s.execute("clip.fillFrame", json!({"clips": [clip]}))?;
                }
                placed.push(json!({"clip": clip, "item": c.item.0, "name": c.name, "phrase": cues[ci].text, "seconds": start}));
            }
        }
        Ok((placed, track))
    })();
    let (placed, track) = match r {
        Ok(v) => {
            crate::recipes::collapse_undo(s, before, "Auto B-roll");
            v
        }
        Err(e) => {
            s.revert_to(mark);
            return Err(e);
        }
    };
    // pictures for the phrases left without footage, drawn in the background
    let mut drawing = 0;
    if generate {
        let taken: Vec<(f64, f64)> = plan.iter().map(|x| (x.2, x.2 + x.3)).collect();
        let mut free_from = f64::MIN;
        let mut want: Vec<&Cue> = Vec::new();
        for c in &cues {
            let clear = taken.iter().all(|(a, b)| c.start + seconds + min_gap <= *a || c.start >= *b + min_gap);
            if clear && c.start >= free_from && c.text.chars().count() >= 20 {
                want.push(c);
                free_from = c.start + seconds + min_gap;
            }
        }
        let n = u64_p(p, "generateCount").unwrap_or(3).clamp(1, 10) as usize;
        for c in want.into_iter().take(n) {
            let mut q = json!({"prompt": format!("A photograph that illustrates: {}. No text, no captions.", c.text), "background": true, "place": false});
            for k in ["confirm", "engine"] {
                if let Some(v) = p.get(k) {
                    q[k] = v.clone();
                }
            }
            let r = s.execute("ai.generateImage", q.clone())?;
            if let Some(job) = r["job"].as_u64() {
                q.as_object_mut().map(|m| m.remove("background"));
                s.broll_pending.push((q, c.start, seconds, track, job));
                drawing += 1;
            }
        }
    }
    Ok(json!({"placed": placed, "phrases": cues.len(), "footage": clips.len(), "drawing": drawing, "method": method}))
}

/// Place the pictures drawn for B-roll as they become ready (call regularly).
pub fn pump(s: &mut Session) {
    if s.broll_pending.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut s.broll_pending);
    for (q, start, len, track, job) in pending {
        match s.jobs.iter().find(|j| j.id == job).map(|j| j.to_json()) {
            Some(v) if v["finished"] != true => s.broll_pending.push((q, start, len, track, job)),
            Some(v) if v["result"]["error"].is_string() => {
                s.log.push(crate::panels::Level::Warning, "broll", format!("B-roll picture: {}", v["result"]["error"].as_str().unwrap_or_default()));
            }
            None => {}
            Some(_) => {
                // the picture is ready: the same request now imports it
                let before = std::sync::Arc::as_ptr(&s.project) as usize;
                let r = s.execute("ai.generateImage", q).and_then(|r| {
                    let item = r["item"].as_u64().ok_or_else(|| EngineError::Other("no picture".into()))?;
                    let rate = s.active_sequence().ok_or(EngineError::NoSequence)?.settings.frame_rate;
                    let at = rate.snap_nearest(Tick::from_seconds_f64(start));
                    let dur = rate.snap_nearest(Tick::from_seconds_f64(len));
                    let r = s.execute("timeline.place", json!({"item": item, "track": track.0, "time": at.0, "duration": dur.0, "audio": false}))?;
                    if let Some(c) = r["clips"][0].as_u64() {
                        s.execute("clip.fillFrame", json!({"clips": [c]}))?;
                    }
                    Ok(())
                });
                match r {
                    Ok(()) => crate::recipes::collapse_undo(s, before, "B-roll Picture"),
                    Err(e) => s.log.push(crate::panels::Level::Warning, "broll", format!("B-roll picture: {e}")),
                }
            }
        }
    }
}

fn labels(s: &mut Session, p: &Value) -> Result<Value> {
    let Some(c) = s.image_classifier.clone() else {
        return Err(EngineError::Other("scene labels need Apple Vision (macOS)".into()));
    };
    let ids: Vec<ItemId> = match p.get("items").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(ItemId).filter(|i| is_footage(s, *i)).collect(),
        None => s.state.project_selection.iter().copied().filter(|i| is_footage(s, *i)).collect(),
    };
    if ids.is_empty() {
        return Err(bad("broll.labels", "select video clips or stills in the Project panel"));
    }
    let found: Vec<(ItemId, Vec<String>)> = ids.iter().map(|i| (*i, classify(s, *i, &c))).collect();
    let out = json!(found.iter().map(|(i, l)| json!({"item": i.0, "labels": l})).collect::<Vec<_>>());
    store_labels(s, found)?;
    Ok(out)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "broll.auto",
            label: "Auto B-roll…",
            menu: crate::ai_menu::EDITING,
            shortcut: None,
            params: r#"{"method":"keywords|ai"="keywords","track":"V2"|id?,"maxClips":n=12,"seconds":f=3,"minGap":f=4,"items":[id]?,"generate":bool?,"generateCount":n=3,"engine":str?,"confirm":bool?,"background":bool?}"#,
            enabled: has_seq,
            run: auto,
            journal: true,
        },
        CommandSpec {
            id: "broll.labels",
            label: "Label Footage with Vision",
            menu: &[],
            shortcut: None,
            params: r#"{"items":[id]?}"#,
            enabled: crate::commands::always,
            run: labels,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_across_endings_and_languages_stay_apart() {
        assert_eq!(tokens("Mountains_at_dawn 02.MOV"), ["mountains", "dawn"]);
        assert_eq!(tokens("Мы поехали в горы, и это было круто"), ["поехали", "горы", "круто"]);
        assert!(same_word("mountains", "mountain"));
        assert!(!same_word("горах", "горы"), "four letters are too short a stem");
        assert!(same_word("горы", "горы"));
        assert!(same_word("город", "городе"));
        assert!(same_word("beach", "beaches"));
        assert!(!same_word("cat", "catalog"));
        assert_eq!(score(&tokens("we walked on the beach at sunset"), &tokens("Beaches sunset drone")), 2);
    }

    #[test]
    fn each_phrase_gets_its_best_clip_and_each_clip_is_used_once() {
        let cues = vec![tokens("the beach and the sea"), tokens("then the city at night"), tokens("the sea again")];
        let clips = vec![tokens("city lights night"), tokens("sea beach waves"), tokens("forest")];
        assert_eq!(keyword_picks(&cues, &clips), vec![(0, 1), (1, 0)]);
        let cue = |a: f64, b: f64| Cue { start: a, end: b, text: String::new() };
        let c = vec![cue(0.0, 2.0), cue(3.0, 5.0), cue(10.0, 12.0)];
        // the second pick is too close to the first; a clip shorter than the length is shorter
        assert_eq!(place_plan(&[(0, 0), (1, 1), (2, 2)], &c, &[None, None, Some(1.5)], 3.0, 4.0, 10), vec![(0, 0, 0.0, 3.0), (2, 2, 10.0, 1.5)]);
        assert_eq!(place_plan(&[(0, 0), (2, 1)], &c, &[None, None], 3.0, 0.0, 1).len(), 1, "at most max");
    }

    #[test]
    fn footage_named_after_what_is_said_goes_above_the_phrase() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let a1 = s.active_sequence().unwrap().audio_tracks[0].items[0].clone();
        let at = |x: f64| a1.source_in + Tick::from_seconds_f64(x);
        let w = |t: &str, a: f64, b: f64| json!({"text": t, "start": at(a).0, "end": at(b).0});
        let words = json!([
            w("We", 0.0, 0.2),
            w("watched", 0.2, 0.5),
            w("the", 0.5, 0.6),
            w("ocean", 0.6, 1.0),
            w("sunset.", 1.0, 1.5),
            w("Later", 2.5, 2.8),
            w("a", 2.8, 2.9),
            w("drive", 2.9, 3.2),
            w("at", 3.2, 3.3),
            w("night.", 3.3, 3.8),
        ]);
        s.execute("transcript.set", json!({"item": a1.item.0, "transcript": {"language": "en", "speakers": [], "words": words}})).unwrap();
        let find = |s: &Session, name: &str| s.project.items.iter().find(|(_, i)| i.name.starts_with(name)).map(|(id, _)| id.0).unwrap();
        let (ocean, city) = (find(&s, "Ocean_Sunset"), find(&s, "City_Night"));
        let r = s.execute("sequence.addTracks", json!({"video": 1})).unwrap();
        let track = r["video"][0].as_u64().unwrap();
        let undo = s.history.undo.len();
        let r = s.execute("broll.auto", json!({"items": [city, ocean], "track": track, "seconds": 2.0, "minGap": 0.5})).unwrap();
        let placed = r["placed"].as_array().unwrap();
        assert_eq!(placed.len(), 2, "{r}");
        assert_eq!(placed[0]["item"], ocean);
        assert_eq!(placed[1]["item"], city);
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
        let q = s.active_sequence().unwrap();
        let t = q.video_tracks.iter().find(|t| t.id.0 == track).unwrap();
        let rate = q.settings.frame_rate;
        let starts: Vec<Tick> = t.items.iter().map(|i| i.start).collect();
        let phrase = |x: f64| rate.snap_nearest(a1.start + Tick::from_seconds_f64(x));
        assert_eq!(starts, [phrase(0.0), phrase(2.5)]);
        assert!(t.items.iter().all(|i| i.duration == rate.snap_nearest(Tick::from_seconds_f64(2.0))));
        assert!(s.execute("broll.auto", json!({"method": "nope"})).is_err());
    }
}
