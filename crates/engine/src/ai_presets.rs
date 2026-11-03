//! Presets for the AI features (the AI Tools panel): a feature with settings saved under a name,
//! run again with one click. Some come with the app (captions for Shorts, pauses cut tight, a mix
//! for Reels…); the user saves their own from the settings a feature last ran with.
//!
//! | Command | |
//! |---|---|
//! | `ai.presets.list` | `{command?}`: the presets (built-in first), and the settings each feature last ran with (`last`) |
//! | `ai.presets.save` | `{command, name, params?}`: a preset of the user's; `params` default to the feature's last run |
//! | `ai.presets.delete` | `{id}`: one of the user's presets |
//! | `ai.presets.run` | `{id, confirm?}`: the preset's feature with its settings (`confirm` as for cloud features) |
//!
//! Only the settings are kept: what a run worked on (clips, items, a track, a time, a folder) and
//! one-time switches (`confirm`, `background`) are left out, so a preset applies to whatever is
//! open when it runs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// A preset of the user's (`Preferences::ai.presets`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiPreset {
    pub id: String,
    pub command: String,
    pub name: String,
    pub params: Value,
}

/// The features presets are offered for (the AI Tools panel's engine commands).
pub const FEATURES: [&str; 15] = [
    "transcript.generate",
    "transcript.createCaptions",
    "captions.translate",
    "transcript.removePauses",
    "audio.removeSilences",
    "transcript.removeFillers",
    "highlights.find",
    "clip.autoZoom",
    "clip.reframeToFace",
    "broll.auto",
    "stickers.byWords",
    "audio.autoMix",
    "youtube.generate",
    "export.allFormats",
    "ai.generateImage",
];

/// Parameters a preset never keeps: what one run worked on, and one-time switches.
const TRANSIENT: [&str; 14] =
    ["confirm", "background", "wait", "clips", "items", "item", "clip", "track", "time", "dir", "path", "place", "replaceChapters", "start"];

