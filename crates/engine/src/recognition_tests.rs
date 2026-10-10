//! Tests of [`crate::recognition`]: cloud recognition only with consent, in a background job whose
//! transcript `transcript.generate` applies as one undo step, retired models replaced. The cloud is a
//! scripted transport: no network.

use std::sync::Arc;

use filmcraft_ai::Response;
use filmcraft_ai::transport::FakeTransport;
use filmcraft_project::ItemId;
use serde_json::{Value, json};

use super::*;
use crate::media_test_util::tmp_dir;

const RATE: u32 = 16_000;

/// 10 s of stereo audio: a tone ("speech") at 1–3 s, silence elsewhere.
fn speech_wav() -> Arc<[u8]> {
    let n = 10 * RATE as usize;
    let mono: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            if (1.0..3.0).contains(&t) { 0.3 * (t * 220.0 * std::f32::consts::TAU).sin() } else { 0.0 }
        })
        .collect();
    let inter: Vec<f32> = mono.iter().flat_map(|v| [*v, *v]).collect();
    crate::previews::write_wav_f32(&inter, RATE).into()
}

fn gemini_answer() -> Response {
    let cues = json!({"language": "Russian", "segments": [{"start": "00:01.000", "end": "00:03.000", "text": "Привет, мир.", "speaker": "S1",
        "words": [{"text": "Привет,", "start": "00:01.000", "end": "00:01.800"}, {"text": "мир.", "start": "00:02.000", "end": "00:02.900"}]}]});
    Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": cues.to_string()}]}, "finishReason": "STOP"}]}))
}

/// A session with the speech clip imported, a Gemini key and a scripted cloud.
fn session(name: &str) -> (Session, Arc<FakeTransport>, ItemId, std::path::PathBuf) {
    let dir = tmp_dir(&format!("recog-{name}"));
    let fake = Arc::new(FakeTransport::new());
    let mut s =
        Session { ai_keys_file: Some(dir.join("ai.json")), ai_transport: Some(fake.clone()), ai_sleep: Some(Arc::new(|_, _| {})), ..Default::default() };
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "test-key"})).unwrap();
    let item = crate::commands::import_bytes(&mut s, "/interview.wav", speech_wav(), None).unwrap();
    (s, fake, item, dir)
}

fn words(s: &Session, item: ItemId) -> Vec<String> {
    s.project.transcripts.get(&item).map(|t| t.words.iter().map(|w| w.text.clone()).collect()).unwrap_or_default()
}

