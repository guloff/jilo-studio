//! Tests of [`crate::ai`]: keys stay out of answers and the journal, key checks run as jobs over
//! the scripted transport (no network), and Settings ▸ AI validates its choices.

use std::sync::Arc;

use filmcraft_ai::transport::FakeTransport;
use filmcraft_ai::{Provider, Response};
use serde_json::json;

use super::*;
use crate::ai::{ctx, key};
use crate::media_test_util::tmp_dir;

const KEY: &str = "AIza-test-secret-0042";

fn session(name: &str) -> (Session, Arc<FakeTransport>, std::path::PathBuf) {
    let dir = tmp_dir(&format!("ai-{name}"));
    let fake = Arc::new(FakeTransport::new());
    let s = Session { ai_keys_file: Some(dir.join("ai.json")), ai_transport: Some(fake.clone()), ai_sleep: Some(Arc::new(|_, _| {})), ..Default::default() };
    (s, fake, dir)
}

fn provider<'a>(status: &'a Value, id: &str) -> &'a Value {
    status["providers"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap()
}

fn job(s: &Session, id: &Value) -> Value {
    s.jobs.iter().find(|j| j.id == id.as_u64().unwrap()).unwrap().to_json()
}

#[test]
fn keys_are_saved_privately_and_never_returned_or_journaled() {
    let (mut s, _, dir) = session("keys");
    let st = s.execute("ai.status", json!({})).unwrap();
    assert_eq!(provider(&st, "gemini")["key"], Value::Null);
    assert_eq!(st["speechEngine"], "whisper", "the local engine is the default");
    assert_eq!(st["network"], true, "an installed transport counts");

    let r = s.execute("ai.setKey", json!({"provider": "gemini", "key": KEY})).unwrap();
    assert_eq!(r, json!({"provider": "gemini", "key": "file"}));
    let file = std::fs::read_to_string(dir.join("ai.json")).unwrap();
    assert!(file.contains(KEY));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("ai.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }

    let st = s.execute("ai.status", json!({})).unwrap();
    assert_eq!(provider(&st, "gemini")["key"], "file");
    assert!(!st.to_string().contains(KEY), "status never shows the key");
    assert!(!s.journal.iter().any(|(id, p)| id.starts_with("ai.") || p.to_string().contains(KEY)), "the key is not journaled");
    assert!(!format!("{:?}", s.prefs).contains(KEY), "nor kept in preferences");

    s.execute("ai.setKey", json!({"provider": "gemini", "key": null})).unwrap();
    assert_eq!(provider(&s.execute("ai.status", json!({})).unwrap(), "gemini")["key"], Value::Null);
    assert!(s.execute("ai.setKey", json!({"provider": "skynet", "key": "x"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn key_check_lists_models_in_a_job() {
    let (mut s, fake, dir) = session("test");
    // no key yet: a clear error, nothing sent
    let e = s.execute("ai.test", json!({"provider": "openai", "wait": true})).unwrap_err();
    assert!(e.to_string().contains("no OpenAI API key"), "{e}");
    assert!(fake.sent().is_empty());

    s.execute("ai.setKey", json!({"provider": "openai", "key": KEY})).unwrap();
    let models = json!({"data": [{"id": "gpt-4o-mini"}, {"id": "whisper-1"}, {"id": "gpt-4o-transcribe"}]});
    fake.push(Response::json(200, &models)).push(Response::json(200, &models));
    let r = s.execute("ai.test", json!({"provider": "openai", "wait": true})).unwrap();
    let j = job(&s, &r["job"]);
    assert_eq!(j["finished"], true);
    assert_eq!(j["status"], "Key accepted, 1 models available");
    assert_eq!(fake.sent()[0].header_value("authorization"), Some(format!("Bearer {KEY}").as_str()));

    let st = s.execute("ai.status", json!({})).unwrap();
    let p = provider(&st, "openai");
    assert_eq!(p["textModels"], json!([{"id": "gpt-4o-mini", "name": "gpt-4o-mini"}]));
    assert_eq!(p["speechModels"].as_array().unwrap().len(), 2);
    assert_eq!(p["speechModel"], "whisper-1");

    // a rejected key: the job fails with the reason and no key in it
    fake.push(Response::json(401, &json!({"error": {"message": "Incorrect API key provided"}})));
    let r = s.execute("ai.test", json!({"provider": "openai", "wait": true})).unwrap();
    let j = job(&s, &r["job"]);
    assert_eq!(j["result"]["error"], "OpenAI did not accept the API key");
    assert!(!j.to_string().contains(KEY));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stopped_check_ends_cancelled() {
    let (mut s, fake, dir) = session("cancel");
    s.execute("ai.setKey", json!({"provider": "gemini", "key": KEY})).unwrap();
    fake.push_error("connection reset");
    // the user presses Stop while FilmCraft waits to retry: the wait raises the job's flag
    let jobs = Arc::new(std::sync::Mutex::new(Vec::<Arc<filmcraft_export::Progress>>::new()));
    let seen = jobs.clone();
    s.ai_sleep = Some(Arc::new(move |_, _| {
        for p in seen.lock().unwrap().iter() {
            p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }));
    // run the job by hand so its progress is known before it starts
    let job = crate::narration::new_job(&mut s, "check".into());
    jobs.lock().unwrap().push(job.progress.clone());
    let c = ctx(&s, job.progress.clone()).unwrap();
    let (k, _) = key(&s, Provider::Gemini).unwrap();
    assert_eq!(filmcraft_ai::test_key(&c, Provider::Gemini, &k).unwrap_err(), filmcraft_ai::AiError::Cancelled);
    assert_eq!(fake.sent().len(), 1, "no second attempt after Stop");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_page_validates_engines_and_defaults_models() {
    let (mut s, _, dir) = session("prefs");
    assert!(crate::settings::category("ai").is_some());
    s.execute("prefs.set", json!({"key": "ai.speechEngine", "value": "gemini"})).unwrap();
    assert_eq!(s.prefs.ai.speech_provider(), Some(Provider::Gemini));
    assert!(s.execute("prefs.set", json!({"key": "ai.speechEngine", "value": "skynet"})).is_err());
    s.execute("prefs.set", json!({"key": "ai.geminiTextModel", "value": "  "})).unwrap();
    assert_eq!(s.prefs.ai.text_model(Provider::Gemini), "gemini-flash-latest", "an empty model field means the default");
    assert_eq!(s.prefs.ai.speech_model(Provider::OpenAi), "whisper-1");
    let _ = std::fs::remove_dir_all(&dir);
}
