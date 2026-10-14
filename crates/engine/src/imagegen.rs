//! Generated images (`ai.generateImage`): a picture drawn from a description by the image engine of
//! Settings ▸ AI (OpenAI GPT Image or Google Gemini), saved where FilmCraft writes the media it
//! makes (Scratch Disks ▸ Captured, else next to the project), imported as a still and, with
//! `place`, put on the timeline at the playhead on the first video track free there.
//!
//! As the cloud text features: `engine` / `model` override Settings ▸ AI, `confirm` is the user's
//! one-time consent per provider, and with `background: true` the picture is drawn in a job
//! (progress, Stop) and the same command without `background` then imports it, sending nothing again.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use filmcraft_ai::images::{self, Shape};
use filmcraft_ai::{AiError, Ctx, Provider};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, str_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "ai.generateImage";
/// Longest description sent.
const PROMPT_MAX: usize = 4000;

fn shape_for(s: &Session, p: &Value) -> Result<Shape> {
    match str_p(p, "shape") {
        Some(id) => Shape::from_id(id).ok_or_else(|| bad(CMD, format!("`shape`: landscape, portrait or square, not `{id}`"))),
        None => Ok(s.active_sequence().map_or(Shape::Landscape, |q| Shape::for_frame(q.settings.width, q.settings.height))),
    }
}

/// The first video track of the active sequence with nothing at `[at, at + dur)` and not locked.
fn free_video_track(s: &Session, at: Tick, dur: Tick) -> Option<filmcraft_project::TrackId> {
    let q = s.active_sequence()?;
    q.video_tracks.iter().find(|t| !t.locked && !t.items.iter().any(|it| it.start < at + dur && at < it.end())).map(|t| t.id)
}

/// Import the picture at `path` and (with `place`) put it on the timeline.
fn apply(s: &mut Session, p: &Value, path: &str, provider: Provider, model: &str) -> Result<Value> {
    let bytes = std::fs::read(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let item = crate::commands::import_bytes(s, path, bytes.into(), None)?;
    let mut out = json!({"item": item.0, "path": path, "engine": provider.id(), "model": model});
    if bool_p(p, "place").unwrap_or(false) && s.active_sequence().is_some() {
        let at = s.playhead();
        let dur = s.project.item(item).map(|i| i.duration()).filter(|d| *d > Tick::ZERO).unwrap_or(Tick::from_seconds_f64(5.0));
        let track = match free_video_track(s, at, dur) {
            Some(t) => t,
            None => {
                s.execute("sequence.addTracks", json!({"video": 1, "audio": 0}))?;
                free_video_track(s, at, dur).ok_or_else(|| EngineError::Other("no video track is free at the playhead".into()))?
            }
        };
        let r = s.execute("timeline.place", json!({"item": item.0, "track": track.0, "time": at.0, "audio": false}))?;
        // the picture fills the frame whatever size the engine drew it
        let clips: Vec<filmcraft_project::ClipId> =
            r["clips"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(filmcraft_project::ClipId).collect();
        let fitted = s.active_sequence().is_some_and(|q| clips.iter().all(|c| q.find_item(*c).is_some_and(|(_, it)| it.scale_to_frame)));
        if !clips.is_empty() && !fitted {
            s.state.selection = clips.clone();
            s.execute("clip.scaleToFrameSize", json!({}))?;
        }
        out["clips"] = r["clips"].clone();
    }
    Ok(out)
}

fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let prompt = str_p(p, "prompt").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad(CMD, "`prompt`: what the picture shows"))?;
    if prompt.chars().count() > PROMPT_MAX {
        return Err(bad(CMD, format!("the description is longer than {PROMPT_MAX} characters")));
    }
    let provider = match str_p(p, "engine") {
        Some(e) => Provider::from_id(e).filter(|p| p.does_images()).ok_or_else(|| bad(CMD, format!("unknown image engine `{e}` (openai or gemini)")))?,
        None => s.prefs.ai.image_provider(),
    };
    let model = str_p(p, "model").map(str::to_string).unwrap_or_else(|| s.prefs.ai.image_model(provider));
    let shape = shape_for(s, p)?;
    let key = crate::ai_tasks::task_key(CMD, &json!({"engine": provider.id(), "model": model, "shape": format!("{shape:?}"), "prompt": prompt}));
    // drawn by a background job: import it
    let ready = {
        let mut c = s.ai_task_cache.lock().unwrap_or_else(PoisonError::into_inner);
        c.iter().position(|(k, _)| *k == key).map(|i| c.remove(i).1)
    };
    if let Some(v) = ready {
        return apply(s, p, v["path"].as_str().unwrap_or_default(), provider, &model);
    }
    let (secret, _) = crate::ai::key(s, provider).ok_or_else(|| crate::ai::ai_err(AiError::NoKey(provider)))?;
    let transport = crate::ai::transport(s).map_err(crate::ai::ai_err)?;
    crate::recognition::ensure_consent_for(s, provider, p, CMD, "the picture's description")?;
    let dir = crate::voiceover::media_dir(s, p, "Generated Images");
    let background = bool_p(p, "background") == Some(true);
    let job = crate::narration::new_job(s, "Generating an image".into());
    job.progress.total.store(1, Ordering::Relaxed);
    let (prog, res, cache) = (job.progress.clone(), job.result.clone(), s.ai_task_cache.clone());
    let sleep = s.ai_sleep.clone();
    let (prompt, model_c, key_c) = (prompt.to_string(), model.clone(), key.clone());
    let id = crate::narration::spawn(s, job, !background, "filmcraft-image", move || {
        let t0 = web_time::Instant::now();
        let stop = prog.clone();
        let mut ctx = Ctx::new(transport).with_cancel(move || stop.cancel.load(Ordering::Relaxed));
        if let Some(sleep) = sleep {
            ctx = ctx.with_sleep(sleep);
        }
        *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("{} is drawing…", provider.name());
        let r = images::generate(&ctx, provider, &secret, &model_c, &prompt, shape).map_err(|e| e.to_string()).and_then(|pic| {
            let stamp = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            let path = std::path::Path::new(&dir).join(format!("Generated Image {stamp}.{}", pic.extension));
            std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &pic.data)).map_err(|e| format!("{}: {e}", path.display()))?;
            cache.lock().unwrap_or_else(PoisonError::into_inner).push((key_c, json!({"path": path.to_string_lossy()})));
            prog.done.store(1, Ordering::Relaxed);
            Ok("The image is ready".to_string())
        });
        crate::narration::finish(&prog, &res, r, t0);
    })?;
    if background {
        return Ok(json!({"job": id, "background": true}));
    }
    if let Some(Err(e)) = s.jobs.iter().find(|j| j.id == id).and_then(|j| j.result.lock().unwrap_or_else(PoisonError::into_inner).clone()) {
        return Err(EngineError::Other(e));
    }
    let v = {
        let mut c = s.ai_task_cache.lock().unwrap_or_else(PoisonError::into_inner);
        c.iter().position(|(k, _)| *k == key).map(|i| c.remove(i).1)
    };
    let path = v.and_then(|v| v["path"].as_str().map(str::to_string)).ok_or_else(|| EngineError::Other("the image was not saved".into()))?;
    apply(s, p, &path, provider, &model)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Generate Image…",
        menu: crate::ai_menu::TOP,
        shortcut: None,
        params: r#"{"prompt":str,"engine":"openai|gemini"?,"model":str?,"shape":"landscape|portrait|square"?,"place":bool?,"dir":str?,"confirm":bool?,"background":bool?}"#,
        enabled: crate::commands::always,
        run: generate,
        journal: true,
    }]
}