/// The presets that come with the app: (id, command, name, params as JSON).
const BUILTIN: [(&str, &str, &str, &str); 22] = [
    ("builtin:transcribe-local-ru", "transcript.generate", "On this computer, Russian", r#"{"engine":"whisper","language":"ru"}"#),
    ("builtin:transcribe-local-auto", "transcript.generate", "On this computer, any language", r#"{"engine":"whisper","language":"auto"}"#),
    ("builtin:transcribe-speakers", "transcript.generate", "With speakers", r#"{"diarize":true}"#),
    ("builtin:captions-shorts", "transcript.createCaptions", "Shorts and Reels: one short line", r#"{"lines":1,"maxChars":18}"#),
    ("builtin:captions-youtube", "transcript.createCaptions", "YouTube: two lines", r#"{"lines":2,"maxChars":42}"#),
    ("builtin:translate-en", "captions.translate", "To English", r#"{"to":"en"}"#),
    ("builtin:translate-ru", "captions.translate", "To Russian", r#"{"to":"ru"}"#),
    ("builtin:translate-tg", "captions.translate", "To Tajik", r#"{"to":"tg"}"#),
    ("builtin:pauses-tight", "transcript.removePauses", "Tight: pauses over 0.4 s", r#"{"minSeconds":0.4,"keepSeconds":0.1}"#),
    ("builtin:pauses-gentle", "transcript.removePauses", "Gentle: pauses over 1 s", r#"{"minSeconds":1.0,"keepSeconds":0.25}"#),
    ("builtin:silences-default", "audio.removeSilences", "Silences under −35 dB", r#"{"thresholdDb":-35,"minSeconds":0.6,"keepSeconds":0.15}"#),
    ("builtin:fillers-ru", "transcript.removeFillers", "Russian fillers", r#"{"language":"ru"}"#),
    ("builtin:fillers-en", "transcript.removeFillers", "English fillers", r#"{"language":"en"}"#),
    ("builtin:highlights-viral", "highlights.find", "3 viral moments up to 60 s", r#"{"mode":"viral","count":3,"maxSeconds":60}"#),
    ("builtin:zoom-subtle", "clip.autoZoom", "Subtle and smooth", r#"{"intensity":1.08,"style":"smooth"}"#),
    ("builtin:zoom-punchy", "clip.autoZoom", "Punchy cuts", r#"{"intensity":1.25,"style":"cut"}"#),
    ("builtin:emoji-few", "stickers.byWords", "A few emoji", r#"{"maxCount":4,"minGap":10}"#),
    ("builtin:mix-youtube", "audio.autoMix", "YouTube", r#"{"platform":"youtube"}"#),
    ("builtin:mix-social", "audio.autoMix", "Reels, TikTok and Shorts", r#"{"platform":"social"}"#),
    ("builtin:mix-podcast", "audio.autoMix", "Podcast", r#"{"platform":"podcast"}"#),
    ("builtin:youtube-ru", "youtube.generate", "In Russian", r#"{"language":"ru"}"#),
    ("builtin:formats-vertical", "export.allFormats", "Vertical and square", r#"{"aspects":["9:16","1:1"]}"#),
];

/// The settings of `params` a preset keeps.
pub fn settings_of(params: &Value) -> Value {
    let mut out = Map::new();
    if let Some(o) = params.as_object() {
        for (k, v) in o {
            if !TRANSIENT.contains(&k.as_str()) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// Called for every command that succeeded: a feature's settings are remembered for "Save as
/// Preset".
pub(crate) fn remember(s: &mut Session, id: &str, params: &Value) {
    if FEATURES.contains(&id) {
        s.ai_last_params.insert(id.to_string(), settings_of(params));
    }
}

fn builtins() -> Vec<AiPreset> {
    BUILTIN
        .iter()
        .map(|(id, command, name, params)| AiPreset {
            id: id.to_string(),
            command: command.to_string(),
            name: name.to_string(),
            params: serde_json::from_str(params).unwrap_or_else(|_| json!({})),
        })
        .collect()
}

/// Every preset, built-in first, then the user's in the order saved.
pub fn all(s: &Session) -> Vec<AiPreset> {
    let mut v = builtins();
    v.extend(s.prefs.ai.presets.iter().cloned());
    v
}

fn find(s: &Session, id: &str) -> Option<AiPreset> {
    all(s).into_iter().find(|p| p.id == id)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let only = str_p(p, "command");
    let presets: Vec<Value> = all(s)
        .into_iter()
        .filter(|x| only.is_none_or(|c| c == x.command))
        .map(|x| json!({"id": x.id, "command": x.command, "name": x.name, "params": x.params, "builtin": x.id.starts_with("builtin:")}))
        .collect();
    let last: BTreeMap<&String, &Value> = s.ai_last_params.iter().filter(|(k, _)| only.is_none_or(|c| c == k.as_str())).collect();
    Ok(json!({"presets": presets, "last": last}))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.presets.save";
    let command = str_p(p, "command").ok_or_else(|| bad(cmd, "`command`: the feature"))?.to_string();
    if !FEATURES.contains(&command.as_str()) {
        return Err(bad(cmd, format!("`{command}` has no presets")));
    }
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name`"))?;
    if name.chars().count() > 80 {
        return Err(bad(cmd, "the name is longer than 80 characters"));
    }
    let params = match p.get("params") {
        Some(v) if v.is_object() => settings_of(v),
        Some(_) => return Err(bad(cmd, "`params` is an object")),
        None => s.ai_last_params.get(&command).cloned().ok_or_else(|| bad(cmd, "run the feature once first: its settings become the preset"))?,
    };
    if params.to_string().len() > 16 * 1024 {
        return Err(bad(cmd, "the settings are too large for a preset"));
    }
    let mut prefs = s.prefs.clone();
    if prefs.ai.presets.len() >= 200 {
        return Err(bad(cmd, "200 presets at most: delete some first"));
    }
    let n = prefs.ai.presets.iter().filter_map(|x| x.id.strip_prefix("user:").and_then(|n| n.parse::<u64>().ok())).max().unwrap_or(0) + 1;
    let id = format!("user:{n}");
    prefs.ai.presets.push(AiPreset { id: id.clone(), command: command.clone(), name: name.to_string(), params: params.clone() });
    s.set_prefs(prefs).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"id": id, "command": command, "name": name, "params": params}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.presets.delete";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "`id`"))?;
    if id.starts_with("builtin:") {
        return Err(bad(cmd, "a built-in preset cannot be deleted"));
    }
    let mut prefs = s.prefs.clone();
    let before = prefs.ai.presets.len();
    prefs.ai.presets.retain(|x| x.id != id);
    if prefs.ai.presets.len() == before {
        return Err(bad(cmd, format!("no preset `{id}`")));
    }
    s.set_prefs(prefs).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(Value::Null)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.presets.run";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "`id`"))?;
    let preset = find(s, id).ok_or_else(|| bad(cmd, format!("no preset `{id}`")))?;
    let mut params = preset.params.clone();
    for k in ["confirm", "background"] {
        if let (Some(v), Some(o)) = (p.get(k), params.as_object_mut()) {
            o.insert(k.to_string(), v.clone());
        }
    }
    s.execute(&preset.command, params)
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>, journal: bool| CommandSpec {
        id,
        label,
        menu: &[],
        shortcut: None,
        params,
        enabled: always,
        run,
        journal,
    };
    vec![
        spec("ai.presets.list", "AI Presets", r#"{"command":str?}"#, list, false),
        spec("ai.presets.save", "Save AI Preset", r#"{"command":str,"name":str,"params":{}?}"#, save, false),
        spec("ai.presets.delete", "Delete AI Preset", r#"{"id":str}"#, delete, false),
        spec("ai.presets.run", "Run AI Preset", r#"{"id":str,"confirm":bool?,"background":bool?}"#, run, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_remembered_saved_as_a_preset_and_run_again() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        // the built-in presets are valid JSON and name a feature with presets
        for b in builtins() {
            assert!(b.params.is_object() && FEATURES.contains(&b.command.as_str()), "{}", b.id);
        }
        let r = s.execute("ai.presets.list", json!({"command": "audio.autoMix"})).unwrap();
        assert_eq!(r["presets"].as_array().unwrap().len(), 3, "{r}");
        // nothing to save before a run
        assert!(s.execute("ai.presets.save", json!({"command": "audio.autoMix", "name": "Mine"})).is_err());
        s.execute("audio.autoMix", json!({"platform": "podcast", "duck": false, "confirm": true})).unwrap();
        let last = s.execute("ai.presets.list", json!({"command": "audio.autoMix"})).unwrap()["last"]["audio.autoMix"].clone();
        assert_eq!(last, json!({"platform": "podcast", "duck": false}), "settings only, no one-time switches");
        let saved = s.execute("ai.presets.save", json!({"command": "audio.autoMix", "name": "Podcast, no ducking"})).unwrap();
        let id = saved["id"].as_str().unwrap().to_string();
        assert_eq!(s.prefs.ai.presets.len(), 1);
        // run again: the same command with the same settings, one undo step of its own
        let undo = s.history.undo.len();
        s.execute("ai.presets.run", json!({"id": id})).unwrap();
        assert!(s.history.undo.len() > undo);
        assert_eq!(s.journal.last().map(|(c, p)| (c.as_str(), p.clone())), Some(("audio.autoMix", json!({"platform": "podcast", "duck": false}))));
        // a built-in one runs too, and cannot be deleted; the user's can
        s.execute("ai.presets.run", json!({"id": "builtin:mix-social"})).unwrap();
        assert!(s.execute("ai.presets.delete", json!({"id": "builtin:mix-social"})).is_err());
        s.execute("ai.presets.delete", json!({"id": id})).unwrap();
        assert!(s.prefs.ai.presets.is_empty());
        // hostile input
        for bad in [
            json!({"command": "file.save", "name": "x", "params": {}}),
            json!({"command": "audio.autoMix", "name": " ", "params": {}}),
            json!({"command": "audio.autoMix", "name": "x", "params": [1]}),
            json!({"command": "audio.autoMix", "name": "é".repeat(81), "params": {}}),
        ] {
            assert!(s.execute("ai.presets.save", bad.clone()).is_err(), "{bad}");
        }
        assert!(s.execute("ai.presets.run", json!({"id": "user:999"})).is_err());
    }

    #[test]
    fn what_a_run_worked_on_is_not_kept() {
        let p = json!({"clips": [1, 2], "track": "V2", "dir": "/tmp", "confirm": true, "background": true, "intensity": 1.2, "style": "cut"});
        assert_eq!(settings_of(&p), json!({"intensity": 1.2, "style": "cut"}));
    }
}
