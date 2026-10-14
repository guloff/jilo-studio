//! The AI assistant (`assistant.plan`): a request in the user's words ("remove the pauses and add
//! yellow karaoke captions") becomes a short plan of editor commands. The text engine of Settings ▸
//! AI gets the request, a summary of the open project (sequence, tracks, captions, transcript,
//! selection — no media and no transcript text) and the commands it may use with their parameters;
//! it answers with a reply and the steps. Steps naming a command that does not exist or that the
//! assistant may not use (files, settings, quitting, recipes, the cloud keys…) are dropped and
//! reported. Nothing changes until the plan runs: the frontend shows it and runs it as a recipe
//! (`recipes.run`), so its edits are one undo step and it can be saved as a recipe.
//!
//! As the other cloud text features ([`crate::ai_tasks`]): `engine` / `model`, `confirm` (consent
//! per provider) and `background`.

use serde_json::{Value, json};

use crate::ai_tasks::{Outcome, TaskWork, other, run, started};
use crate::commands::{CommandSpec, bad, str_p};
use crate::{Result, Session};

/// Command families the assistant may use.
const ALLOWED: [&str; 17] = [
    "transcript.",
    "captions.",
    "clip.",
    "sequence.",
    "edit.",
    "audio.",
    "highlights.",
    "youtube.",
    "markers.",
    "graphics.template.apply",
    "effects.apply",
    "essentialSound.",
    "ai.generateImage",
    "tts.create",
    "playhead.",
    "timeline.select",
    "export.queue.add",
];

/// Never, whatever the family: commands that leave the project, change settings or keys, or
/// would loop back into the assistant.
const NEVER: [&str; 6] = ["sequence.close", "edit.undo", "edit.redo", "captions.import", "captions.export", "clip.replaceFromBin"];

pub fn allowed(id: &str) -> bool {
    ALLOWED.iter().any(|p| id.starts_with(p)) && !NEVER.contains(&id) && crate::find_command(id).is_some()
}

/// Commands offered to the engine: the allowed ones that are in a menu, and the parameter-driven
/// ones a menu item opens a dialog for.
fn catalog() -> String {
    let extra =
        ["captions.applyStylePreset", "captions.setStyle", "playhead.set", "timeline.select", "markers.add", "export.queue.add", "graphics.template.apply"];
    crate::command_specs()
        .iter()
        .filter(|c| allowed(c.id) && (!c.menu.is_empty() || extra.contains(&c.id)))
        .map(|c| format!("{} — {} — {}", c.id, c.label.trim_end_matches('…'), c.params))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A summary of the open project for the engine: no media, no transcript text.
pub fn context(s: &Session) -> Value {
    let Some(q) = s.active_sequence() else { return json!({"sequence": null}) };
    let st = &q.settings;
    let words = crate::transcript::sequence_words(s).len();
    json!({
        "sequence": s.state.active_sequence.and_then(|i| s.project.item(i)).map(|i| i.name.clone()),
        "seconds": (q.duration().seconds() * 10.0).round() / 10.0,
        "frame": [st.width, st.height],
        "vertical": st.height > st.width,
        "videoTracks": q.video_tracks.iter().map(|t| t.items.len()).collect::<Vec<_>>(),
        "audioTracks": q.audio_tracks.iter().map(|t| t.items.len()).collect::<Vec<_>>(),
        "captionTracks": q.caption_tracks.iter().map(|t| json!({"name": t.name, "language": t.language, "captions": t.captions.len()})).collect::<Vec<_>>(),
        "transcriptWords": words,
        "selectedClips": s.state.selection.len(),
        "selectedCaptions": s.state.caption_selection.len(),
        "playheadSeconds": (s.playhead().seconds() * 10.0).round() / 10.0,
        "markers": q.markers.len(),
        "captionStylePresets": crate::caption_presets::builtin().iter().map(|p| p.id).collect::<Vec<_>>(),
    })
}

fn prompt() -> String {
    [
        "You are the assistant inside Jilo Studio, a video editor. Turn the user's request into a short plan of editor commands.",
        "",
        "Rules:",
        "1. Use only commands from the list below, with their exact ids; give each step's parameters as a JSON object in a string, following the command's parameter description. Leave out parameters you do not need.",
        "2. Never give ids of clips, items, captions or tracks: commands act on the selection and the open sequence. Caption tracks may be named \"C1\" or \"all\".",
        "3. Keep the plan short (at most 12 steps) and in the order it must run. Transcribe (transcript.generate) before anything that needs a transcript (captions from the transcript, removing pauses or filler words, highlights, YouTube texts, zooms by meaning).",
        "4. Commands whose label ends a dialog (such as Auto Zoom, Reframe to Face, Find Highlights) need their parameters when used here.",
        "5. If the request cannot be done with these commands, return no steps and say why.",
        "6. \"reply\": one or two short sentences to the user, in the language of the request, saying what the plan will do.",
        "",
        "The user message is JSON: the request and a summary of the project.",
        "",
        "Commands (id — label — parameters):",
    ]
    .join("\n")
        + "\n"
        + &catalog()
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reply": {"type": "string"},
            "steps": {"type": "array", "items": {"type": "object", "properties": {
                "command": {"type": "string"},
                "params": {"type": "string"},
                "why": {"type": "string"}
            }, "required": ["command", "params"]}}
        },
        "required": ["reply", "steps"]
    })
}

