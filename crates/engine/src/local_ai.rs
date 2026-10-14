//! Settings ▸ AI on this computer: the local models (Whisper speech models, the natural voices)
//! with their size, downloaded or not, downloaded in the background and deleted to free the disk;
//! the cloud consents given so far, taken back; and "Only on this computer", which refuses every
//! cloud engine (the consent check says why) until it is turned off.
//!
//! | Command | |
//! |---|---|
//! | `ai.localModels` | the models, sizes and what is downloaded |
//! | `ai.downloadModel` | `{model}`: a Whisper model id or `voices`, as a background job |
//! | `ai.deleteModel` | `{model}`: delete a downloaded model (the bytes freed) |
//! | `ai.resetConsents` | forget which cloud providers the user agreed to send material to |

use std::path::Path;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// The id `ai.*Model` use for the natural voices package.
pub const VOICES: &str = "voices";

fn dir_size(d: &Path) -> u64 {
    std::fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    _ => e.metadata().map_or(0, |m| m.len()),
                })
                .sum()
        })
        .unwrap_or(0)
}

fn local_models(_: &mut Session, _: &Value) -> Result<Value> {
    let dir = crate::transcript::models_dir();
    let whisper: Vec<Value> = filmcraft_speech::models::catalogue()
        .iter()
        .map(|m| {
            let installed = dir.as_ref().is_some_and(|d| filmcraft_speech::models::installed(d, m));
            json!({"id": m.id, "name": m.name, "kind": "speech", "size": m.size(), "installed": installed, "license": m.license, "source": m.source, "description": m.description})
        })
        .collect();
    let voices = json!({
        "id": VOICES, "name": filmcraft_tts::catalog::PACKAGE_NAME, "kind": "voices", "size": filmcraft_tts::catalog::size(),
        "installed": dir.as_ref().is_some_and(|d| filmcraft_tts::catalog::installed(d)),
    });
    let used = dir.as_ref().map_or(0, |d| dir_size(d));
    Ok(json!({
        "dir": dir.map(|d| d.to_string_lossy().to_string()),
        "used": used,
        "models": whisper.into_iter().chain(std::iter::once(voices)).collect::<Vec<_>>(),
        "speechAvailable": filmcraft_speech::available(),
    }))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.downloadModel";
    let id = str_p(p, "model").ok_or_else(|| bad(cmd, "`model`: a Whisper model id or `voices`"))?;
    if id == VOICES {
        return s.execute("tts.downloadVoices", json!({"wait": p.get("wait").cloned().unwrap_or(json!(false))}));
    }
    let m = filmcraft_speech::models::find(id).ok_or_else(|| bad(cmd, format!("no model `{id}`")))?;
    let dir = crate::transcript::models_dir().ok_or_else(|| bad(cmd, "no data directory for models"))?;
    if filmcraft_speech::models::installed(&dir, m) {
        return Ok(json!({"job": null, "installed": true}));
    }
    if !cfg!(feature = "speech-download") {
        return Err(bad(cmd, "model downloads are not available in this build"));
    }
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let job = crate::narration::new_job(s, format!("Downloading {}", m.name));
    job.progress.total.store(m.size(), Ordering::Relaxed);
    let (prog, res) = (job.progress.clone(), job.result.clone());
    let id = crate::narration::spawn(s, job, wait, "filmcraft-model-download", move || {
        let t0 = web_time::Instant::now();
        let r = fetch(&dir, m, &prog);
        crate::narration::finish(&prog, &res, r.map(|()| format!("{} downloaded", m.name)), t0);
    })?;
    Ok(json!({"job": id, "installed": false}))
}

#[cfg(feature = "speech-download")]
fn fetch(dir: &Path, m: &'static filmcraft_speech::models::ModelInfo, prog: &filmcraft_export::Progress) -> std::result::Result<(), String> {
    let mut progress = |done: u64, total: u64, file: &str| {
        prog.done.store(done, Ordering::Relaxed);
        prog.total.store(total, Ordering::Relaxed);
        *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Downloading {file}");
        !prog.cancel.load(Ordering::Relaxed)
    };
    filmcraft_speech::models::download(dir, m, &mut progress).map_err(|e| e.to_string())
}

#[cfg(not(feature = "speech-download"))]
fn fetch(_: &Path, _: &'static filmcraft_speech::models::ModelInfo, _: &filmcraft_export::Progress) -> std::result::Result<(), String> {
    let _ = PoisonError::<()>::new;
    Err("model downloads are not available in this build".into())
}

fn delete(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.deleteModel";
    let id = str_p(p, "model").ok_or_else(|| bad(cmd, "`model`"))?;
    let dir = crate::transcript::models_dir().ok_or_else(|| bad(cmd, "no data directory for models"))?;
    let target = if id == VOICES {
        filmcraft_tts::catalog::package_dir(&dir)
    } else {
        let m = filmcraft_speech::models::find(id).ok_or_else(|| bad(cmd, format!("no model `{id}`")))?;
        filmcraft_speech::models::model_dir(&dir, m)
    };
    if !target.starts_with(&dir) || !target.exists() {
        return Ok(json!({"model": id, "freed": 0}));
    }
    let freed = dir_size(&target);
    std::fs::remove_dir_all(&target).map_err(|e| EngineError::Other(format!("{}: {e}", target.display())))?;
    Ok(json!({"model": id, "freed": freed}))
}

fn reset_consents(s: &mut Session, _: &Value) -> Result<Value> {
    let before = s.prefs.ai.confirmed_providers.clone();
    s.execute("prefs.set", json!({"key": "ai.confirmedProviders", "value": []}))?;
    Ok(json!({"forgotten": before}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec("ai.localModels", "Local AI Models", "{}", local_models as fn(&mut Session, &Value) -> Result<Value>),
        spec("ai.downloadModel", "Download AI Model", r#"{"model":"whisper-…|voices","wait":bool?}"#, download),
        spec("ai.deleteModel", "Delete AI Model", r#"{"model":"whisper-…|voices"}"#, delete),
        spec("ai.resetConsents", "Reset Cloud Consents", "{}", reset_consents),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_listed_with_sizes_and_consents_reset() {
        let mut s = Session::default();
        let r = s.execute("ai.localModels", json!({})).unwrap();
        let models = r["models"].as_array().unwrap();
        assert!(models.len() >= 2);
        assert!(models.iter().all(|m| m["size"].as_u64().unwrap_or(0) > 1_000_000), "{r}");
        assert!(models.iter().any(|m| m["id"] == VOICES));
        assert!(s.execute("ai.deleteModel", json!({"model": "nope"})).is_err());
        s.prefs.ai.confirmed_providers = vec!["gemini".into(), "openai".into()];
        let r = s.execute("ai.resetConsents", json!({})).unwrap();
        assert_eq!(r["forgotten"], json!(["gemini", "openai"]));
        assert!(s.prefs.ai.confirmed_providers.is_empty());
    }

    #[test]
    fn only_on_this_computer_refuses_the_cloud() {
        let mut s = Session::default();
        s.prefs.ai.local_only = true;
        let e = crate::recognition::ensure_consent_for(&mut s, filmcraft_ai::Provider::Gemini, &json!({"confirm": true}), "x", "text").unwrap_err();
        assert!(e.to_string().contains("Only on this computer"), "{e}");
        assert!(s.prefs.ai.confirmed_providers.is_empty(), "no consent recorded");
        s.prefs.ai.local_only = false;
        crate::recognition::ensure_consent_for(&mut s, filmcraft_ai::Provider::Gemini, &json!({"confirm": true}), "x", "text").unwrap();
    }
}
