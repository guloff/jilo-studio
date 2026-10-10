//! Cloud AI providers for FilmCraft's AI features (Settings ▸ AI).
//!
//! - [`Provider`]: Google Gemini and OpenAI. Nothing here runs unless a feature the user started
//!   asks for it; the local engines (Whisper, see `filmcraft-speech`) stay the default.
//! - [`transport`]: the [`Transport`] trait every request goes through. [`transport::FakeTransport`]
//!   answers from a script and records the requests, so tests never touch the network;
//!   [`transport::HttpTransport`] (feature `net`) sends them over HTTPS.
//! - [`gemini`] and [`openai`]: `generateContent` / chat completions with a JSON schema, model
//!   lists, key checks, and the providers' error answers mapped onto [`AiError`].
//! - [`generate_json`]: one provider-neutral call for the text features (translation, highlights,
//!   YouTube texts) that also switches to the replacement of a retired model.
//! - [`speech`]: recognising one window of audio (Gemini inline audio with a schema; OpenAI
//!   `whisper-1` with segment and word times).
//! - [`keys`]: API keys in `<data dir>/ai.json` (owner-only, written atomically) or the
//!   environment. Keys never appear in errors, `Debug` output, projects or logs.
//!
//! Every call retries like Jilo Studio does: up to 3 attempts, waiting 1 s·2ⁿ after a network
//! failure, 2 s·2ⁿ after "too many requests" (429) and 1.5 s·2ⁿ after a server error (5xx). A
//! cancel flag is checked before each attempt and while waiting.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

pub mod base64;
pub mod gemini;
pub mod keys;
pub mod openai;
pub mod speech;
pub mod transport;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use keys::Secret;
pub use transport::{Request, Response, Transport};

/// A cloud AI service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    #[serde(rename = "openai")]
    OpenAi,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Gemini, Provider::OpenAi];

    /// Stable id (`gemini`, `openai`), as in preferences and commands.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Gemini => "gemini",
            Provider::OpenAi => "openai",
        }
    }

    pub fn from_id(s: &str) -> Option<Provider> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gemini" | "google" => Some(Provider::Gemini),
            "openai" => Some(Provider::OpenAi),
            _ => None,
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Provider::Gemini => "Google Gemini",
            Provider::OpenAi => "OpenAI",
        }
    }

    /// Where the user gets a key.
    pub fn key_url(self) -> &'static str {
        match self {
            Provider::Gemini => "https://aistudio.google.com/apikey",
            Provider::OpenAi => "https://platform.openai.com/api-keys",
        }
    }

    /// Environment variables read when `ai.json` has no key, in order.
    pub fn env_vars(self) -> &'static [&'static str] {
        match self {
            Provider::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Provider::OpenAi => &["OPENAI_API_KEY"],
        }
    }

    /// The default model for speech recognition (OpenAI: `whisper-1`, the only one of its models
    /// that returns word times).
    pub fn default_speech_model(self) -> &'static str {
        match self {
            Provider::Gemini => gemini::DEFAULT_MODEL,
            Provider::OpenAi => openai::DEFAULT_SPEECH_MODEL,
        }
    }

    /// The default model for the text features.
    pub fn default_text_model(self) -> &'static str {
        match self {
            Provider::Gemini => gemini::DEFAULT_MODEL,
            Provider::OpenAi => openai::DEFAULT_TEXT_MODEL,
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// What went wrong talking to a provider. The messages never contain a key.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AiError {
    #[error("stopped")]
    Cancelled,
    #[error("no {0} API key: add one in Settings ▸ AI")]
    NoKey(Provider),
    #[error("cloud AI is not available in this build (built without network access)")]
    NoNetwork,
    #[error("could not reach {0}: check the internet connection ({1})")]
    Network(Provider, String),
    #[error("{0} did not accept the API key")]
    BadApiKey(Provider),
    #[error("{0} refused access: {1}")]
    Forbidden(Provider, String),
    #[error("{0}: too many requests, try again later")]
    RateLimited(Provider),
    #[error("{0} is temporarily unavailable: {1}")]
    Unavailable(Provider, String),
    #[error("{provider} no longer serves the model `{model}`")]
    ModelRetired { provider: Provider, model: String, replacement: Option<String> },
    #[error("{0}: the model `{1}` is not available for this key")]
    ModelNotFound(Provider, String),
    #[error("{0} declined the request: {1}")]
    Blocked(Provider, String),
    #[error("{0} sent an answer that could not be read: {1}")]
    BadResponse(Provider, String),
    #[error("{0}: {1}")]
    Api(Provider, String),
}

