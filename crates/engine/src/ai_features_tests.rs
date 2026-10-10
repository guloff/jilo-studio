//! Tests of the cloud text features ([`crate::ai_tasks`]): translation, highlights, YouTube texts
//! and zooms by meaning, over a scripted transport (no network).

use std::sync::Arc;

use filmcraft_ai::Response;
use filmcraft_ai::transport::FakeTransport;
use serde_json::{Value, json};

use super::*;
use crate::media_test_util::tmp_dir;

/// Gemini answering `generateContent` with `answer(prompt, user text)`.
pub fn gemini(fake: &FakeTransport, answer: impl Fn(&str, &str) -> Value + Send + Sync + 'static) {
    fake.route(
        |r| r.url.contains(":generateContent"),
        move |r| {
            let b = r.json().unwrap_or_default();
            let prompt = b["contents"][0]["parts"][0]["text"].as_str().unwrap_or_default().to_string();
            let user = b["contents"][0]["parts"][1]["text"].as_str().unwrap_or_default().to_string();
            let v = answer(&prompt, &user);
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": v.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
}

pub fn session(name: &str) -> (Session, Arc<FakeTransport>, std::path::PathBuf) {
    let dir = tmp_dir(&format!("aifeat-{name}"));
    let fake = Arc::new(FakeTransport::new());
    let mut s =
        Session { ai_keys_file: Some(dir.join("ai.json")), ai_transport: Some(fake.clone()), ai_sleep: Some(Arc::new(|_, _| {})), ..Default::default() };
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "k"})).unwrap();
    s.execute("file.newSequence", json!({"name": "s", "audio": 1, "video": 1})).unwrap();
    (s, fake, dir)
}

fn add_captions(s: &mut Session, n: usize) {
    s.execute("captions.newTrack", json!({"language": "en", "name": "English"})).unwrap();
    for i in 0..n {
        s.execute("captions.add", json!({"text": format!("line {i}"), "seconds": i as f64 * 2.0, "durationSeconds": 1.5})).unwrap();
    }
}

#[test]
fn captions_are_translated_into_a_new_track_in_batches() {
    let (mut s, fake, dir) = session("translate");
    add_captions(&mut s, 45);
    gemini(&fake, |prompt, user| {
        assert!(prompt.contains("into Russian") && prompt.contains("from English"), "{prompt}");
        let items: Value = serde_json::from_str(user).unwrap();
        // the engine skips the very last caption
        let out: Vec<Value> = items["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["text"] != "line 44")
            .map(|i| json!({"id": i["id"], "text": format!("строка {}", i["text"].as_str().unwrap().trim_start_matches("line "))}))
            .collect();
        json!({"items": out})
    });
    let e = s.execute("captions.translate", json!({"to": "ru"})).unwrap_err();
    assert!(e.to_string().contains("CONFIRM_CLOUD") && e.to_string().contains("caption text"), "{e}");
    let undo = s.history.undo.len();
    let r = s.execute("captions.translate", json!({"to": "ru", "confirm": true})).unwrap();
    assert_eq!((r["captions"].clone(), r["untranslated"].clone()), (json!(45), json!(1)));
    assert_eq!(fake.sent().len(), 2, "two batches of at most 40");
    let q = s.active_sequence().unwrap();
    assert_eq!(q.caption_tracks.len(), 2);
    let (src, tr) = (&q.caption_tracks[0], &q.caption_tracks[1]);
    assert_eq!(tr.language, "ru");
    assert_eq!(tr.name, "Russian (translated from English)");
    assert_eq!(tr.style, src.style, "the translation looks like the original");
    assert_eq!(tr.captions[3].text, "строка 3");
    assert_eq!(tr.captions[44].text, "line 44", "a skipped caption keeps its text");
    for (a, b) in src.captions.iter().zip(&tr.captions) {
        assert_eq!((a.start, a.duration), (b.start, b.duration), "the original timing");
        assert_ne!(a.id, b.id);
    }
    assert_eq!(s.history.undo.len(), undo + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_background_translation_is_applied_without_sending_again() {
    let (mut s, fake, dir) = session("translate-bg");
    add_captions(&mut s, 3);
    s.execute("prefs.set", json!({"key": "ai.confirmedProviders", "value": ["gemini"]})).unwrap();
    gemini(&fake, |_, user| {
        let items: Value = serde_json::from_str(user).unwrap();
        json!({"items": items["items"].as_array().unwrap().iter().map(|i| json!({"id": i["id"], "text": "салом"})).collect::<Vec<_>>()})
    });
    let r = s.execute("captions.translate", json!({"to": "tg", "background": true})).unwrap();
    let id = r["job"].as_u64().unwrap();
    for _ in 0..500 {
        if s.jobs.iter().find(|j| j.id == id).is_some_and(|j| j.to_json()["finished"] == true) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(s.active_sequence().unwrap().caption_tracks.len(), 1, "the job changes nothing");
    let sent = fake.sent().len();
    s.execute("captions.translate", json!({"to": "tg"})).unwrap();
    assert_eq!(fake.sent().len(), sent);
    let q = s.active_sequence().unwrap();
    assert_eq!(q.caption_tracks[1].captions[0].text, "салом");
    assert!(q.caption_tracks[1].name.starts_with("Tajik"));
    let _ = std::fs::remove_dir_all(&dir);
}
