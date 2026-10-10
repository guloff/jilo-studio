//! Cloud AI providers (Settings ▸ AI): keys, key checks and model lists.
//!
//! | command | does |
//! |---|---|
//! | `ai.status` | engines, models, where each provider's key comes from (`file`, `env:NAME` or none — never the key), the model lists found by the last check, and whether this build can reach the network |
//! | `ai.setKey` | store (`key`) or remove (empty / null) a provider's key in `<data dir>/ai.json`; not journaled |
//! | `ai.test` | check a key by listing the provider's models, in a background job (network) |
//!
//! Keys live in `<data dir>/ai.json` (owner-only, written atomically; see `filmcraft_ai::keys`), or
//! come from `GEMINI_API_KEY` / `GOOGLE_API_KEY` / `OPENAI_API_KEY`. They are not kept in the
//! keychain, never enter projects, preferences, the command journal or logs, and no command returns
//! them. Requests go through [`Session::ai_transport`] when a host or test installed one, else over
//! HTTPS (feature `ai-net`); without either, AI commands that need the network fail with a clear
//! error and nothing is sent.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_ai::{AiError, Ctx, ModelInfo, ModelUse, Provider, Secret, Transport};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;

fn spec(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    // none of these change the project; `ai.setKey` must never be journaled (it carries the key)
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

/// The models a provider offered at its last key check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderModels {
    pub speech: Vec<ModelInfo>,
    pub text: Vec<ModelInfo>,
}

/// Model lists by provider, filled by `ai.test` jobs.
pub type ModelCache = Arc<Mutex<BTreeMap<Provider, ProviderModels>>>;

/// Whether this build can send requests over HTTPS.
pub fn network_available() -> bool {
    cfg!(feature = "ai-net")
}

/// The key file: [`Session::ai_keys_file`], else `<data dir>/ai.json`.
pub fn keys_path(s: &Session) -> Option<PathBuf> {
    s.ai_keys_file.clone().or_else(|| crate::autosave::default_data_dir().map(|d| filmcraft_ai::keys::path_in(&d)))
}

/// The key for `p` and where it came from.
pub fn key(s: &Session, p: Provider) -> Option<(Secret, filmcraft_ai::keys::KeySource)> {
    filmcraft_ai::keys::key(keys_path(s).as_deref(), p)
}

/// The transport requests go through: the installed one, else HTTPS (feature `ai-net`).
pub fn transport(s: &Session) -> std::result::Result<Arc<dyn Transport>, AiError> {
    if let Some(t) = &s.ai_transport {
        return Ok(t.clone());
    }
    #[cfg(feature = "ai-net")]
    {
        Ok(Arc::new(filmcraft_ai::transport::HttpTransport::new()))
    }
    #[cfg(not(feature = "ai-net"))]
    {
        Err(AiError::NoNetwork)
    }
}

/// A call context whose cancel check is a job's Stop flag.
pub fn ctx(s: &Session, progress: Arc<filmcraft_export::Progress>) -> std::result::Result<Ctx, AiError> {
    let mut c = Ctx::new(transport(s)?).with_cancel(move || progress.cancel.load(Ordering::Relaxed));
    if let Some(sleep) = &s.ai_sleep {
        c = c.with_sleep(sleep.clone());
    }
    Ok(c)
}

pub(crate) fn ai_err(e: AiError) -> EngineError {
    EngineError::Other(e.to_string())
}

fn provider_p(cmd: &str, p: &Value) -> Result<Provider> {
    let id = str_p(p, "provider").ok_or_else(|| bad(cmd, "`provider` is required (gemini or openai)"))?;
    Provider::from_id(id).ok_or_else(|| bad(cmd, format!("unknown provider `{id}` (gemini or openai)")))
}

fn models_json(m: &[ModelInfo]) -> Value {
    Value::Array(m.iter().map(|m| json!({"id": m.id, "name": m.name})).collect())
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let a = &s.prefs.ai;
    let cache = s.ai_models.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let providers: Vec<Value> = Provider::ALL
        .iter()
        .map(|&p| {
            let found = cache.get(&p);
            json!({
                "id": p.id(),
                "name": p.name(),
                "key": key(s, p).map(|(_, src)| src.describe()),
                "keyUrl": p.key_url(),
                "speechModel": a.speech_model(p),
                "textModel": a.text_model(p),
                "confirmed": a.confirmed(p),
                "speechModels": found.map(|m| models_json(&m.speech)),
                "textModels": found.map(|m| models_json(&m.text)),
            })
        })
        .collect();
    Ok(json!({
        "speechEngine": a.speech_engine,
        "textEngine": a.text_engine,
        "network": network_available() || s.ai_transport.is_some(),
        "keysFile": keys_path(s).map(|p| p.to_string_lossy().into_owned()),
        "providers": providers,
    }))
}

fn set_key(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.setKey";
    let provider = provider_p(cmd, p)?;
    let k = match p.get("key") {
        None | Some(Value::Null) => None,
        Some(Value::String(k)) => Some(k.as_str()),
        Some(_) => return Err(bad(cmd, "`key` is a string (empty or null removes the key)")),
    };
    let path = keys_path(s).ok_or_else(|| bad(cmd, "no data directory for the key file"))?;
    // the error names the file, never the key
    filmcraft_ai::keys::set_key(&path, provider, k).map_err(|e| EngineError::Other(format!("could not save the key file {}: {e}", path.display())))?;
    s.ai_models.lock().unwrap_or_else(PoisonError::into_inner).remove(&provider);
    Ok(json!({"provider": provider.id(), "key": key(s, provider).map(|(_, src)| src.describe())}))
}

fn test(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.test";
    let provider = provider_p(cmd, p)?;
    let (k, _) = key(s, provider).ok_or_else(|| ai_err(AiError::NoKey(provider)))?;
    let job = crate::narration::new_job(s, format!("Checking the {} key", provider.name()));
    let c = ctx(s, job.progress.clone()).map_err(ai_err)?;
    let (prog, res, cache) = (job.progress.clone(), job.result.clone(), s.ai_models.clone());
    let wait = bool_p(p, "wait").unwrap_or(false);
    let id = crate::narration::spawn(s, job, wait, "filmcraft-ai-test", move || {
        let t0 = web_time::Instant::now();
        *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Contacting {}…", provider.name());
        let r = check(&c, provider, &k).map(|(msg, models)| {
            cache.lock().unwrap_or_else(PoisonError::into_inner).insert(provider, models);
            msg
        });
        crate::narration::finish(&prog, &res, r.map_err(|e| e.to_string()), t0);
    })?;
    Ok(json!({"job": id}))
}

/// List the provider's models (OpenAI: the speech and the text models are told apart by id).
fn check(c: &Ctx, provider: Provider, k: &Secret) -> std::result::Result<(String, ProviderModels), AiError> {
    let (msg, text) = filmcraft_ai::test_key(c, provider, k)?;
    let speech = match provider {
        Provider::Gemini => text.clone(),
        Provider::OpenAi => filmcraft_ai::list_models(c, provider, k, ModelUse::Speech)?,
    };
    Ok((msg, ProviderModels { speech, text }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec("ai.status", "AI Status", "{}", status),
        spec("ai.setKey", "Set AI Key", r#"{"provider":"gemini|openai","key":str|null}"#, set_key),
        spec("ai.test", "Check AI Key", r#"{"provider":"gemini|openai","wait":bool?}"#, test),
    ]
}
