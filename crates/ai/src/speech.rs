//! Speech recognition of one window of audio by a cloud provider: Gemini (`generateContent` with
//! the audio inline and a response schema) or OpenAI (`/v1/audio/transcriptions`, `verbose_json`
//! with segment and word times). The prompt, schema and parsing follow Jilo Studio
//! (`plugins/ai/{gemini,openai,shared}.ts`). Times in the answer are seconds from the window's
//! first sample.

use serde_json::{Value, json};

use crate::{AiError, Ctx, Provider, Request, Result, Secret, clip, gemini, openai, send};

/// A recognised word.
#[derive(Clone, Debug, PartialEq)]
pub struct WordTime {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// A recognised phrase.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: Option<String>,
    pub words: Vec<WordTime>,
}

/// The answer for one window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recognised {
    pub segments: Vec<Segment>,
    pub language: Option<String>,
    /// Gemini stopped at its output limit: the window should be recognised again in halves.
    pub truncated: bool,
}

/// What to recognise.
#[derive(Clone, Copy, Debug)]
pub struct SpeechRequest<'a> {
    /// The window as a WAV file.
    pub wav: &'a [u8],
    /// Window length and its start in the whole recording, seconds (for the prompt).
    pub duration: f64,
    pub offset: f64,
    /// ISO 639-1 (`ru`, `tg`, `en`…) or `None` to detect.
    pub language: Option<&'a str>,
}

/// English names of languages for the prompts (models follow them better than codes).
pub fn language_name(code: &str) -> String {
    match code {
        "ru" => "Russian".into(),
        "tg" => "Tajik (Cyrillic script, as spoken in Tajikistan)".into(),
        "en" => "English".into(),
        "fa" => "Persian".into(),
        "uz" => "Uzbek".into(),
        other => other.to_string(),
    }
}

/// `m:ss` or `h:mm:ss`.
pub fn format_time(seconds: f64) -> String {
    let t = seconds.max(0.0).floor() as u64;
    let (h, m, s) = (t / 3600, t % 3600 / 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// The recognition prompt for one window (the same rules for any model).
pub fn subtitle_prompt(language: Option<&str>, offset: f64, duration: f64) -> String {
    let target = match language {
        None => "Detect the spoken language yourself and transcribe in that language, using its standard script.".to_string(),
        Some(l) => format!("The speech is in {}. Transcribe it in that language and script; do not translate it into any other language.", language_name(l)),
    };
    [
        "You are a professional subtitle transcriber working on an audio clip.".to_string(),
        target,
        String::new(),
        "Rules:".into(),
        "1. Transcribe verbatim what is actually said. Never invent, summarise or translate.".into(),
        "2. Split the speech into subtitle cues at natural phrase boundaries.".into(),
        "   Each cue: at most 7 seconds long and at most ~90 characters.".into(),
        "3. Timestamps are relative to the START of THIS audio clip, format MM:SS.mmm".into(),
        format!("   (this clip is {duration:.1} seconds long; never emit a timestamp beyond it)."),
        "4. Cues must be in chronological order and must not overlap.".into(),
        "5. Use normal punctuation and sentence case. No speaker labels inside \"text\".".into(),
        "6. If several people speak, put a short label in \"speaker\" (for example \"S1\").".into(),
        "7. Skip music, noise and silence. If there is no speech at all, return an empty list.".into(),
        "8. For every cue also list its words in \"words\", each with its own start and end".into(),
        "   inside the cue. Keep the words in spoken order; punctuation stays attached".into(),
        "   to the word it follows. The words must read exactly as \"text\" when joined.".into(),
        String::new(),
        format!("For reference, this clip starts at {} of the full recording,", format_time(offset)),
        "but all timestamps you output must still be relative to the clip start.".into(),
    ]
    .join("\n")
}

/// The response schema of Gemini recognition.
pub fn transcription_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "language": {"type": "string"},
            "segments": {"type": "array", "items": {"type": "object", "properties": {
                "start": {"type": "string"},
                "end": {"type": "string"},
                "text": {"type": "string"},
                "speaker": {"type": "string"},
                "words": {"type": "array", "items": {"type": "object", "properties": {
                    "text": {"type": "string"}, "start": {"type": "string"}, "end": {"type": "string"}
                }, "required": ["text", "start", "end"]}}
            }, "required": ["start", "end", "text"]}}
        },
        "required": ["language", "segments"]
    })
}

