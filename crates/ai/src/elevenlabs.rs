//! ElevenLabs (`api.elevenlabs.io/v1`): speech recognition (Scribe, `/speech-to-text`, with a time
//! for every word and speaker labels), the account's voices (`/voices`) and speech synthesis
//! (`/text-to-speech/{voice}`, raw 16-bit PCM), and ElevenLabs' error answers.

use serde_json::{Value, json};

use crate::speech::{Recognised, Segment, SpeechRequest, WordTime};
use crate::{AiError, Ctx, ModelInfo, Provider, Request, Response, Result, Secret, clip, send};

pub const API_ROOT: &str = "https://api.elevenlabs.io/v1";
/// Speech recognition with word times and speakers.
pub const DEFAULT_SPEECH_MODEL: &str = "scribe_v1";
/// Speech synthesis in many languages (Russian, English, …).
pub const DEFAULT_VOICE_MODEL: &str = "eleven_multilingual_v2";
/// The sample rate of synthesised speech (`output_format=pcm_44100`: 16-bit little-endian mono).
pub const VOICE_SAMPLE_RATE: u32 = 44_100;

const P: Provider = Provider::ElevenLabs;

fn with_key(req: Request, key: &Secret) -> Request {
    req.header("xi-api-key", key.expose())
}

/// ElevenLabs' error message: `detail.message`, `detail` as text, or the status line.
fn message(r: &Response) -> String {
    serde_json::from_slice::<Value>(&r.body)
        .ok()
        .and_then(|v| {
            v.pointer("/detail/message").and_then(Value::as_str).map(str::to_string).or_else(|| v.get("detail").and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_else(|| format!("HTTP {}", r.status))
}

/// ElevenLabs' error answer as an [`AiError`] (429 and 5xx were retried before this).
pub fn error(model: &str, r: &Response) -> AiError {
    let msg = message(r);
    match r.status {
        401 => AiError::BadApiKey(P),
        403 => AiError::Forbidden(P, msg),
        404 => AiError::ModelNotFound(P, model.to_string()),
        422 if msg.to_ascii_lowercase().contains("model") => AiError::ModelNotFound(P, model.to_string()),
        429 => AiError::RateLimited(P),
        s if s >= 500 => AiError::Unavailable(P, msg),
        _ => AiError::Api(P, msg),
    }
}

/// Phrases from Scribe's words: a new phrase after a pause over 0.7 s, a change of speaker, at the
/// end of a sentence once the phrase has 15 characters, or at 7 seconds (as the other engines'
/// subtitle cues).
pub fn phrases(words: &[(WordTime, Option<String>)]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for (w, speaker) in words {
        let start_new = match out.last() {
            None => true,
            Some(s) => {
                let gap = w.start - s.end;
                let sentence_end = s.text.ends_with(['.', '!', '?', '…']) && s.text.chars().count() >= 15;
                gap > 0.7 || *speaker != s.speaker || sentence_end || w.end - s.start > 7.0
            }
        };
        if start_new {
            out.push(Segment { start: w.start, end: w.end, text: w.text.clone(), speaker: speaker.clone(), words: vec![w.clone()] });
        } else if let Some(s) = out.last_mut() {
            s.text.push(' ');
            s.text.push_str(&w.text);
            s.end = w.end;
            s.words.push(w.clone());
        }
    }
    out
}

/// Recognise one window with Scribe (`model`), words timed and speakers labelled.
pub fn transcribe(ctx: &Ctx, key: &Secret, model: &str, req: &SpeechRequest) -> Result<Recognised> {
    let mut fields = vec![("model_id", model), ("timestamps_granularity", "word"), ("diarize", "true"), ("tag_audio_events", "false")];
    if let Some(l) = req.language {
        fields.push(("language_code", l));
    }
    let (ctype, body) = crate::speech::multipart(&fields, "file", "chunk.wav", "audio/wav", req.wav);
    let resp = send(ctx, P, &with_key(Request::post(format!("{API_ROOT}/speech-to-text"), &ctype, body), key))?;
    if !resp.ok() {
        return Err(error(model, &resp));
    }
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| AiError::BadResponse(P, clip(&String::from_utf8_lossy(&resp.body), 200)))?;
    let words: Vec<(WordTime, Option<String>)> = v
        .get("words")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|w| w.get("type").and_then(Value::as_str).is_none_or(|t| t == "word"))
        .filter_map(|w| {
            let text = w.get("text").and_then(Value::as_str)?.trim();
            let (start, end) = (w.get("start")?.as_f64()?, w.get("end")?.as_f64()?);
            let speaker = w.get("speaker_id").and_then(Value::as_str).map(|s| s.replace("speaker_", "S"));
            (!text.is_empty() && start.is_finite() && end >= start).then(|| (WordTime { text: text.to_string(), start, end }, speaker))
        })
        .collect();
    let language = v.get("language_code").and_then(Value::as_str).map(|l| l.chars().take(2).collect());
    Ok(Recognised { segments: phrases(&words), language, truncated: false })
}

/// A voice of the account.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// "female", "male"… when ElevenLabs labels it.
    pub gender: Option<String>,
    /// ISO 639-1 when the voice is labelled with one language; None = multilingual.
    pub language: Option<String>,
}

