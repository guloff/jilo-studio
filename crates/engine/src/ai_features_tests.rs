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

// ---------- highlights ----------

/// A sequence with 60 s of audio on A1 and a Russian transcript: sentences of five one-second words
/// ("слово1 … слово5."), each followed by a 1 s pause, so a cue every 6 s.
fn transcribed(name: &str) -> (Session, Arc<FakeTransport>, std::path::PathBuf) {
    let (mut s, fake, dir) = session(name);
    let rate = 16_000;
    let wav = crate::previews::write_wav_f32(&vec![0.0; rate as usize * 120], rate);
    let item = crate::commands::import_bytes(&mut s, "/talk.wav", wav.into(), None).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    let tick = |x: f64| filmcraft_time::Tick::from_seconds_f64(x).0;
    let mut words = Vec::new();
    for sentence in 0..10 {
        for w in 0..5 {
            let t = sentence as f64 * 6.0 + w as f64;
            let text = if w == 4 { format!("слово{sentence}{w}.") } else { format!("слово{sentence}{w}") };
            words.push(json!({"text": text, "start": tick(t), "end": tick(t + 0.9)}));
        }
    }
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "ru", "speakers": [], "words": words}})).unwrap();
    (s, fake, dir)
}

#[test]
fn cues_break_at_pauses_phrase_ends_and_seven_seconds() {
    let (s, _, dir) = transcribed("hl-cues");
    let cues = crate::highlights::cues_from_words(&crate::transcript::sequence_words(&s));
    assert_eq!(cues.len(), 10, "{cues:?}");
    assert_eq!((cues[1].start, cues[1].text.split(' ').count()), (6.0, 5));
    assert!((cues[1].end - 10.9).abs() < 1e-6);
    let text = crate::highlights::transcript_text(&cues[..1]);
    assert_eq!(text, "[0.0-4.9] слово00 слово01 слово02 слово03 слово04.");
    // a long run without pauses or phrase ends is cut every seven seconds
    let long: Vec<filmcraft_edit::transcript::SeqWord> = crate::transcript::sequence_words(&s)
        .into_iter()
        .enumerate()
        .map(|(i, mut w)| {
            w.text = format!("w{i}");
            w.start = filmcraft_time::Tick::from_seconds_f64(i as f64 * 0.5);
            w.end = filmcraft_time::Tick::from_seconds_f64(i as f64 * 0.5 + 0.4);
            w
        })
        .collect();
    let cues = crate::highlights::cues_from_words(&long);
    assert!(cues.iter().all(|c| c.end - c.start <= 7.0), "{cues:?}");
    assert!(cues.len() >= 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fragments_are_ranked_snapped_cut_and_kept_apart() {
    use crate::highlights::{Cue, Limits, snap_fragments};
    let cues: Vec<Cue> = (0..10).map(|i| Cue { start: i as f64 * 6.0, end: i as f64 * 6.0 + 4.9, text: format!("cue {i}") }).collect();
    let limits = Limits { count: 3, min_seconds: 5.0, max_seconds: 20.0 };
    let raw = vec![
        json!({"start": 12.3, "end": 22.0, "title": "Second", "score": 70, "reason": "r2"}),
        json!({"start": 0.4, "end": 10.6, "title": "Best", "score": 95.4, "reason": "r1"}),
        // overlaps "Best" after snapping: dropped
        json!({"start": 6.2, "end": 16.0, "title": "Overlap", "score": 80, "reason": ""}),
        // longer than 20 s: ends at the last cue that fits
        json!({"start": 30.0, "end": 59.0, "title": "Long", "score": 60, "reason": ""}),
        // not a fragment
        json!({"start": "x", "end": 3, "title": "Bad", "score": 99}),
        json!({"start": 1, "end": 3, "title": "  ", "score": 99}),
        json!({"start": 48.0, "end": 58.0, "title": "Over the count", "score": 10, "reason": ""}),
    ];
    let f = snap_fragments(&raw, &cues, limits);
    let titles: Vec<&str> = f.iter().map(|x| x.title.as_str()).collect();
    assert_eq!(titles, ["Best", "Second", "Long"]);
    assert_eq!((f[0].start, f[0].end, f[0].score), (0.0, 10.9, 95));
    assert_eq!((f[1].start, f[1].end), (12.0, 22.9));
    assert_eq!((f[2].start, f[2].end), (30.0, 46.9), "cut to the last cue ending within 20 s");
    assert!(snap_fragments(&raw, &[], limits).is_empty());
    // limits are clamped
    let l = Limits::from_params(&json!({"count": 99, "minSeconds": 1, "maxSeconds": 2}));
    assert_eq!((l.count, l.min_seconds, l.max_seconds), (20, 5.0, 10.0));
    let l = Limits::from_params(&json!({"count": "x", "minSeconds": f64::NAN}));
    assert_eq!((l.count, l.min_seconds, l.max_seconds), (5, 15.0, 60.0));
}

#[test]
fn highlights_become_range_markers_in_one_undo_step() {
    let (mut s, fake, dir) = transcribed("hl-find");
    gemini(&fake, |prompt, user| {
        assert!(prompt.contains("go viral") && prompt.contains("in Russian") && prompt.contains("up to 2 "), "{prompt}");
        assert!(user.starts_with("[0.0-4.9] слово00"), "{user}");
        json!({"fragments": [
            {"start": 24.0, "end": 41.0, "title": "Второй", "score": 71, "reason": "почему"},
            {"start": 0.0, "end": 16.5, "title": "Первый", "score": 90, "reason": "потому что"}
        ]})
    });
    let e = s.execute("highlights.find", json!({"count": 2})).unwrap_err();
    assert!(e.to_string().contains("CONFIRM_CLOUD") && e.to_string().contains("transcript"), "{e}");
    let undo = s.history.undo.len();
    let r = s.execute("highlights.find", json!({"count": 2, "confirm": true})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let list = r["highlights"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!((list[0]["title"].clone(), list[0]["score"].clone()), (json!("Первый"), json!(90)));
    let q = s.active_sequence().unwrap();
    assert_eq!(q.markers.len(), 2);
    let m = &q.markers[0];
    assert_eq!((m.name.as_str(), m.comment.as_str(), m.color), ("Первый", "90/100 · потому что", filmcraft_project::Label::Mango));
    assert_eq!(m.start, filmcraft_time::Tick::ZERO);
    assert_eq!(m.duration, filmcraft_time::Tick::from_seconds_f64(16.9), "ends with its cue");
    assert_eq!(q.markers[1].start, filmcraft_time::Tick::from_seconds_f64(24.0));
    s.undo();
    assert!(s.active_sequence().unwrap().markers.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_or_unmatched_highlight_answer_is_an_error() {
    let (mut s, fake, dir) = transcribed("hl-bad");
    s.execute("prefs.set", json!({"key": "ai.confirmedProviders", "value": ["gemini"]})).unwrap();
    gemini(&fake, |prompt, _| {
        if prompt.contains("fact-dense") {
            json!({"items": []})
        } else {
            json!({"fragments": [{"start": 500.0, "end": 400.0, "title": "Nowhere", "score": 50, "reason": ""}]})
        }
    });
    let e = s.execute("highlights.find", json!({"mode": "facts"})).unwrap_err().to_string();
    assert!(e.contains("list of fragments"), "{e}");
    let e = s.execute("highlights.find", json!({"mode": "meaningful"})).unwrap_err().to_string();
    assert!(e.contains("none matches"), "{e}");
    assert!(s.execute("highlights.find", json!({"mode": "funny"})).unwrap_err().to_string().contains("unknown mode"));
    assert!(s.active_sequence().unwrap().markers.is_empty());
    // no transcript: refused before anything is sent
    let sent = fake.sent().len();
    s.execute("file.newSequence", json!({"name": "empty", "audio": 1})).unwrap();
    assert!(s.execute("highlights.find", json!({})).unwrap_err().to_string().contains("no transcript"));
    assert_eq!(fake.sent().len(), sent);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn highlights_assemble_into_subsequences_a_sequence_or_the_export_queue() {
    let (mut s, fake, dir) = transcribed("hl-assemble");
    s.execute("prefs.set", json!({"key": "ai.confirmedProviders", "value": ["gemini"]})).unwrap();
    gemini(&fake, |_, _| {
        json!({"fragments": [
            {"start": 0.0, "end": 10.9, "title": "One", "score": 90, "reason": ""},
            {"start": 30.0, "end": 40.9, "title": "Two/2", "score": 80, "reason": ""}
        ]})
    });
    let r = s.execute("highlights.find", json!({"minSeconds": 5})).unwrap();
    let ids: Vec<u64> = r["highlights"].as_array().unwrap().iter().map(|h| h["marker"].as_u64().unwrap()).collect();
    let seq = s.state.active_sequence.unwrap();
    let before = s.project.items.len();
    let undo = s.history.undo.len();
    let r = s.execute("highlights.assemble", json!({"markers": ids, "as": "subsequences"})).unwrap();
    assert_eq!(r["sequences"].as_array().unwrap().len(), 2);
    assert_eq!(s.project.items.len(), before + 2);
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step for all");
    let sub = s.project.sequence(filmcraft_project::ItemId(r["sequences"][1].as_u64().unwrap())).unwrap();
    assert_eq!(sub.duration(), filmcraft_time::Tick::from_seconds_f64(10.9));
    assert_eq!(s.project.item(filmcraft_project::ItemId(r["sequences"][1].as_u64().unwrap())).unwrap().name, "s · Two/2");
    assert_eq!(s.state.active_sequence, Some(seq), "the edited sequence stays active");
    let r = s.execute("highlights.assemble", json!({"markers": ids, "as": "sequence"})).unwrap();
    let all = s.project.sequence(filmcraft_project::ItemId(r["sequences"][0].as_u64().unwrap())).unwrap();
    assert_eq!(all.duration(), filmcraft_time::Tick::from_seconds_f64(21.8), "both, end to end");
    assert_eq!(all.audio_tracks[0].items.len(), 2);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let r = s.execute("highlights.assemble", json!({"markers": ids, "as": "queue", "path": out.to_string_lossy()})).unwrap();
    assert_eq!(r["queued"].as_array().unwrap().len(), 2);
    let paths: Vec<String> = s.export_queue.items.iter().map(|i| i.settings.path.clone()).collect();
    assert!(paths[0].ends_with("s 01 One.mp4") && paths[1].ends_with("s 02 Two_2.mp4"), "{paths:?}");
    let r0 = s.export_queue.items[1].settings.range.unwrap();
    assert_eq!((r0.start, r0.duration), (filmcraft_time::Tick::from_seconds_f64(30.0), filmcraft_time::Tick::from_seconds_f64(10.9)));
    // hostile parameters
    for bad in [json!({}), json!({"markers": []}), json!({"markers": [999999]}), json!({"markers": ids, "as": "zip"})] {
        assert!(s.execute("highlights.assemble", bad.clone()).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
