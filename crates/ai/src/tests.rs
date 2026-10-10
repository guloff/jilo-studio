//! Provider tests over the scripted transport: no network.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::transport::FakeTransport;
use crate::*;

/// A context over `fake` whose waits are recorded instead of slept.
fn ctx(fake: &Arc<FakeTransport>) -> (Ctx, Arc<Mutex<Vec<Duration>>>) {
    let waits: Arc<Mutex<Vec<Duration>>> = Default::default();
    let w = waits.clone();
    let c = Ctx::new(fake.clone()).with_sleep(Arc::new(move |d, _| w.lock().unwrap().push(d)));
    (c, waits)
}

fn key() -> Secret {
    Secret::new("test-key-123")
}

fn gemini_answer(text: &str, finish: &str) -> Response {
    Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": text}]}, "finishReason": finish}]}))
}

fn req<'a>(schema: Option<&'a Value>) -> JsonRequest<'a> {
    JsonRequest { system: "Translate.", user: r#"{"items":[]}"#, schema, temperature: 0.2 }
}

#[test]
fn gemini_json_request_shape_and_answer() {
    let fake = Arc::new(FakeTransport::new());
    fake.push(gemini_answer("```json\n{\"items\": [{\"id\": \"1\", \"text\": \"Привет\"}]}\n```", "STOP"));
    let (c, _) = ctx(&fake);
    let schema = json!({"type": "object", "properties": {"items": {"type": "array"}}});
    let a = generate_json(&c, Provider::Gemini, &key(), "gemini-flash-latest", &req(Some(&schema))).unwrap();
    assert_eq!(a.value["items"][0]["text"], "Привет");
    assert_eq!(a.model, "gemini-flash-latest");
    let sent = fake.sent();
    assert_eq!(sent.len(), 1);
    let r = &sent[0];
    assert_eq!(r.method, "POST");
    assert_eq!(r.url, "https://generativelanguage.googleapis.com/v1beta/models/gemini-flash-latest:generateContent");
    assert_eq!(r.header_value("x-goog-api-key"), Some("test-key-123"));
    assert!(!r.url.contains("test-key"), "the key goes in a header, never the URL");
    let b = r.json().unwrap();
    assert_eq!(b["contents"][0]["parts"][0]["text"], "Translate.");
    assert_eq!(b["contents"][0]["parts"][1]["text"], r#"{"items":[]}"#);
    assert_eq!(b["generationConfig"]["responseMimeType"], "application/json");
    assert_eq!(b["generationConfig"]["responseSchema"], schema);
    assert!((b["generationConfig"]["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
}

#[test]
fn gemini_inline_audio_is_base64() {
    let fake = Arc::new(FakeTransport::new());
    fake.push(gemini_answer("{}", "MAX_TOKENS"));
    let (c, _) = ctx(&fake);
    let g = gemini::generate(
        &c,
        &key(),
        "m",
        &[gemini::Part::Inline { mime: "audio/wav", data: b"foo" }, gemini::Part::Text("hi")],
        &gemini::Config { temperature: 0.0, schema: None },
    )
    .unwrap();
    assert!(g.truncated());
    let b = fake.sent()[0].json().unwrap();
    assert_eq!(b["contents"][0]["parts"][0]["inline_data"], json!({"mime_type": "audio/wav", "data": "Zm9v"}));
}

#[test]
fn retries_wait_like_jilo_then_succeed() {
    let fake = Arc::new(FakeTransport::new());
    fake.push_error("connection reset").push(Response::json(503, &json!({"error": {"message": "overloaded"}}))).push(gemini_answer("{\"ok\": true}", "STOP"));
    let (c, waits) = ctx(&fake);
    let a = generate_json(&c, Provider::Gemini, &key(), "m", &req(None)).unwrap();
    assert_eq!(a.value, json!({"ok": true}));
    // network failure: 1 s·2⁰; server error: 1.5 s·2¹
    assert_eq!(*waits.lock().unwrap(), vec![Duration::from_millis(1000), Duration::from_millis(3000)]);
}

#[test]
fn rate_limit_gives_up_after_three_attempts() {
    let fake = Arc::new(FakeTransport::new());
    for _ in 0..3 {
        fake.push(Response::json(429, &json!({"error": {"message": "quota"}})));
    }
    let (c, waits) = ctx(&fake);
    let e = generate_json(&c, Provider::OpenAi, &key(), "gpt-4o-mini", &req(None)).unwrap_err();
    assert_eq!(e, AiError::RateLimited(Provider::OpenAi));
    assert_eq!(e.code(), "RATE_LIMIT");
    assert_eq!(fake.sent().len(), 3);
    // 2 s·2⁰, 2 s·2¹; no wait after the last attempt
    assert_eq!(*waits.lock().unwrap(), vec![Duration::from_millis(2000), Duration::from_millis(4000)]);
}

#[test]
fn cancel_stops_before_the_next_attempt() {
    let fake = Arc::new(FakeTransport::new());
    fake.push_error("down").push(gemini_answer("{}", "STOP"));
    let cancel = Arc::new(AtomicBool::new(false));
    let (flag, seen) = (cancel.clone(), cancel.clone());
    // the user presses Stop while FilmCraft waits to retry
    let c = Ctx::new(fake.clone()).with_cancel(move || seen.load(Ordering::Relaxed)).with_sleep(Arc::new(move |_, _| flag.store(true, Ordering::Relaxed)));
    assert_eq!(generate_json(&c, Provider::Gemini, &key(), "m", &req(None)).unwrap_err(), AiError::Cancelled);
    assert_eq!(fake.sent().len(), 1);
}

#[test]
fn gemini_errors_map_to_codes() {
    let cases = [
        (400, "API key not valid. Please pass a valid API key.", "BAD_API_KEY"),
        (403, "Permission denied", "FORBIDDEN"),
        (404, "models/gemini-9 is not found", "MODEL_NOT_FOUND"),
        (400, "Invalid JSON payload", "API_ERROR"),
    ];
    for (status, msg, code) in cases {
        let fake = Arc::new(FakeTransport::new());
        fake.push(Response::json(status, &json!({"error": {"code": status, "message": msg}})));
        let (c, _) = ctx(&fake);
        let e = generate_json(&c, Provider::Gemini, &key(), "gemini-9", &req(None)).unwrap_err();
        assert_eq!(e.code(), code, "{status} {msg}");
        assert!(!e.to_string().contains("test-key"));
    }
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(200, &json!({"promptFeedback": {"blockReason": "SAFETY"}})));
    let (c, _) = ctx(&fake);
    assert_eq!(generate_json(&c, Provider::Gemini, &key(), "m", &req(None)).unwrap_err(), AiError::Blocked(Provider::Gemini, "SAFETY".into()));
    let fake = Arc::new(FakeTransport::new());
    fake.push(gemini_answer("not json at all", "STOP"));
    let (c, _) = ctx(&fake);
    assert_eq!(generate_json(&c, Provider::Gemini, &key(), "m", &req(None)).unwrap_err().code(), "BAD_RESPONSE");
}

#[test]
fn retired_model_switches_to_the_named_replacement_once() {
    let fake = Arc::new(FakeTransport::new());
    let retired = "Gemini 1.5 Flash is no longer available. Please use models/gemini-2.5-flash for the latest features.";
    fake.push(Response::json(404, &json!({"error": {"message": retired}}))).push(gemini_answer("{\"n\": 1}", "STOP"));
    let (c, _) = ctx(&fake);
    let a = generate_json(&c, Provider::Gemini, &key(), "gemini-1.5-flash", &req(None)).unwrap();
    assert_eq!((a.value, a.model.as_str()), (json!({"n": 1}), "gemini-2.5-flash"));
    let sent = fake.sent();
    assert!(sent[1].url.contains("/models/gemini-2.5-flash:generateContent"));
    // without a named replacement the error stays
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(404, &json!({"error": {"message": "This model is deprecated."}})));
    let (c, _) = ctx(&fake);
    let e = generate_json(&c, Provider::Gemini, &key(), "old", &req(None)).unwrap_err();
    assert_eq!(e, AiError::ModelRetired { provider: Provider::Gemini, model: "old".into(), replacement: None });
    assert_eq!(gemini::replacement("x models/a for nothing; use models/gemini-flash-latest  for the latest"), Some("gemini-flash-latest".into()));
}

#[test]
fn gemini_model_list_filters_and_puts_aliases_first() {
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(
        200,
        &json!({"models": [
            {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/gemini-flash-latest", "displayName": "Gemini Flash Latest", "supportedGenerationMethods": ["generateContent", "countTokens"]},
            {"name": "models/gemini-embedding-001", "supportedGenerationMethods": ["embedContent"]},
            {"name": "models/gemini-2.5-flash-preview-tts", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/imagen-4", "supportedGenerationMethods": ["generateContent"]}
        ]}),
    ));
    let (c, _) = ctx(&fake);
    let (msg, models) = test_key(&c, Provider::Gemini, &key()).unwrap();
    assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gemini-flash-latest", "gemini-2.5-pro"]);
    assert_eq!(msg, "Key accepted, 2 models available");
    assert_eq!(fake.sent()[0].method, "GET");
}

#[test]
fn openai_strict_schema_and_chat_request() {
    let schema = json!({"type": "object", "properties": {
        "fragments": {"type": "array", "items": {"type": "object", "properties": {"start": {"type": "number"}, "title": {"type": "string"}}, "required": ["start"]}}
    }});
    let strict = openai::strict_schema(&schema);
    assert_eq!(strict["required"], json!(["fragments"]));
    assert_eq!(strict["additionalProperties"], json!(false));
    let item = &strict["properties"]["fragments"]["items"];
    assert_eq!(item["required"], json!(["start", "title"]));
    assert_eq!(item["additionalProperties"], json!(false));

    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(200, &json!({"choices": [{"message": {"content": "{\"fragments\": []}"}}]})));
    let (c, _) = ctx(&fake);
    let a = generate_json(&c, Provider::OpenAi, &key(), "gpt-4o-mini", &req(Some(&schema))).unwrap();
    assert_eq!(a.value, json!({"fragments": []}));
    let r = &fake.sent()[0];
    assert_eq!(r.url, "https://api.openai.com/v1/chat/completions");
    assert_eq!(r.header_value("authorization"), Some("Bearer test-key-123"));
    let b = r.json().unwrap();
    assert_eq!(b["messages"][0], json!({"role": "system", "content": "Translate."}));
    assert_eq!(b["response_format"]["type"], "json_schema");
    assert_eq!(b["response_format"]["json_schema"]["strict"], true);
    assert!(b.get("temperature").is_none(), "OpenAI uses the model's default temperature");
    // a request with secrets prints without them
    assert!(!format!("{r:?}").contains("test-key"));
}

#[test]
fn openai_errors_and_model_lists() {
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(401, &json!({"error": {"message": "Incorrect API key provided: sk-…123"}})));
    let (c, _) = ctx(&fake);
    assert_eq!(test_key(&c, Provider::OpenAi, &key()).unwrap_err(), AiError::BadApiKey(Provider::OpenAi));

    let models = json!({"data": [{"id": "gpt-4o-mini"}, {"id": "whisper-1"}, {"id": "gpt-4o-transcribe"}, {"id": "o3-mini"}, {"id": "gpt-4o-realtime-preview"}, {"id": "text-embedding-3-small"}, {"id": "dall-e-3"}, {"id": "omni-moderation-latest"}]});
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(200, &models)).push(Response::json(200, &models));
    let (c, _) = ctx(&fake);
    let text: Vec<String> = list_models(&c, Provider::OpenAi, &key(), ModelUse::Text).unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(text, ["gpt-4o-mini", "o3-mini"]);
    let speech = list_models(&c, Provider::OpenAi, &key(), ModelUse::Speech).unwrap();
    assert_eq!(speech.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-4o-transcribe", "whisper-1"]);
    assert_eq!(speech[1].name, "whisper-1 (word times)");
}

#[test]
fn providers_round_trip_ids() {
    for p in Provider::ALL {
        assert_eq!(Provider::from_id(p.id()), Some(p));
        assert_eq!(serde_json::to_value(p).unwrap(), json!(p.id()));
    }
    assert_eq!(Provider::OpenAi.default_speech_model(), "whisper-1");
    assert_eq!(Provider::Gemini.default_text_model(), "gemini-flash-latest");
}

#[test]
fn claude_answers_json_through_structured_outputs() {
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(200, &json!({"model": "claude-opus-5-5", "stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": "{\"items\": [{\"id\": \"1\", \"text\": \"Салом\"}]}"}]})));
    let (c, _) = ctx(&fake);
    let schema = json!({"type": "object", "properties": {"items": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {"id": {"type": "string"}, "n": {"type": "integer", "minimum": 0}}}}}});
    let a = generate_json(&c, Provider::Anthropic, &key(), anthropic::DEFAULT_TEXT_MODEL, &req(Some(&schema))).unwrap();
    assert_eq!(a.value["items"][0]["text"], "Салом");
    let sent = fake.sent();
    let r = &sent[0];
    assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
    assert_eq!(r.header_value("x-api-key"), Some("test-key-123"));
    assert_eq!(r.header_value("anthropic-version"), Some("2023-06-01"));
    assert_eq!(r.header_value("anthropic-beta"), Some("server-side-fallback-2026-07-01"));
    let b = r.json().unwrap();
    assert_eq!(b["model"], "claude-opus-5-5");
    assert_eq!(b["fallbacks"], "default");
    assert_eq!(b["system"], "Translate.");
    assert!(b.get("temperature").is_none(), "Opus 5.5 takes no sampling parameters");
    let s = &b["output_config"]["format"]["schema"];
    assert_eq!(b["output_config"]["format"]["type"], "json_schema");
    assert_eq!(s["additionalProperties"], false);
    assert!(s["properties"]["items"].get("minItems").is_none(), "no length constraints");
    assert!(s["properties"]["items"]["items"]["properties"]["n"].get("minimum").is_none(), "no numeric constraints");
    assert_eq!(s["properties"]["items"]["items"]["required"], json!(["id", "n"]));
    // a model without fallbacks gets neither the parameter nor the header
    fake.push(Response::json(200, &json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "{}"}]})));
    generate_json(&c, Provider::Anthropic, &key(), "claude-haiku-5-5", &req(None)).unwrap();
    let r = &fake.sent()[1];
    assert!(r.header_value("anthropic-beta").is_none() && r.json().unwrap().get("fallbacks").is_none());
}

