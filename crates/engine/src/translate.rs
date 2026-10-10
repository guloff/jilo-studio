//! Translating a caption track into a new one (`captions.translate`), as Jilo Studio does.
//!
//! The captions go to the text engine in batches of 40 as `{"items": [{"id", "text"}]}` with a
//! prompt that asks to keep every id and the order, translate meaning (not words) in spoken style,
//! keep about the original length (it must fit the same time on screen), keep names and numbers,
//! and never merge or split items (Gemini: a response schema, temperature 0.2; OpenAI: strict JSON
//! schema). The new track takes the source's timing, speakers and style; a caption the engine
//! skipped keeps its original text. One undo step adds the track.

use filmcraft_ai::speech::language_name;
use filmcraft_project::{CaptionTrack, ClipId, TrackId};
use serde_json::{Value, json};

use crate::ai_tasks::{Outcome, TaskWork, other, run, started};
use crate::captions::track_param;
use crate::commands::{CommandSpec, bad, str_p};
use crate::{EngineError, Result, Session};

/// Captions per request: smaller batches translate better and show progress.
pub const BATCH: usize = 40;

/// The translation prompt (the rules are the same for every engine).
pub fn translation_prompt(from: Option<&str>, to: &str) -> String {
    let source = from.filter(|f| !f.is_empty()).map(|f| format!("from {} ", language_name(f))).unwrap_or_default();
    [
        format!("You are a professional subtitle translator. Translate the cues {source}into {}.", language_name(to)),
        String::new(),
        "Rules:".into(),
        "1. Return every item you received, with the SAME \"id\" values, in the same order.".into(),
        "2. Translate meaning, not words. Keep the tone of spoken speech.".into(),
        "3. Keep each translation close in length to the original — it has to fit the same".into(),
        "   time on screen. Shorten wording rather than dropping meaning.".into(),
        "4. Keep names, numbers, and technical terms accurate. Do not add explanations.".into(),
        "5. Never merge or split items, never add or remove items.".into(),
        "6. Consider the surrounding cues for context: they are one continuous speech.".into(),
        String::new(),
        "Input is JSON of the form {\"items\":[{\"id\":\"…\",\"text\":\"…\"}]}.".into(),
        "Answer with JSON of the same shape, where \"text\" is the translation.".into(),
    ]
    .join("\n")
}

pub fn translation_schema() -> Value {
    json!({"type": "object", "properties": {"items": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}, "text": {"type": "string"}}, "required": ["id", "text"]}}}, "required": ["items"]})
}

fn translate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.translate";
    let to =
        str_p(p, "to").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(cmd, "`to` (a language code such as ru, tg or en) is required"))?.to_string();
    let tid = track_param(s, p).ok_or_else(|| bad(cmd, "no such caption track"))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let source = seq.caption_track(tid).ok_or_else(|| bad(cmd, "no such caption track"))?.clone();
    if source.captions.is_empty() {
        return Err(bad(cmd, "the caption track has no captions"));
    }
    if source.language == to {
        return Err(bad(cmd, format!("the track is already in `{to}`")));
    }
    let items: Vec<Value> = source.captions.iter().map(|c| json!({"id": c.id.0.to_string(), "text": c.text})).collect();
    let from = source.language.clone();
    let inputs = json!({"track": tid.0, "to": to, "items": items});
    let (to2, from2) = (to.clone(), from.clone());
    let work: TaskWork = Box::new(move |engine, ctx, progress| {
        let mut out = serde_json::Map::new();
        let batches: Vec<&[Value]> = items.chunks(BATCH).collect();
        let schema = translation_schema();
        let prompt = translation_prompt(Some(&from2), &to2);
        for (i, b) in batches.iter().enumerate() {
            if !progress(i as f32 / batches.len() as f32, &format!("Translating {} of {}", i + 1, batches.len())) {
                return Err(filmcraft_ai::AiError::Cancelled);
            }
            let v = engine.json(ctx, &prompt, &json!({"items": b}).to_string(), Some(&schema), 0.2)?;
            for it in v.get("items").and_then(Value::as_array).into_iter().flatten() {
                if let (Some(id), Some(text)) = (it.get("id").and_then(Value::as_str), it.get("text").and_then(Value::as_str)) {
                    out.insert(id.to_string(), Value::String(text.trim().to_string()));
                }
            }
        }
        let _ = progress(1.0, "Translated");
        Ok(Value::Object(out))
    });
    let answer = match run(s, p, cmd, "Translating captions", &inputs, "the caption text", work)? {
        Outcome::Started(id) => return Ok(started(id)),
        Outcome::Ready(v) => v,
    };
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let missed = source.captions.iter().filter(|c| answer.get(c.id.0.to_string()).and_then(Value::as_str).is_none_or(str::is_empty)).count();
    if missed == source.captions.len() {
        return Err(other("the engine returned no translation"));
    }
    let lang_label = language_name(&to);
    let lang_label = lang_label.split(" (").next().unwrap_or(&to).to_string();
    let from_label = if from.is_empty() { String::new() } else { format!(" (translated from {})", language_name(&from).split(" (").next().unwrap_or(&from)) };
    let new_id = s.edit("Translate Captions", |pr, _| {
        let id = TrackId(pr.alloc_id());
        let mut t: CaptionTrack = source.clone();
        t.id = id;
        t.name = format!("{lang_label}{from_label}");
        t.language = to.clone();
        t.vtt_blocks.clear();
        for c in &mut t.captions {
            let old = c.id;
            c.id = ClipId(pr.alloc_id());
            c.cue_id = None;
            if let Some(text) = answer.get(old.0.to_string()).and_then(Value::as_str).filter(|x| !x.is_empty()) {
                c.text = text.to_string();
            }
        }
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let at = q.caption_tracks.iter().position(|x| x.id == tid).map_or(0, |i| i + 1);
        q.caption_tracks.insert(at, t);
        Ok(id)
    })?;
    Ok(json!({"track": new_id.0, "captions": source.captions.len(), "untranslated": missed, "language": to}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "captions.translate",
        label: "Translate Captions…",
        menu: &[],
        shortcut: None,
        params: r#"{"track":id|"C1"?,"to":"ru|tg|en|…","engine":"gemini|openai|anthropic"?,"model":str?,"confirm":bool?,"background":bool?}"#,
        enabled: crate::commands::has_seq,
        run: translate,
        journal: true,
    }]
}