/// A timestamp in seconds from `62.5`, `"1:02.500"`, `"00:01:02"` or `"1:02,5"` (models write all
/// of them).
pub fn parse_timestamp(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return n.is_finite().then_some(n);
    }
    let s = v.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for p in parts {
        let n: f64 = p.replace(',', ".").trim().parse().ok()?;
        if !n.is_finite() {
            return None;
        }
        secs = secs * 60.0 + n;
    }
    Some(secs)
}

/// Recognise one window with Gemini `model`.
pub fn gemini_transcribe(ctx: &Ctx, key: &Secret, model: &str, req: &SpeechRequest) -> Result<Recognised> {
    let prompt = subtitle_prompt(req.language, req.offset, req.duration);
    let schema = transcription_schema();
    let g = gemini::generate(
        ctx,
        key,
        model,
        &[gemini::Part::Inline { mime: "audio/wav", data: req.wav }, gemini::Part::Text(&prompt)],
        &gemini::Config { temperature: 0.0, schema: Some(&schema) },
    )?;
    let truncated = g.truncated();
    if g.text.is_empty() {
        return Ok(Recognised { truncated, ..Default::default() });
    }
    let v: Value = match crate::parse_json(Provider::Gemini, &g.text) {
        Ok(v) => v,
        // a cut-off answer does not parse: the window is recognised again in halves
        Err(_) if truncated => return Ok(Recognised { truncated, ..Default::default() }),
        Err(e) => return Err(e),
    };
    let mut segments = Vec::new();
    for s in v.get("segments").and_then(Value::as_array).into_iter().flatten() {
        let (Some(start), Some(end)) = (s.get("start").and_then(parse_timestamp), s.get("end").and_then(parse_timestamp)) else { continue };
        let text = s.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if text.is_empty() || end <= start {
            continue;
        }
        let words = s
            .get("words")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|w| {
                let (ws, we) = (w.get("start").and_then(parse_timestamp)?, w.get("end").and_then(parse_timestamp)?);
                let t = w.get("text").and_then(Value::as_str)?.trim();
                (!t.is_empty()).then(|| WordTime { text: t.to_string(), start: ws, end: we.max(ws + 0.05) })
            })
            .collect();
        let speaker = s.get("speaker").and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
        segments.push(Segment { start, end, text, speaker, words });
    }
    let language = v.get("language").and_then(Value::as_str).map(str::to_string);
    Ok(Recognised { segments, language, truncated })
}

/// A `multipart/form-data` body: text fields, then one file.
pub fn multipart(fields: &[(&str, &str)], file_field: &str, file_name: &str, mime: &str, data: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----filmcraft-ai-7d1f3c9a5b2e4f60";
    let mut b = Vec::with_capacity(data.len() + 1024);
    for (k, v) in fields {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    b.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{file_field}\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes(),
    );
    b.extend_from_slice(data);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), b)
}

