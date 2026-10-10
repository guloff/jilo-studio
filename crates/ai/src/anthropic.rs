//! Anthropic Claude (`api.anthropic.com/v1`): a Messages request whose answer is constrained to a
//! JSON schema (structured outputs, `output_config.format`), the model list, and Anthropic's error
//! answers. Claude works on text only here: translation, highlights, YouTube texts, zooms by meaning.

use serde_json::{Map, Value, json};

use crate::{AiError, Ctx, JsonRequest, ModelInfo, Provider, Request, Response, Result, Secret, clip, error_message, parse_json, send};

pub const API_ROOT: &str = "https://api.anthropic.com/v1";
pub const API_VERSION: &str = "2023-06-01";
/// The default model for the text features.
pub const DEFAULT_TEXT_MODEL: &str = "claude-opus-5-5";
/// The header that lets a request declined by a model's safety classifier be answered by another
/// model (`fallbacks: "default"`), on the models that take it.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

const P: Provider = Provider::Anthropic;

fn with_auth(req: Request, key: &Secret) -> Request {
    req.header("x-api-key", key.expose()).header("anthropic-version", API_VERSION)
}

/// Anthropic's error answer as an [`AiError`] (429 and 5xx were retried before this).
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

/// A schema structured outputs accept: every object closed (`additionalProperties: false`) with all
/// its properties required, and no numeric or length constraints (which the API rejects).
pub fn output_schema(schema: &Value) -> Value {
    match schema {
        Value::Array(a) => Value::Array(a.iter().map(output_schema).collect()),
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, v) in o {
                if matches!(
                    k.as_str(),
                    "minimum"
                        | "maximum"
                        | "exclusiveMinimum"
                        | "exclusiveMaximum"
                        | "multipleOf"
                        | "minLength"
                        | "maxLength"
                        | "minItems"
                        | "maxItems"
                        | "pattern"
                ) {
                    continue;
                }
                let v = match (k.as_str(), v) {
                    ("properties", Value::Object(props)) => Value::Object(props.iter().map(|(n, s)| (n.clone(), output_schema(s))).collect()),
                    _ => output_schema(v),
                };
                out.insert(k.clone(), v);
            }
            if o.get("type").and_then(Value::as_str) == Some("object") {
                let keys: Vec<Value> =
                    o.get("properties").and_then(Value::as_object).map(|p| p.keys().map(|k| Value::String(k.clone())).collect()).unwrap_or_default();
                out.insert("required".into(), Value::Array(keys));
                out.insert("additionalProperties".into(), Value::Bool(false));
            }
            Value::Object(out)
        }
        v => v.clone(),
    }
}

/// Whether `model` takes `fallbacks: "default"` (the current Opus, Fable and Sonnet 5.5 models).
fn takes_fallbacks(model: &str) -> bool {
    ["claude-opus-5", "claude-fable-5", "claude-sonnet-5-5"].iter().any(|m| model.starts_with(m))
}

/// The Messages request body.
pub fn body(model: &str, req: &JsonRequest) -> Value {
    let user = if req.user.is_empty() { "Answer as instructed." } else { req.user };
    let mut b = json!({
        "model": model,
        "max_tokens": 16000,
        "system": req.system,
        "messages": [{"role": "user", "content": user}],
    });
    match req.schema {
        Some(s) => b["output_config"] = json!({"format": {"type": "json_schema", "schema": output_schema(s)}}),
        None => b["system"] = json!(format!("{}\n\nAnswer with one JSON object only.", req.system)),
    }
    if takes_fallbacks(model) {
        b["fallbacks"] = json!("default");
    }
    b
}

/// A JSON answer from a Claude model.
pub fn generate_json(ctx: &Ctx, key: &Secret, model: &str, req: &JsonRequest) -> Result<Value> {
    let mut r = with_auth(Request::post(format!("{API_ROOT}/messages"), "application/json", body(model, req).to_string().into_bytes()), key);
    if takes_fallbacks(model) {
        r = r.header("anthropic-beta", FALLBACK_BETA);
    }
    let resp = send(ctx, P, &r)?;
    if !resp.ok() {
        return Err(error(model, &resp));
    }
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| AiError::BadResponse(P, clip(&String::from_utf8_lossy(&resp.body), 400)))?;
    match v.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => {
            let why = v.pointer("/stop_details/category").and_then(Value::as_str).unwrap_or("declined by the model's safety checks");
            return Err(AiError::Blocked(P, why.to_string()));
        }
        Some("max_tokens") => return Err(AiError::BadResponse(P, "the answer was cut off at its length limit".into())),
        _ => {}
    }
    let text: String = v
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    parse_json(P, &text)
}

/// The Claude models of this key (`GET /v1/models`), newest first as the API lists them.
pub fn list_models(ctx: &Ctx, key: &Secret) -> Result<Vec<ModelInfo>> {
    let r = send(ctx, P, &with_auth(Request::get(format!("{API_ROOT}/models?limit=1000")), key))?;
    if !r.ok() {
        return Err(error("", &r));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, "model list".into()))?;
    Ok(v.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str)?;
            let name = m.get("display_name").and_then(Value::as_str).unwrap_or(id);
            id.starts_with("claude").then(|| ModelInfo { id: id.to_string(), name: name.to_string() })
        })
        .collect())
}