#[test]
fn claude_refusals_cut_answers_and_errors() {
    let fake = Arc::new(FakeTransport::new());
    let (c, _) = ctx(&fake);
    fake.push(Response::json(200, &json!({"stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber"}, "content": []})));
    assert!(
        matches!(generate_json(&c, Provider::Anthropic, &key(), "claude-opus-5-5", &req(None)), Err(AiError::Blocked(Provider::Anthropic, w)) if w == "cyber")
    );
    fake.push(Response::json(200, &json!({"stop_reason": "max_tokens", "content": [{"type": "text", "text": "{\"a\":"}]})));
    assert!(matches!(generate_json(&c, Provider::Anthropic, &key(), "claude-opus-5-5", &req(None)), Err(AiError::BadResponse(..))));
    fake.push(Response::json(401, &json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}})));
    assert_eq!(generate_json(&c, Provider::Anthropic, &key(), "claude-opus-5-5", &req(None)).unwrap_err(), AiError::BadApiKey(Provider::Anthropic));
    fake.push(Response::json(404, &json!({"error": {"message": "model: nope"}})));
    assert!(matches!(generate_json(&c, Provider::Anthropic, &key(), "nope", &req(None)), Err(AiError::ModelNotFound(..))));
    // the model list of a key check
    fake.push(Response::json(
        200,
        &json!({"data": [{"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5"}, {"id": "claude-haiku-5-5", "display_name": "Claude Haiku 5.5"}]}),
    ));
    let (msg, models) = test_key(&c, Provider::Anthropic, &key()).unwrap();
    assert_eq!(models.len(), 2, "{msg}");
    assert!(fake.sent().last().unwrap().url.starts_with("https://api.anthropic.com/v1/models"));
}