/// Recognise one window with OpenAI `model` (`whisper-1` gives segment and word times).
pub fn openai_transcribe(ctx: &Ctx, key: &Secret, model: &str, req: &SpeechRequest) -> Result<Recognised> {
    let mut fields =
        vec![("model", model), ("response_format", "verbose_json"), ("timestamp_granularities[]", "segment"), ("timestamp_granularities[]", "word")];
    if let Some(l) = req.language {
        fields.push(("language", l));
    }
    let (ctype, body) = multipart(&fields, "file", "chunk.wav", "audio/wav", req.wav);
    let r = Request::post(format!("{}/audio/transcriptions", openai::API_ROOT), &ctype, body).header("Authorization", &format!("Bearer {}", key.expose()));
    let resp = send(ctx, Provider::OpenAi, &r)?;
    if !resp.ok() {
        return Err(openai::error(model, &resp));
    }
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| AiError::BadResponse(Provider::OpenAi, clip(&String::from_utf8_lossy(&resp.body), 200)))?;
    let Some(segs) = v.get("segments").and_then(Value::as_array) else {
        return Err(AiError::BadResponse(Provider::OpenAi, format!("the model {model} gives no timestamps; choose whisper-1 for subtitles")));
    };
    let all_words: Vec<WordTime> = v
        .get("words")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|w| {
            let t = w.get("word").and_then(Value::as_str)?.trim();
            let (s, e) = (w.get("start")?.as_f64()?, w.get("end")?.as_f64()?);
            (!t.is_empty()).then(|| WordTime { text: t.to_string(), start: s, end: e })
        })
        .collect();
    let mut segments = Vec::new();
    for s in segs {
        let text = s.get("text").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        let (Some(start), Some(end)) = (s.get("start").and_then(Value::as_f64), s.get("end").and_then(Value::as_f64)) else { continue };
        if text.is_empty() || end <= start {
            continue;
        }
        // Whisper sometimes "hears" a silence: drop what it is sure is not speech
        if s.get("no_speech_prob").and_then(Value::as_f64).unwrap_or(0.0) > 0.8 {
            continue;
        }
        let words = all_words.iter().filter(|w| w.start >= start - 0.01 && w.end <= end + 0.01).cloned().collect();
        segments.push(Segment { start, end, text, speaker: None, words });
    }
    let language = v.get("language").and_then(Value::as_str).map(str::to_string);
    Ok(Recognised { segments, language, truncated: false })
}

/// Recognise one window with `provider`.
pub fn transcribe(ctx: &Ctx, provider: Provider, key: &Secret, model: &str, req: &SpeechRequest) -> Result<Recognised> {
    match provider {
        Provider::Gemini => gemini_transcribe(ctx, key, model, req),
        Provider::OpenAi => openai_transcribe(ctx, key, model, req),
    }
}

