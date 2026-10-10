//! Cloud text features (translation, highlights, YouTube texts, zooms by meaning) share one shape:
//! material taken from the project, one or more JSON answers from the text engine of Settings ▸ AI
//! (Gemini or OpenAI), and a change applied as one undo step.
//!
//! Each feature command takes `engine` / `model` (default: Settings ▸ AI), `confirm` (the user's
//! one-time consent per provider, as for speech) and `background`:
//! - without `background` the answer is fetched in the call and applied;
//! - with `background: true` it is fetched in a job (progress, Stop) and kept in
//!   [`Session::ai_task_cache`]; the same command without `background` then applies it at once,
//!   sending nothing again. The interface works that way, so it never waits on the network.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_ai::{AiError, Ctx, JsonRequest, Provider, Secret, Transport};
use serde_json::{Value, json};

use crate::commands::{bool_p, str_p};
use crate::{EngineError, Result, Session};

/// Fetched answers by task key, waiting to be applied.
pub type TaskCache = Arc<Mutex<Vec<(String, Value)>>>;

/// The text engine of a task.
#[derive(Clone)]
pub struct TextEngine {
    pub provider: Provider,
    pub model: String,
    key: Secret,
    transport: Arc<dyn Transport>,
    sleep: Option<filmcraft_ai::SleepFn>,
}

impl TextEngine {
    /// The engine `p` asks for (`engine`, `model`), else Settings ▸ AI; needs a key, the network and
    /// the user's consent to send `what`.
    pub fn for_task(s: &mut Session, p: &Value, cmd: &str, what: &str) -> Result<Self> {
        let provider = match str_p(p, "engine") {
            Some(e) => Provider::from_id(e)
                .filter(|p| p.does_text())
                .ok_or_else(|| crate::commands::bad(cmd, format!("unknown text engine `{e}` (gemini, openai or anthropic)")))?,
            None => s.prefs.ai.text_provider(),
        };
        let model = str_p(p, "model").map(str::to_string).unwrap_or_else(|| s.prefs.ai.text_model(provider));
        let (key, _) = crate::ai::key(s, provider).ok_or_else(|| crate::ai::ai_err(AiError::NoKey(provider)))?;
        let transport = crate::ai::transport(s).map_err(crate::ai::ai_err)?;
        crate::recognition::ensure_consent_for(s, provider, p, cmd, what)?;
        Ok(Self { provider, model, key, transport, sleep: s.ai_sleep.clone() })
    }

    /// A call context whose cancel check is `stop`.
    pub fn ctx(&self, stop: impl Fn() -> bool + Send + Sync + 'static) -> Ctx {
        let c = Ctx::new(self.transport.clone()).with_cancel(stop);
        match &self.sleep {
            Some(sl) => c.with_sleep(sl.clone()),
            None => c,
        }
    }

    /// One JSON answer (a retired model is replaced for this call).
    pub fn json(&self, ctx: &Ctx, system: &str, user: &str, schema: Option<&Value>, temperature: f32) -> std::result::Result<Value, AiError> {
        let req = JsonRequest { system, user, schema, temperature };
        filmcraft_ai::generate_json(ctx, self.provider, &self.key, &self.model, &req).map(|a| a.value)
    }
}

/// A stable key for a task's inputs.
pub fn task_key(cmd: &str, inputs: &Value) -> String {
    // FNV-1a over the canonical JSON
    let mut h: u64 = 0xcbf29ce484222325;
    for b in inputs.to_string().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{cmd}:{h:016x}")
}

/// Progress of a running task: `(fraction, status)`; false = stopped.
pub type TaskProgress<'a> = &'a dyn Fn(f32, &str) -> bool;

/// The work of a task: fetch the answer with the engine.
pub type TaskWork = Box<dyn FnOnce(&TextEngine, &Ctx, TaskProgress) -> std::result::Result<Value, AiError> + Send>;

/// What [`run`] did.
pub enum Outcome {
    /// The answer is here: apply it.
    Ready(Value),
    /// A background job is fetching it.
    Started(u64),
}

/// Fetch a task's answer: from the cache (a finished background job), in a background job
/// (`background: true`), or now.
pub fn run(s: &mut Session, p: &Value, cmd: &str, label: &str, inputs: &Value, what: &str, work: TaskWork) -> Result<Outcome> {
    let key = task_key(cmd, inputs);
    {
        let mut c = s.ai_task_cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(i) = c.iter().position(|(k, _)| *k == key) {
            return Ok(Outcome::Ready(c.remove(i).1));
        }
    }
    let engine = TextEngine::for_task(s, p, cmd, what)?;
    if bool_p(p, "background") == Some(true) {
        let job = crate::narration::new_job(s, label.to_string());
        job.progress.total.store(1000, Ordering::Relaxed);
        let (prog, res, cache) = (job.progress.clone(), job.result.clone(), s.ai_task_cache.clone());
        let id = crate::narration::spawn(s, job, false, "filmcraft-ai-task", move || {
            let t0 = web_time::Instant::now();
            let stop = prog.clone();
            let ctx = engine.ctx(move || stop.cancel.load(Ordering::Relaxed));
            let progress = |f: f32, status: &str| {
                prog.done.store((f.clamp(0.0, 1.0) * 1000.0) as u64, Ordering::Relaxed);
                *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = status.to_string();
                !prog.cancel.load(Ordering::Relaxed)
            };
            let r = work(&engine, &ctx, &progress).map(|v| {
                cache.lock().unwrap_or_else(PoisonError::into_inner).push((key, v));
                "Ready; run the command again to apply it".to_string()
            });
            crate::narration::finish(&prog, &res, r.map_err(|e| e.to_string()), t0);
        })?;
        return Ok(Outcome::Started(id));
    }
    let ctx = engine.ctx(|| false);
    work(&engine, &ctx, &|_, _| true).map(Outcome::Ready).map_err(crate::ai::ai_err)
}

/// The JSON a background start returns.
pub fn started(id: u64) -> Value {
    json!({"job": id, "background": true})
}

pub fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}