/// The steps of an answer that may run, and the commands it named that may not.
pub fn checked_steps(answer: &Value) -> (Vec<Value>, Vec<String>) {
    let mut ok = Vec::new();
    let mut rejected = Vec::new();
    for st in answer.get("steps").and_then(Value::as_array).into_iter().flatten().take(20) {
        let cmd = st.get("command").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if !allowed(&cmd) {
            if !cmd.is_empty() {
                rejected.push(cmd);
            }
            continue;
        }
        // parameters: a JSON object, or a string holding one
        let params = match st.get("params") {
            Some(Value::String(t)) if !t.trim().is_empty() => serde_json::from_str::<Value>(t).ok().filter(Value::is_object).unwrap_or(json!({})),
            Some(v @ Value::Object(_)) => v.clone(),
            _ => json!({}),
        };
        let why = st.get("why").and_then(Value::as_str).unwrap_or_default();
        ok.push(json!({"command": cmd, "params": params, "enabled": true, "why": why}));
    }
    (ok, rejected)
}

fn plan(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "assistant.plan";
    let request = str_p(p, "request").map(str::trim).filter(|r| !r.is_empty()).ok_or_else(|| bad(cmd, "`request`: what to do, in your words"))?.to_string();
    let user = json!({"request": request, "project": context(s)}).to_string();
    let system = prompt();
    let inputs = json!({"user": user});
    let work: TaskWork = Box::new(move |engine, ctx, progress| {
        if !progress(0.1, "Planning") {
            return Err(filmcraft_ai::AiError::Cancelled);
        }
        let v = engine.json(ctx, &system, &user, Some(&schema()), 0.2)?;
        let _ = progress(1.0, "Plan ready");
        Ok(v)
    });
    let answer = match run(s, p, cmd, "Planning", &inputs, "your request and a summary of the project", work)? {
        Outcome::Started(id) => return Ok(started(id)),
        Outcome::Ready(v) => v,
    };
    let reply = answer.get("reply").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let (steps, rejected) = checked_steps(&answer);
    if reply.is_empty() && steps.is_empty() {
        return Err(other("the engine answered nothing; try again or rephrase"));
    }
    Ok(json!({"request": request, "reply": reply, "steps": steps, "rejected": rejected}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "assistant.plan",
        label: "Ask the Assistant",
        menu: &[],
        shortcut: None,
        params: r#"{"request":str,"engine":"gemini|openai|anthropic"?,"model":str?,"confirm":bool?,"background":bool?}"#,
        enabled: crate::commands::always,
        run: plan,
        journal: false,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_features_tests::{gemini, session};

    #[test]
    fn a_request_becomes_checked_steps_that_run_as_one_undo_step() {
        let (mut s, fake, _dir) = session("assistant");
        s.execute("captions.newTrack", json!({})).unwrap();
        s.execute("captions.add", json!({"text": "hello", "seconds": 0})).unwrap();
        gemini(&fake, |prompt, user| {
            assert!(prompt.contains("captions.applyStylePreset") && !prompt.contains("file.save "), "the catalogue has editing commands only");
            let u: Value = serde_json::from_str(user).unwrap();
            assert_eq!(u["request"], "make the captions yellow karaoke");
            assert_eq!(u["project"]["captionTracks"][0]["captions"], 1);
            json!({"reply": "Captions get the Karaoke Yellow style.", "steps": [
                {"command": "captions.applyStylePreset", "params": "{\"preset\": \"karaoke-yellow\", \"track\": \"all\"}", "why": "style"},
                {"command": "file.save", "params": "{}"},
                {"command": "no.such", "params": ""}
            ]})
        });
        let e = s.execute("assistant.plan", json!({"request": "make the captions yellow karaoke"})).unwrap_err();
        assert!(e.to_string().contains("CONFIRM_CLOUD"), "asks before sending: {e}");
        let r = s.execute("assistant.plan", json!({"request": "make the captions yellow karaoke", "confirm": true})).unwrap();
        assert_eq!(r["reply"], "Captions get the Karaoke Yellow style.");
        assert_eq!(r["steps"].as_array().unwrap().len(), 1);
        assert_eq!(r["rejected"], json!(["file.save", "no.such"]));
        assert_eq!(r["steps"][0]["params"]["preset"], "karaoke-yellow");
        // nothing changed yet; the plan runs as a recipe
        assert_ne!(s.active_sequence().unwrap().caption_tracks[0].style.highlight, filmcraft_project::CaptionHighlight::Color);
        let undo = s.history.undo.len();
        s.execute("recipes.run", json!({"name": "Assistant", "steps": r["steps"], "wait": true})).unwrap();
        assert_eq!(s.active_sequence().unwrap().caption_tracks[0].style, crate::caption_presets::find("karaoke-yellow").unwrap().style);
        assert_eq!(s.history.undo.len(), undo + 1);
    }

    #[test]
    fn the_prompt_stays_small() {
        let p = prompt();
        assert!(p.len() < 48_000, "{} bytes", p.len());
        assert!(p.contains("transcript.removePauses") && p.contains("captions.applyStylePreset"));
    }

    #[test]
    fn the_assistant_cannot_touch_files_settings_or_itself() {
        for id in ["file.save", "app.quit", "prefs.set", "ai.setKey", "recipes.delete", "assistant.plan", "edit.undo", "sequence.close"] {
            assert!(!allowed(id), "{id}");
        }
        for id in ["transcript.removePauses", "captions.applyStylePreset", "clip.autoZoom", "sequence.addEdit"] {
            assert!(allowed(id), "{id}");
        }
    }
}