impl AiError {
    /// Stable machine-readable code (as in Jilo Studio): `BAD_API_KEY`, `MODEL_RETIRED`, …
    pub fn code(&self) -> &'static str {
        match self {
            AiError::Cancelled => "CANCELLED",
            AiError::NoKey(_) => "NO_API_KEY",
            AiError::NoNetwork => "NO_NETWORK",
            AiError::Network(..) => "NETWORK",
            AiError::BadApiKey(_) => "BAD_API_KEY",
            AiError::Forbidden(..) => "FORBIDDEN",
            AiError::RateLimited(_) => "RATE_LIMIT",
            AiError::Unavailable(..) => "RETRY",
            AiError::ModelRetired { .. } => "MODEL_RETIRED",
            AiError::ModelNotFound(..) => "MODEL_NOT_FOUND",
            AiError::Blocked(..) => "BLOCKED",
            AiError::BadResponse(..) => "BAD_RESPONSE",
            AiError::Api(..) => "API_ERROR",
        }
    }
}

pub type Result<T> = std::result::Result<T, AiError>;

/// Whether the user stopped the work (a job's Stop button).
pub type CancelFn = Arc<dyn Fn() -> bool + Send + Sync>;

/// Waits between attempts; returns early once the cancel function says so.
pub type SleepFn = Arc<dyn Fn(Duration, &dyn Fn() -> bool) + Send + Sync>;

/// What a provider call runs with: the transport, a cancel check and the waiting between retries.
#[derive(Clone)]
pub struct Ctx {
    pub transport: Arc<dyn Transport>,
    pub cancel: CancelFn,
    pub sleep: SleepFn,
}

impl Ctx {
    /// A context that really waits between retries (in 50 ms steps, so a cancel is quick).
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport, cancel: Arc::new(|| false), sleep: Arc::new(sleep_sliced) }
    }

    pub fn with_cancel(mut self, cancel: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.cancel = Arc::new(cancel);
        self
    }

    pub fn with_sleep(mut self, sleep: SleepFn) -> Self {
        self.sleep = sleep;
        self
    }

    pub fn cancelled(&self) -> bool {
        (self.cancel)()
    }

    fn check(&self) -> Result<()> {
        if self.cancelled() { Err(AiError::Cancelled) } else { Ok(()) }
    }

    fn wait(&self, d: Duration) -> Result<()> {
        (self.sleep)(d, &*self.cancel);
        self.check()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sleep_sliced(d: Duration, cancelled: &dyn Fn() -> bool) {
    let end = std::time::Instant::now() + d;
    while !cancelled() {
        let now = std::time::Instant::now();
        if now >= end {
            break;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(50)));
    }
}

/// The browser has no blocking sleep (and no network transport here): retry at once.
#[cfg(target_arch = "wasm32")]
fn sleep_sliced(_: Duration, _: &dyn Fn() -> bool) {}

/// Attempts per request.
pub const ATTEMPTS: u32 = 3;