#[test]
fn elevenlabs_scribe_words_become_timed_phrases() {
    let fake = Arc::new(FakeTransport::new());
    fake.push(Response::json(
        200,
        &json!({
            "language_code": "rus",
            "text": "Привет мир. Как дела у вас?",
            "words": [
                {"text": "Привет", "start": 0.1, "end": 0.5, "type": "word", "speaker_id": "speaker_0"},
                {"text": " ", "start": 0.5, "end": 0.55, "type": "spacing"},
                {"text": "мир.", "start": 0.55, "end": 0.9, "type": "word", "speaker_id": "speaker_0"},
                {"text": "Как", "start": 2.0, "end": 2.2, "type": "word", "speaker_id": "speaker_1"},
                {"text": "дела", "start": 2.2, "end": 2.5, "type": "word", "speaker_id": "speaker_1"},
                {"text": "(смех)", "start": 2.5, "end": 2.6, "type": "audio_event"},
                {"text": "у", "start": 2.6, "end": 2.7, "type": "word", "speaker_id": "speaker_1"},
                {"text": "вас?", "start": 2.7, "end": 3.0, "type": "word", "speaker_id": "speaker_1"}
            ]
        }),
    ));
    let (c, _) = ctx(&fake);
    let r = speech::transcribe(
        &c,
        Provider::ElevenLabs,
        &key(),
        elevenlabs::DEFAULT_SPEECH_MODEL,
        &speech::SpeechRequest { wav: b"RIFF", duration: 3.0, offset: 0.0, language: Some("ru") },
    )
    .unwrap();
    assert_eq!(r.language.as_deref(), Some("ru"));
    assert_eq!(r.segments.len(), 2, "{:?}", r.segments);
    assert_eq!(r.segments[0].text, "Привет мир.");
    assert_eq!(r.segments[1].text, "Как дела у вас?");
    assert_eq!(r.segments[1].speaker.as_deref(), Some("S1"));
    assert_eq!(r.segments[1].words.len(), 4, "spacing and audio events are not words");
    let sent = &fake.sent()[0];
    assert_eq!(sent.url, "https://api.elevenlabs.io/v1/speech-to-text");
    assert_eq!(sent.header_value("xi-api-key"), Some("test-key-123"));
    let body = String::from_utf8_lossy(&sent.body);
    assert!(
        body.contains("name=\"model_id\"\r\n\r\nscribe_v1")
            && body.contains("name=\"language_code\"\r\n\r\nru")
            && body.contains("name=\"timestamps_granularity\"\r\n\r\nword")
    );
    fake.push(Response::json(401, &json!({"detail": {"status": "invalid_api_key", "message": "Invalid API key"}})));
    assert_eq!(
        speech::transcribe(&c, Provider::ElevenLabs, &key(), "scribe_v1", &speech::SpeechRequest { wav: b"RIFF", duration: 3.0, offset: 0.0, language: None })
            .unwrap_err(),
        AiError::BadApiKey(Provider::ElevenLabs)
    );
    // Claude does not recognise speech
    assert!(
        speech::transcribe(&c, Provider::Anthropic, &key(), "", &speech::SpeechRequest { wav: b"RIFF", duration: 1.0, offset: 0.0, language: None }).is_err()
    );
}

