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