/// The voices of the account (premade ones and the user's own).
pub fn voices(ctx: &Ctx, key: &Secret) -> Result<Vec<Voice>> {
    let r = send(ctx, P, &with_key(Request::get(format!("{API_ROOT}/voices")), key))?;
    if !r.ok() {
        return Err(error("", &r));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, "voice list".into()))?;
    Ok(v.get("voices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let id = x.get("voice_id").and_then(Value::as_str)?;
            let name = x.get("name").and_then(Value::as_str).unwrap_or(id);
            let label = |k: &str| x.pointer(&format!("/labels/{k}")).and_then(Value::as_str).map(str::to_string);
            Some(Voice { id: id.to_string(), name: name.to_string(), gender: label("gender"), language: label("language") })
        })
        .collect())
}

/// Speak `text` with `voice` (`model`): 16-bit little-endian mono PCM at [`VOICE_SAMPLE_RATE`].
pub fn speak(ctx: &Ctx, key: &Secret, model: &str, voice: &str, text: &str) -> Result<Vec<i16>> {
    let voice: String = voice.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if voice.is_empty() {
        return Err(AiError::Api(P, "no voice chosen".into()));
    }
    let url = format!("{API_ROOT}/text-to-speech/{voice}?output_format=pcm_44100");
    let body = json!({"text": text, "model_id": model}).to_string().into_bytes();
    let r = send(ctx, P, &with_key(Request::post(url, "application/json", body), key).header("Accept", "audio/pcm"))?;
    if !r.ok() {
        return Err(error(model, &r));
    }
    if r.body.len() % 2 != 0 || r.body.first() == Some(&b'{') {
        return Err(AiError::BadResponse(P, clip(&String::from_utf8_lossy(&r.body), 200)));
    }
    Ok(r.body.as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)).collect())
}

/// The account's models (a key check): speech synthesis and recognition models.
pub fn list_models(ctx: &Ctx, key: &Secret) -> Result<Vec<ModelInfo>> {
    let r = send(ctx, P, &with_key(Request::get(format!("{API_ROOT}/models")), key))?;
    if !r.ok() {
        return Err(error("", &r));
    }
    let v: Value = serde_json::from_slice(&r.body).map_err(|_| AiError::BadResponse(P, "model list".into()))?;
    let mut out: Vec<ModelInfo> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("model_id").and_then(Value::as_str)?;
            Some(ModelInfo { id: id.to_string(), name: m.get("name").and_then(Value::as_str).unwrap_or(id).to_string() })
        })
        .collect();
    if !out.iter().any(|m| m.id == DEFAULT_SPEECH_MODEL) {
        out.push(ModelInfo { id: DEFAULT_SPEECH_MODEL.into(), name: "Scribe (speech to text)".into() });
    }
    Ok(out)
}