#[test]
fn elevenlabs_voices_and_speech() {
    let fake = Arc::new(FakeTransport::new());
    let (c, _) = ctx(&fake);
    fake.push(Response::json(
        200,
        &json!({"voices": [{"voice_id": "abc123", "name": "Rachel", "labels": {"gender": "female"}}, {"voice_id": "x", "name": "Sam"}]}),
    ));
    let v = elevenlabs::voices(&c, &key()).unwrap();
    assert_eq!(v[0], elevenlabs::Voice { id: "abc123".into(), name: "Rachel".into(), gender: Some("female".into()), language: None });
    fake.push(Response::new(200, vec![1u8, 0, 0xff, 0x7f]));
    let pcm = elevenlabs::speak(&c, &key(), elevenlabs::DEFAULT_VOICE_MODEL, "abc123", "Привет").unwrap();
    assert_eq!(pcm, vec![1, i16::MAX]);
    let r = &fake.sent()[1];
    assert_eq!(r.url, "https://api.elevenlabs.io/v1/text-to-speech/abc123?output_format=pcm_44100");
    assert_eq!(r.json().unwrap()["model_id"], "eleven_multilingual_v2");
    // a voice id cannot reach another path
    fake.push(Response::new(200, vec![0u8, 0]));
    elevenlabs::speak(&c, &key(), "m", "../../user?x=1", "a").unwrap();
    assert_eq!(fake.sent()[2].url, "https://api.elevenlabs.io/v1/text-to-speech/userx1?output_format=pcm_44100");
    fake.push(Response::json(200, &json!({"detail": "quota_exceeded"})));
    assert!(elevenlabs::speak(&c, &key(), "m", "abc", "a").is_err(), "a JSON answer is not audio");
    assert!(elevenlabs::speak(&c, &key(), "m", "", "a").is_err());
}

