//! Image generation from a text prompt: OpenAI (`/v1/images/generations`, GPT Image) and Google
//! Gemini (`generateContent` with an image answer). The picture comes back as the provider's file
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
        other => Err(AiError::Api(other, format!("{} does not generate images here; choose OpenAI or Google Gemini", other.name()))),
    }
}
