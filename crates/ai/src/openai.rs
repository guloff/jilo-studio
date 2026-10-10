//! OpenAI (`api.openai.com/v1`): chat completions with a strict JSON schema, the model list, and
//! OpenAI's error answers.

use serde_json::{Map, Value, json};

use crate::{AiError, Ctx, JsonRequest, ModelInfo, ModelUse, Provider, Request, Response, Result, Secret, error_message, parse_json, send};

pub const API_ROOT: &str = "https://api.openai.com/v1";

/// The only OpenAI speech model that returns segment and word times.
pub const DEFAULT_SPEECH_MODEL: &str = "whisper-1";
pub const DEFAULT_TEXT_MODEL: &str = "gpt-4o-mini";

const P: Provider = Provider::OpenAi;

fn bearer(key: &Secret) -> String {
    format!("Bearer {}", key.expose())
}

/// OpenAI's error answer as an [`AiError`] (429 and 5xx were retried before this).
pub fn error(model: &str, r: &Response) -> AiError {
    let msg = error_message(r);
    match r.status {
        401 => AiError::BadApiKey(P),
        403 => AiError::Forbidden(P, msg),
        404 => AiError::ModelNotFound(P, model.to_string()),
        429 => AiError::RateLimited(P),
        s if s >= 500 => AiError::Unavailable(P, msg),
        _ => AiError::Api(P, msg),
    }
}

/// A schema for OpenAI's strict mode: every object lists all its properties as required and allows
/// no others. Without it, "just JSON" answers named the fields as they liked and came out empty.
pub fn strict_schema(schema: &Value) -> Value {
    match schema {
        Value::Array(a) => Value::Array(a.iter().map(strict_schema).collect()),
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, v) in o {
                let v = match (k.as_str(), v) {
                    ("properties", Value::Object(props)) => Value::Object(props.iter().map(|(n, s)| (n.clone(), strict_schema(s))).collect()),
                    _ => strict_schema(v),
                };
                out.insert(k.clone(), v);
            }
            if o.get("type").and_then(Value::as_str) == Some("object")
                && let Some(Value::Object(props)) = o.get("properties")
            {
                out.insert("required".into(), Value::Array(props.keys().map(|k| Value::String(k.clone())).collect()));
                out.insert("additionalProperties".into(), Value::Bool(false));
            }
            Value::Object(out)
        }
        v => v.clone(),
    }
}

/// The chat completions request body.
pub fn body(model: &str, req: &JsonRequest) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    if !req.user.is_empty() {
        messages.push(json!({"role": "user", "content": req.user}));
    }
    let response_format = match req.schema {
        Some(s) => json!({"type": "json_schema", "json_schema": {"name": "result", "strict": true, "schema": strict_schema(s)}}),
        None => json!({"type": "json_object"}),
    };
    json!({"model": model, "messages": messages, "response_format": response_format})
}

/// A JSON answer from a chat model.
pub fn generate_json(ctx: &Ctx, key: &Secret, model: &str, req: &JsonRequest) -> Result<Value> {
    let req = Request::post(format!("{API_ROOT}/chat/completions"), "application/json", body(model, req).to_string().into_bytes())
        .header("Authorization", &bearer(key));
    let r = send(ctx, P, &req)?;
    if !r.ok() {
        return Err(error(model, &r));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, "chat completion".into()))?;
    if let Some(why) = v.pointer("/choices/0/message/refusal").and_then(Value::as_str) {
        return Err(AiError::Blocked(P, why.to_string()));
    }
    let text = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default();
    parse_json(P, text)
}

/// OpenAI models for `usage`: speech — `whisper*` / `*transcribe*`; text — GPT and o-series chat
/// models (audio, realtime, image, TTS, embedding, moderation and search models left out).
pub fn list_models(ctx: &Ctx, key: &Secret, usage: ModelUse) -> Result<Vec<ModelInfo>> {
    let req = Request::get(format!("{API_ROOT}/models")).header("Authorization", &bearer(key));
    let r = send(ctx, P, &req)?;
    if !r.ok() {
        return Err(error("", &r));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, "model list".into()))?;
    let mut ids: Vec<String> =
        v.get("data").and_then(Value::as_array).into_iter().flatten().filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_string).collect();
    ids.retain(|id| match usage {
        ModelUse::Speech => id.contains("whisper") || id.contains("transcribe"),
        ModelUse::Text => {
            let chat = id.starts_with("gpt") || (id.len() > 1 && id.starts_with('o') && id.as_bytes()[1].is_ascii_digit() && id.as_bytes()[1] != b'0');
            chat && !["audio", "realtime", "image", "tts", "transcribe", "embedding", "moderation", "search"].iter().any(|x| id.contains(x))
        }
    });
    ids.sort();
    Ok(ids.into_iter().map(|id| ModelInfo { name: if id == DEFAULT_SPEECH_MODEL { format!("{id} (word times)") } else { id.clone() }, id }).collect())
}