/// Send `req`, retrying network failures, 429 and 5xx answers (see the crate docs). Any other
/// answer, successful or not, is returned for the provider to read.
pub(crate) fn send(ctx: &Ctx, provider: Provider, req: &Request) -> Result<Response> {
    let mut last = AiError::Unavailable(provider, "no answer".into());
    for attempt in 0..ATTEMPTS {
        ctx.check()?;
        let base_ms = match ctx.transport.send(req) {
            Err(e) => {
                last = AiError::Network(provider, e);
                1000
            }
            Ok(r) if r.status == 429 => {
                last = AiError::RateLimited(provider);
                2000
            }
            Ok(r) if r.status >= 500 => {
                last = AiError::Unavailable(provider, error_message(&r));
                1500
            }
            Ok(r) => return Ok(r),
        };
        if attempt + 1 < ATTEMPTS {
            ctx.wait(Duration::from_millis(base_ms << attempt))?;
        }
    }
    ctx.check()?;
    Err(last)
}

/// The `error.message` of a JSON error answer, else the status line.
pub(crate) fn error_message(r: &Response) -> String {
    serde_json::from_slice::<Value>(&r.body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| format!("HTTP {}", r.status))
}

/// At most `n` characters of `s` (for error details that quote a model's answer).
pub(crate) fn clip(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// A model a provider offers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

/// What the model list is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUse {
    Speech,
    Text,
}

/// The models of `provider` usable for `usage`, aliases (`…-latest`) first.
pub fn list_models(ctx: &Ctx, provider: Provider, key: &Secret, usage: ModelUse) -> Result<Vec<ModelInfo>> {
    match provider {
        Provider::Gemini => gemini::list_models(ctx, key),
        Provider::OpenAi => openai::list_models(ctx, key, usage),
    }
}

/// Check a key by listing the provider's models: `"Key accepted, N models available"`.
pub fn test_key(ctx: &Ctx, provider: Provider, key: &Secret) -> Result<(String, Vec<ModelInfo>)> {
    let models = list_models(ctx, provider, key, ModelUse::Text)?;
    Ok((format!("Key accepted, {} models available", models.len()), models))
}

/// A request for one JSON answer.
#[derive(Clone, Copy, Debug)]
pub struct JsonRequest<'a> {
    /// Instructions (Gemini: the first text part; OpenAI: the system message).
    pub system: &'a str,
    /// The material to work on (cues, transcript…).
    pub user: &'a str,
    /// JSON schema of the answer (Gemini `responseSchema`; OpenAI strict `json_schema`).
    pub schema: Option<&'a Value>,
    /// Sampling temperature (Gemini only; OpenAI uses the model's default, as Jilo does).
    pub temperature: f32,
}

/// The answer of [`generate_json`] and the model that produced it (a replacement when the
/// requested model was retired: the caller should remember it).
#[derive(Clone, Debug, PartialEq)]
pub struct JsonAnswer {
    pub value: Value,
    pub model: String,
}

/// Ask `provider` for a JSON answer. A retired Gemini model whose error names its replacement is
/// replaced once (Jilo's `withModelFallback`).
pub fn generate_json(ctx: &Ctx, provider: Provider, key: &Secret, model: &str, req: &JsonRequest) -> Result<JsonAnswer> {
    let (value, model) = with_model_fallback(model, |m| match provider {
        Provider::Gemini => gemini::generate_json(ctx, key, m, req),
        Provider::OpenAi => openai::generate_json(ctx, key, m, req),
    })?;
    Ok(JsonAnswer { value, model })
}

/// Run `f` with `model`; when it fails with [`AiError::ModelRetired`] naming another model, run it
/// once more with that one. Returns the result and the model that produced it.
pub fn with_model_fallback<T>(model: &str, mut f: impl FnMut(&str) -> Result<T>) -> Result<(T, String)> {
    match f(model) {
        Ok(v) => Ok((v, model.to_string())),
        Err(AiError::ModelRetired { replacement: Some(r), .. }) if r != model => f(&r).map(|v| (v, r)),
        Err(e) => Err(e),
    }
}

/// Parse a model's JSON text (some models wrap it in a ```json fence).
pub(crate) fn parse_json(provider: Provider, text: &str) -> Result<Value> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).map(|s| s.trim_end().trim_end_matches("```")).unwrap_or(t);
    serde_json::from_str(t.trim()).map_err(|_| AiError::BadResponse(provider, clip(text, 400)))
}

#[cfg(test)]
mod tests;
