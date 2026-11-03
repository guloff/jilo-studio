//! Image generation from a text prompt: OpenAI (`/v1/images/generations`, GPT Image), Google
//! Gemini (`generateContent` with an image answer) and a service at the user's own address that
//! answers as OpenAI does ([`generate_custom`]). The picture comes back as the provider's file
//! (PNG or JPEG) for the editor to import as a still.

use serde_json::{Value, json};

use crate::{AiError, Ctx, Provider, Request, Result, Secret, clip, gemini, openai, send};

pub const DEFAULT_OPENAI_MODEL: &str = "gpt-image-1";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash-image";

/// The shape of the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Landscape,
    Portrait,
    Square,
}

impl Shape {
    /// The shape nearest a frame of `w` × `h`.
    pub fn for_frame(w: u32, h: u32) -> Shape {
        let r = f64::from(w.max(1)) / f64::from(h.max(1));
        if r > 1.15 {
            Shape::Landscape
        } else if r < 0.87 {
            Shape::Portrait
        } else {
            Shape::Square
        }
    }

    pub fn from_id(s: &str) -> Option<Shape> {
        match s {
            "landscape" | "16:9" => Some(Shape::Landscape),
            "portrait" | "9:16" => Some(Shape::Portrait),
            "square" | "1:1" => Some(Shape::Square),
            _ => None,
        }
    }

    fn openai_size(self) -> &'static str {
        match self {
            Shape::Landscape => "1536x1024",
            Shape::Portrait => "1024x1536",
            Shape::Square => "1024x1024",
        }
    }

    fn gemini_aspect(self) -> &'static str {
        match self {
            Shape::Landscape => "16:9",
            Shape::Portrait => "9:16",
            Shape::Square => "1:1",
        }
    }
}

/// A generated picture: the file's bytes and its type (`png`, `jpg`, `webp`).
#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub data: Vec<u8>,
    pub extension: &'static str,
}

/// The file type of image bytes, from their signature.
pub fn extension_of(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if data.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn picture(provider: Provider, b64: &str) -> Result<Picture> {
    let data = crate::base64::decode(b64).ok_or_else(|| AiError::BadResponse(provider, "the picture could not be decoded".into()))?;
    let extension = extension_of(&data).ok_or_else(|| AiError::BadResponse(provider, "the answer is not a picture".into()))?;
    Ok(Picture { data, extension })
}

/// Generate one picture of `prompt` with `provider` (OpenAI or Gemini) and `model`.
pub fn generate(ctx: &Ctx, provider: Provider, key: &Secret, model: &str, prompt: &str, shape: Shape) -> Result<Picture> {
    match provider {
        Provider::OpenAi => {
            let body = json!({"model": model, "prompt": prompt, "size": shape.openai_size(), "n": 1}).to_string().into_bytes();
            let r = Request::post(format!("{}/images/generations", openai::API_ROOT), "application/json", body)
                .header("Authorization", &format!("Bearer {}", key.expose()));
            let resp = send(ctx, provider, &r)?;
            if !resp.ok() {
                return Err(openai::error(model, &resp));
            }
            let v: Value = serde_json::from_slice(&resp.body).map_err(|_| AiError::BadResponse(provider, clip(&String::from_utf8_lossy(&resp.body), 200)))?;
            let b64 = v.pointer("/data/0/b64_json").and_then(Value::as_str).ok_or_else(|| AiError::BadResponse(provider, "no picture in the answer".into()))?;
            picture(provider, b64)
        }
        Provider::Gemini => {
            let body = json!({
                "contents": [{"role": "user", "parts": [{"text": prompt}]}],
                "generationConfig": {"responseModalities": ["IMAGE"], "imageConfig": {"aspectRatio": shape.gemini_aspect()}},
            })
            .to_string()
            .into_bytes();
            let url = format!("{}/models/{}:generateContent", gemini::API_ROOT, model.replace(['/', '?', '#'], ""));
            let resp = send(ctx, provider, &Request::post(url, "application/json", body).header("x-goog-api-key", key.expose()))?;
            if !resp.ok() {
                return Err(gemini::error(model, &resp));
            }
            let v: Value = serde_json::from_slice(&resp.body).map_err(|_| AiError::BadResponse(provider, clip(&String::from_utf8_lossy(&resp.body), 200)))?;
            if let Some(why) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
                return Err(AiError::Blocked(provider, why.to_string()));
            }
            let parts = v.pointer("/candidates/0/content/parts").and_then(Value::as_array).cloned().unwrap_or_default();
            let data = parts.iter().find_map(|p| p.pointer("/inlineData/data").or_else(|| p.pointer("/inline_data/data")).and_then(Value::as_str));
            match data {
                Some(b64) => picture(provider, b64),
                None => {
                    let reason = v.pointer("/candidates/0/finishReason").and_then(Value::as_str).unwrap_or("no picture in the answer");
                    Err(AiError::Blocked(provider, reason.to_string()))
                }
            }
        }
        Provider::Custom => Err(AiError::Api(Provider::Custom, "the custom image service needs its address: use generate_custom".into())),
        other => Err(AiError::Api(other, format!("{} does not generate images here; choose OpenAI, Google Gemini or a custom service", other.name()))),
    }
}

