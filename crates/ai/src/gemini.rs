//! Google Gemini (`generativelanguage.googleapis.com/v1beta`): `generateContent` with a response
//! schema, `ListModels`, and Gemini's error answers.

use serde_json::{Value, json};

use crate::{AiError, Ctx, JsonRequest, ModelInfo, Provider, Request, Response, Result, Secret, clip, error_message, parse_json, send};

pub const API_ROOT: &str = "https://generativelanguage.googleapis.com/v1beta";

/// An alias rather than a version: Google retires versions, `-latest` always names a served model.
pub const DEFAULT_MODEL: &str = "gemini-flash-latest";

const P: Provider = Provider::Gemini;

/// One part of a request.
#[derive(Clone, Copy, Debug)]
pub enum Part<'a> {
    Text(&'a str),
    /// Inline media (`audio/wav`…), sent base64-encoded.
    Inline {
        mime: &'a str,
        data: &'a [u8],
    },
}

/// Generation settings.
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    pub temperature: f32,
    /// JSON schema of the answer (`responseSchema`); the answer is JSON either way.
    pub schema: Option<&'a Value>,
}

/// A model's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub text: String,
    /// `STOP`, `MAX_TOKENS` (the answer was cut off), …
    pub finish_reason: Option<String>,
}

impl Generated {
    /// The answer stopped at the output limit (a cut-off transcript is recognised again in halves).
    pub fn truncated(&self) -> bool {
        self.finish_reason.as_deref() == Some("MAX_TOKENS")
    }
}

/// The `generateContent` request body.
pub fn body(parts: &[Part], cfg: &Config) -> Value {
    let parts: Vec<Value> = parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => json!({"text": t}),
            Part::Inline { mime, data } => json!({"inline_data": {"mime_type": mime, "data": crate::base64::encode(data)}}),
        })
        .collect();
    let mut gc = json!({"temperature": cfg.temperature, "responseMimeType": "application/json"});
    if let Some(s) = cfg.schema {
        gc["responseSchema"] = s.clone();
    }
    json!({"contents": [{"role": "user", "parts": parts}], "generationConfig": gc})
}

/// Call `generateContent` on `model`.
pub fn generate(ctx: &Ctx, key: &Secret, model: &str, parts: &[Part], cfg: &Config) -> Result<Generated> {
    let url = format!("{API_ROOT}/models/{}:generateContent", encode_path(model));
    let req = Request::post(url, "application/json", body(parts, cfg).to_string().into_bytes()).header("x-goog-api-key", key.expose());
    let r = send(ctx, P, &req)?;
    if !r.ok() {
        return Err(error(model, &r));
    }
    read_answer(&r)
}

fn read_answer(r: &Response) -> Result<Generated> {
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, clip(&String::from_utf8_lossy(&r.body), 400)))?;
    if let Some(why) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(AiError::Blocked(P, why.to_string()));
    }
    let cand = v.pointer("/candidates/0");
    let text: String = cand
        .and_then(|c| c.pointer("/content/parts"))
        .and_then(Value::as_array)
        .map(|ps| ps.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    let finish_reason = cand.and_then(|c| c.get("finishReason")).and_then(Value::as_str).map(str::to_string);
    let text = text.trim().to_string();
    if text.is_empty()
        && let Some(why) = finish_reason.as_deref().filter(|f| matches!(*f, "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "SPII" | "RECITATION"))
    {
        return Err(AiError::Blocked(P, why.to_string()));
    }
    Ok(Generated { text, finish_reason })
}

/// A JSON answer: the instructions and the material as two text parts.
pub fn generate_json(ctx: &Ctx, key: &Secret, model: &str, req: &JsonRequest) -> Result<Value> {
    let parts = [Part::Text(req.system), Part::Text(req.user)];
    let parts = if req.user.is_empty() { &parts[..1] } else { &parts[..] };
    let g = generate(ctx, key, model, parts, &Config { temperature: req.temperature, schema: req.schema })?;
    parse_json(P, &g.text)
}

/// Gemini's error answer as an [`AiError`] (429 and 5xx were retried before this).
pub fn error(model: &str, r: &Response) -> AiError {
    let msg = error_message(r);
    let lower = msg.to_ascii_lowercase();
    match r.status {
        400 if lower.contains("api key") || lower.contains("api_key") || lower.contains("apikey") => AiError::BadApiKey(P),
        401 => AiError::BadApiKey(P),
        403 => AiError::Forbidden(P, msg),
        404 if lower.contains("no longer available") || lower.contains("deprecated") || lower.contains("retired") => {
            AiError::ModelRetired { provider: P, model: model.to_string(), replacement: replacement(&msg) }
        }
        404 => AiError::ModelNotFound(P, model.to_string()),
        429 => AiError::RateLimited(P),
        s if s >= 500 => AiError::Unavailable(P, msg),
        _ => AiError::Api(P, msg),
    }
}

/// The model a "retired" error points to: `… Please use models/<id> for the latest …`.
pub fn replacement(msg: &str) -> Option<String> {
    let mut rest = msg;
    while let Some(i) = rest.find("models/") {
        let tail = &rest[i + "models/".len()..];
        let id: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).collect();
        if !id.is_empty() && tail[id.len()..].trim_start().starts_with("for the latest") {
            return Some(id);
        }
        rest = tail;
    }
    None
}

/// Gemini models that generate content, `-latest` aliases first, then by id (embedding, image,
/// TTS and live-audio models left out).
pub fn list_models(ctx: &Ctx, key: &Secret) -> Result<Vec<ModelInfo>> {
    let req = Request::get(format!("{API_ROOT}/models?pageSize=200")).header("x-goog-api-key", key.expose());
    let r = send(ctx, P, &req)?;
    if !r.ok() {
        return Err(error("", &r));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, "model list".into()))?;
    let mut out: Vec<ModelInfo> = v
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|m| m.get("supportedGenerationMethods").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x == "generateContent")))
        .filter_map(|m| {
            let id = m.get("name").and_then(Value::as_str)?.trim_start_matches("models/").to_string();
            let name = m.get("displayName").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| id.clone());
            Some(ModelInfo { id, name })
        })
        .filter(|m| m.id.starts_with("gemini") && !["embedding", "image", "tts", "native-audio"].iter().any(|x| m.id.contains(x)))
        .collect();
    out.sort_by(|a, b| (!a.id.ends_with("-latest"), &a.id).cmp(&(!b.id.ends_with("-latest"), &b.id)));
    Ok(out)
}

/// Percent-encode a model id for the URL path (ids are plain, but a typed one may not be).
fn encode_path(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