#[test]
fn the_cloud_waits_for_consent() {
    let (mut s, fake, item, dir) = session("consent");
    let e = s.execute("transcript.generate", json!({"items": [item.0], "engine": "gemini", "language": "ru"})).unwrap_err();
    assert!(e.to_string().contains("CONFIRM_CLOUD") && e.to_string().contains("Google Gemini"), "{e}");
    assert!(fake.sent().is_empty(), "nothing is sent before the user agrees");
    assert!(s.project.transcripts.is_empty());
    // the agreement is remembered per provider
    fake.route(|r| r.url.contains(":generateContent"), |_| Ok(gemini_answer()));
    let r = s.execute("transcript.generate", json!({"items": [item.0], "engine": "gemini", "language": "ru", "confirm": true})).unwrap();
    assert!(s.prefs.ai.confirmed(filmcraft_ai::Provider::Gemini));
    assert_eq!(r["engine"], "gemini:gemini-flash-latest");
    assert_eq!(words(&s, item), ["Привет,", "мир."]);
    let t = s.project.transcripts.get(&item).unwrap();
    assert_eq!(t.language, "ru");
    assert_eq!(t.source, "gemini:gemini-flash-latest");
    assert_eq!(t.speakers.len(), 1, "the model's label is the speaker");
    assert_eq!(r["items"][0]["recognition"]["retried"], 0);
    // the audio went inline as WAV with the Tajik-aware prompt, in one window
    let sent = fake.sent();
    assert_eq!(sent.len(), 1);
    let body: Value = sent[0].json().unwrap();
    let data = body["contents"][0]["parts"][0]["inline_data"]["data"].as_str().unwrap();
    assert!(data.starts_with("UklGR"), "base64 of RIFF");
    // OpenAI still needs its own agreement
    s.execute("ai.setKey", json!({"provider": "openai", "key": "sk"})).unwrap();
    assert!(s.execute("transcript.generate", json!({"items": [item.0], "engine": "openai"})).unwrap_err().to_string().contains("CONFIRM_CLOUD"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn background_recognition_is_applied_as_one_undo_step() {
    let (mut s, fake, item, dir) = session("job");
    fake.route(|r| r.url.contains(":generateContent"), |_| Ok(gemini_answer()));
    let p = json!({"items": [item.0], "engine": "gemini", "language": "ru", "confirm": true});
    let mut job = p.clone();
    job["wait"] = json!(true);
    let r = s.execute("transcript.recognize", job).unwrap();
    let j = s.jobs.iter().find(|j| j.id == r["job"].as_u64().unwrap()).unwrap().to_json();
    assert_eq!(j["finished"], true, "{j}");
    assert!(j["status"].as_str().unwrap().starts_with("Recognised 2 words"), "{j}");
    assert!(s.project.transcripts.is_empty(), "the job does not change the project");
    let sent_before = fake.sent().len();
    let undo_before = s.history.undo.len();
    s.execute("transcript.generate", p.clone()).unwrap();
    assert_eq!(fake.sent().len(), sent_before, "the waiting transcript is used, nothing is sent again");
    assert_eq!(words(&s, item), ["Привет,", "мир."]);
    assert_eq!(s.history.undo.len(), undo_before + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.project.transcripts.is_empty());
    // other settings do not take a waiting transcript
    assert!(s.recognition_cache.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_retired_model_is_replaced_and_remembered() {
    let (mut s, fake, item, dir) = session("retired");
    s.execute("prefs.set", json!({"key": "ai.geminiSpeechModel", "value": "gemini-1.5-flash"})).unwrap();
    fake.route(
        |r| r.url.contains("/models/gemini-1.5-flash:"),
        |_| {
            Ok(Response::json(
                404,
                &json!({"error": {"message": "Gemini 1.5 Flash is no longer available. Please use models/gemini-2.5-flash for the latest features."}}),
            ))
        },
    );
    fake.route(|r| r.url.contains("/models/gemini-2.5-flash:"), |_| Ok(gemini_answer()));
    let r = s.execute("transcript.generate", json!({"items": [item.0], "engine": "gemini", "confirm": true})).unwrap();
    assert_eq!(words(&s, item).len(), 2);
    assert_eq!(s.prefs.ai.gemini_speech_model, "gemini-2.5-flash");
    assert_eq!(r["items"][0]["source"], "gemini:gemini-2.5-flash");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn provider_errors_reach_the_user_without_the_key() {
    let (mut s, fake, item, dir) = session("errors");
    fake.route(|_| true, |_| Ok(Response::json(400, &json!({"error": {"message": "API key not valid. Please pass a valid API key."}}))));
    let e = s.execute("transcript.generate", json!({"items": [item.0], "engine": "gemini", "confirm": true})).unwrap_err();
    assert!(e.to_string().contains("did not accept the API key"), "{e}");
    assert!(!e.to_string().contains("test-key"));
    assert!(s.project.transcripts.is_empty());
    // no key at all
    s.execute("ai.setKey", json!({"provider": "gemini", "key": null})).unwrap();
    assert!(s.execute("transcript.generate", json!({"items": [item.0], "engine": "gemini"})).unwrap_err().to_string().contains("no Google Gemini API key"));
    assert!(s.execute("transcript.generate", json!({"items": [item.0], "engine": "claude"})).unwrap_err().to_string().contains("unknown engine"));
    let _ = std::fs::remove_dir_all(&dir);
}