/// The longest window a provider takes (Gemini: 2 minutes, see `filmcraft_speech::chunked`;
/// OpenAI: 10 minutes, which as 16-bit mono 16 kHz WAV stays under its 25 MB upload limit).
pub fn max_window_seconds(provider: Provider) -> f64 {
    match provider {
        Provider::Gemini => 120.0,
        Provider::OpenAi => 600.0,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::transport::FakeTransport;
    use crate::{Response, Secret};

    fn ctx(f: &Arc<FakeTransport>) -> Ctx {
        Ctx::new(f.clone()).with_sleep(Arc::new(|_, _| {}))
    }

    fn req(language: Option<&str>) -> SpeechRequest<'_> {
        SpeechRequest { wav: b"RIFF....WAVE", duration: 126.0, offset: 117.0, language }
    }

    #[test]
    fn timestamps_in_every_spelling() {
        for (v, want) in [(json!(62.5), 62.5), (json!("1:02.500"), 62.5), (json!("00:01:02"), 62.0), (json!("1:02,5"), 62.5), (json!("7"), 7.0)] {
            assert_eq!(parse_timestamp(&v), Some(want), "{v}");
        }
        for v in [json!(""), json!("a:b"), json!("1:2:3:4"), json!(null), json!("inf")] {
            assert_eq!(parse_timestamp(&v), None, "{v}");
        }
    }

    #[test]
    fn the_prompt_names_tajik_and_the_window() {
        let p = subtitle_prompt(Some("tg"), 125.0, 126.0);
        assert!(p.contains("Tajik (Cyrillic script, as spoken in Tajikistan)"));
        assert!(p.contains("this clip is 126.0 seconds long"));
        assert!(p.contains("starts at 2:05 of the full recording"));
        assert!(subtitle_prompt(None, 0.0, 10.0).contains("Detect the spoken language"));
    }

    #[test]
    fn gemini_window_parses_cues_and_words() {
        let f = Arc::new(FakeTransport::new());
        let answer = json!({"language": "Russian", "segments": [
            {"start": "00:01.200", "end": "00:03.000", "text": "Привет, мир.", "speaker": "S1", "words": [
                {"text": "Привет,", "start": "00:01.200", "end": "00:01.700"}, {"text": "мир.", "start": "00:01.800", "end": "00:01.800"}]},
            {"start": "bad", "end": "00:04.000", "text": "skipped"},
            {"start": "00:05", "end": "00:04", "text": "backwards"}
        ]});
        f.push(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})));
        let r = transcribe(&ctx(&f), Provider::Gemini, &Secret::new("k"), "gemini-flash-latest", &req(Some("ru"))).unwrap();
        assert_eq!(r.segments.len(), 1);
        let s = &r.segments[0];
        assert_eq!((s.start, s.end, s.text.as_str(), s.speaker.as_deref()), (1.2, 3.0, "Привет, мир.", Some("S1")));
        assert_eq!(s.words.len(), 2);
        assert!((s.words[1].end - 1.85).abs() < 1e-9, "a zero-length word gets 50 ms");
        assert_eq!(r.language.as_deref(), Some("Russian"));
        let body = f.sent()[0].json().unwrap();
        assert_eq!(body["contents"][0]["parts"][0]["inline_data"]["mime_type"], "audio/wav");
        assert_eq!(body["generationConfig"]["temperature"], 0.0);
        assert!(body["contents"][0]["parts"][1]["text"].as_str().unwrap().contains("Russian"));
    }

    #[test]
    fn a_cut_off_gemini_answer_is_flagged_not_failed() {
        let f = Arc::new(FakeTransport::new());
        f.push(Response::json(
            200,
            &json!({"candidates": [{"content": {"parts": [{"text": "{\"segments\": [{\"start\": \"0"}]}, "finishReason": "MAX_TOKENS"}]}),
        ));
        let r = transcribe(&ctx(&f), Provider::Gemini, &Secret::new("k"), "m", &req(None)).unwrap();
        assert!(r.truncated && r.segments.is_empty());
    }

    #[test]
    fn openai_window_is_multipart_and_words_go_to_their_segments() {
        let f = Arc::new(FakeTransport::new());
        f.push(Response::json(
            200,
            &json!({"language": "tajik", "segments": [
                {"start": 0.0, "end": 2.0, "text": " Салом дӯстон", "no_speech_prob": 0.1},
                {"start": 2.0, "end": 3.0, "text": " шум", "no_speech_prob": 0.95}
            ], "words": [{"word": "Салом", "start": 0.1, "end": 0.6}, {"word": "дӯстон", "start": 0.7, "end": 1.4}, {"word": "шум", "start": 2.1, "end": 2.5}]}),
        ));
        let r = transcribe(&ctx(&f), Provider::OpenAi, &Secret::new("sk"), "whisper-1", &req(Some("tg"))).unwrap();
        assert_eq!(r.segments.len(), 1, "the no-speech segment is dropped");
        assert_eq!(r.segments[0].text, "Салом дӯстон");
        assert_eq!(r.segments[0].words.len(), 2);
        let sent = &f.sent()[0];
        assert_eq!(sent.url, "https://api.openai.com/v1/audio/transcriptions");
        assert!(sent.header_value("content-type").unwrap().starts_with("multipart/form-data; boundary="));
        let body = String::from_utf8_lossy(&sent.body);
        for part in [
            "name=\"model\"\r\n\r\nwhisper-1",
            "name=\"language\"\r\n\r\ntg",
            "name=\"timestamp_granularities[]\"\r\n\r\nword",
            "filename=\"chunk.wav\"",
            "RIFF....WAVE",
        ] {
            assert!(body.contains(part), "{part}");
        }
        // a model without timestamps
        f.push(Response::json(200, &json!({"text": "hello"})));
        let e = transcribe(&ctx(&f), Provider::OpenAi, &Secret::new("sk"), "gpt-4o-transcribe", &req(None)).unwrap_err();
        assert!(e.to_string().contains("whisper-1"), "{e}");
    }
}