/// Whether `url` may receive a description and a key: HTTPS anywhere, plain HTTP only to this
/// computer (a local server). The address without a trailing slash, or why it is refused.
pub fn custom_base(url: &str) -> std::result::Result<String, String> {
    checked_url(url.trim().trim_end_matches('/'), false)
}

/// `u` if it is HTTPS, or plain HTTP to this computer; a service address takes no query
/// (`query`: a picture's address may, signed CDN links carry one).
fn checked_url(u: &str, query: bool) -> std::result::Result<String, String> {
    if u.is_empty() {
        return Err("no address: set the custom image service's address in Settings ▸ AI".into());
    }
    let rest = if let Some(r) = u.strip_prefix("https://") {
        r
    } else if let Some(r) = u.strip_prefix("http://") {
        let host = r.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        let name = if host.starts_with('[') {
            host.split(']').next().map(|h| format!("{h}]")).unwrap_or_default()
        } else {
            host.split(':').next().unwrap_or("").to_string()
        };
        if !matches!(name.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "[::1]") {
            return Err(format!("{u}: use https:// (plain http only to this computer)"));
        }
        r
    } else {
        return Err(format!("{u}: the address starts with https://"));
    };
    if rest.is_empty() || rest.starts_with(['/', '?', '#']) || rest.contains(char::is_whitespace) || (!query && u.contains(['?', '#'])) {
        return Err(format!("{u}: not a service address"));
    }
    Ok(u.to_string())
}

/// The host of a custom service's address (for the user's consent, asked once per host).
pub fn custom_host(url: &str) -> String {
    let u = url.trim();
    let rest = u.split_once("://").map_or(u, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    host.rsplit_once('@').map_or(host, |(_, h)| h).to_ascii_lowercase()
}

/// Generate one picture with a service at `base` (e.g. `https://api.example.com/v1`) that serves
/// OpenAI's `POST /images/generations`: the picture in the answer (`b64_json`), or at an HTTPS
/// address in it (`url`), which is then fetched. `key` is sent as a bearer token when there is one.
pub fn generate_custom(ctx: &Ctx, base: &str, key: Option<&Secret>, model: &str, prompt: &str, shape: Shape) -> Result<Picture> {
    let p = Provider::Custom;
    let base = custom_base(base).map_err(|e| AiError::Api(p, e))?;
    let body = json!({"model": model, "prompt": prompt, "size": shape.openai_size(), "n": 1}).to_string().into_bytes();
    let mut r = Request::post(format!("{base}/images/generations"), "application/json", body);
    if let Some(k) = key {
        r = r.header("Authorization", &format!("Bearer {}", k.expose()));
    }
    let resp = send(ctx, p, &r)?;
    if !resp.ok() {
        return Err(openai::error_for(p, model, &resp));
    }
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| AiError::BadResponse(p, clip(&String::from_utf8_lossy(&resp.body), 200)))?;
    if let Some(b64) = v.pointer("/data/0/b64_json").and_then(Value::as_str) {
        return picture(p, b64);
    }
    let url = v.pointer("/data/0/url").and_then(Value::as_str).ok_or_else(|| AiError::BadResponse(p, "no picture in the answer".into()))?;
    // the picture's own address: HTTPS only (or this computer), and no key with it
    let url = checked_url(url.trim(), true).map_err(|e| AiError::BadResponse(p, e))?;
    let got = send(ctx, p, &Request::get(url))?;
    if !got.ok() {
        return Err(openai::error_for(p, model, &got));
    }
    let extension = extension_of(&got.body).ok_or_else(|| AiError::BadResponse(p, "the answer is not a picture".into()))?;
    Ok(Picture { data: got.body, extension })
}