#[test]
fn images_from_openai_and_gemini() {
    let png = [0x89u8, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0];
    let b64 = base64::encode(&png);
    let fake = Arc::new(FakeTransport::new());
    let (c, _) = ctx(&fake);
    fake.push(Response::json(200, &json!({"data": [{"b64_json": b64}]})));
    let pic = images::generate(&c, Provider::OpenAi, &key(), images::DEFAULT_OPENAI_MODEL, "a red fox", images::Shape::Portrait).unwrap();
    assert_eq!((pic.data.as_slice(), pic.extension), (&png[..], "png"));
    let r = &fake.sent()[0];
    assert_eq!(r.url, "https://api.openai.com/v1/images/generations");
    assert_eq!(r.json().unwrap()["size"], "1024x1536");
    fake.push(Response::json(
        200,
        &json!({"candidates": [{"content": {"parts": [{"text": "Here it is"}, {"inlineData": {"mimeType": "image/png", "data": b64}}]}}]}),
    ));
    let pic = images::generate(&c, Provider::Gemini, &key(), images::DEFAULT_GEMINI_MODEL, "a red fox", images::Shape::Landscape).unwrap();
    assert_eq!(pic.extension, "png");
    let r = &fake.sent()[1];
    assert!(r.url.ends_with("/models/gemini-2.5-flash-image:generateContent"), "{}", r.url);
    assert_eq!(r.json().unwrap()["generationConfig"]["imageConfig"]["aspectRatio"], "16:9");
    // no picture, a declined prompt, not an image, and a provider without images
    fake.push(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": "no"}]}, "finishReason": "IMAGE_SAFETY"}]})));
    assert!(matches!(images::generate(&c, Provider::Gemini, &key(), "m", "x", images::Shape::Square), Err(AiError::Blocked(_, w)) if w == "IMAGE_SAFETY"));
    fake.push(Response::json(200, &json!({"data": [{"b64_json": base64::encode(b"hello")}]})));
    assert!(matches!(images::generate(&c, Provider::OpenAi, &key(), "m", "x", images::Shape::Square), Err(AiError::BadResponse(..))));
    assert!(images::generate(&c, Provider::Anthropic, &key(), "m", "x", images::Shape::Square).is_err());
    assert_eq!(images::Shape::for_frame(1080, 1920), images::Shape::Portrait);
    assert_eq!(images::Shape::for_frame(1920, 1080), images::Shape::Landscape);
}

#[test]
fn every_provider_says_what_it_does() {
    for p in Provider::ALL {
        assert_eq!(Provider::from_id(p.id()), Some(p));
        assert!(p.does_speech() || p.does_text() || p.does_images() || p.does_voices(), "{p}");
        assert_eq!(p.does_speech(), !p.default_speech_model().is_empty(), "{p}");
        assert_eq!(p.does_text(), !p.default_text_model().is_empty(), "{p}");
        assert_eq!(p.does_images(), !p.default_image_model().is_empty(), "{p}");
    }
    assert_eq!(Provider::from_id("claude"), Some(Provider::Anthropic));
}
