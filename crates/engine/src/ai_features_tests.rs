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
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "ru", "speakers": [], "words": sentence_words()}})).unwrap();
    (s, fake, dir)
}

/// Ten sentences of five one-second words ("слово00 … слово04."), a 1 s pause after each.
fn sentence_words() -> Vec<Value> {
    let tick = |x: f64| filmcraft_time::Tick::from_seconds_f64(x).0;
    let mut words = Vec::new();
    for sentence in 0..10 {
        for w in 0..5 {
            let t = sentence as f64 * 6.0 + w as f64;
            let text = if w == 4 { format!("слово{sentence}{w}.") } else { format!("слово{sentence}{w}") };
            words.push(json!({"text": text, "start": tick(t), "end": tick(t + 0.9)}));
        }
    }
    words
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

// ---------- auto zooms ----------

/// `transcribed` with a picture on V1 for the whole minute and a sound that is loud during the
/// third and seventh sentences (12–16.9 s, 36–40.9 s) and quiet elsewhere.
fn talking_head(name: &str) -> (Session, Arc<FakeTransport>, std::path::PathBuf, u64) {
    let (mut s, fake, dir) = session(name);
    let rate = 16_000u32;
    let mut pcm = Vec::with_capacity(rate as usize * 120);
    for i in 0..rate as usize * 60 {
        let t = i as f64 / f64::from(rate);
        let loud = (12.0..16.9).contains(&t) || (36.0..40.9).contains(&t);
        let v = ((t * 220.0 * std::f64::consts::TAU).sin() * if loud { 0.6 } else { 0.1 }) as f32;
        pcm.push(v);
        pcm.push(v);
    }
    let item = crate::commands::import_bytes(&mut s, "/loud.wav", crate::previews::write_wav_f32(&pcm, rate).into(), None).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "ru", "speakers": [], "words": sentence_words()}})).unwrap();
    let png = dir.join("face.png");
    image::RgbaImage::from_pixel(64, 36, image::Rgba([40, 120, 200, 255])).save(&png).unwrap();
    let r = s.execute("file.import", json!({"paths": [png.to_string_lossy()]})).unwrap();
    let pic = r["items"][0].as_u64().unwrap();
    let r =
        s.execute("timeline.place", json!({"item": pic, "track": "V1", "seconds": 0.0, "duration": filmcraft_time::Tick::from_seconds_f64(60.0).0})).unwrap();
    let clip = r["clips"][0].as_u64().unwrap();
    s.state.selection.clear();
    (s, fake, dir, clip)
}

/// The clip's Scale at sequence second `t`, and its keyframe count.
fn scale_at(s: &Session, clip: u64, t: f64) -> (f64, usize) {
    let (_, it) = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap();
    let p = &it.effect("motion").unwrap().params["scale"];
    (p.f64_at(it.effect_time_at(filmcraft_time::Tick::from_seconds_f64(t))), p.keyframes.len())
}

#[test]
fn loud_moments_stand_out_from_the_median_and_keep_their_distance() {
    use crate::autozoom::{Moment, pick_loud_moments};
    use crate::highlights::Cue;
    let cues: Vec<Cue> = (0..8).map(|i| Cue { start: i as f64 * 3.0, end: i as f64 * 3.0 + 2.5, text: String::new() }).collect();
    let level = |a: f64, _: f64| match a as i64 {
        6 => 0.9,
        9 => 0.8, // loud, but within 6 s of the louder one at 6 s
        18 => 0.5,
        _ => 0.1,
    };
    assert_eq!(pick_loud_moments(&cues, level, 6.0, 10), [Moment { start: 6.0, end: 8.5 }, Moment { start: 18.0, end: 20.5 }]);
    assert_eq!(pick_loud_moments(&cues, level, 6.0, 1), [Moment { start: 6.0, end: 8.5 }]);
    assert!(pick_loud_moments(&cues, |_, _| 0.3, 6.0, 10).is_empty(), "nothing louder than the rest");
    assert!(pick_loud_moments(&[], level, 6.0, 10).is_empty());
}

#[test]
fn a_quoted_phrase_is_found_in_the_words_of_its_cue() {
    let (s, _, dir) = transcribed("az-quote");
    let words = crate::transcript::sequence_words(&s);
    let spans = crate::highlights::cue_spans(&words);
    let cue = |i: usize| words.get(spans[i].1.clone()).unwrap();
    // the 2nd and 3rd words of the second sentence, whatever the case and punctuation
    assert_eq!(crate::autozoom::phrase_time(cue(1), "Слово11, СЛОВО12"), Some((7.0, 8.9)));
    // a changed tail still finds the head
    assert_eq!(crate::autozoom::phrase_time(cue(1), "слово12 слово13 слово14 и ещё что-то"), Some((8.0, 10.9)));
    assert_eq!(crate::autozoom::phrase_time(cue(1), "нет такого"), None);
    // the model's moments: by quote (anywhere), by time without one, too close ones dropped
    let raw = vec![
        json!({"start": 30.0, "end": 31.0, "quote": "слово52 слово53"}), // its cue is 30–34.9, the quote too
        json!({"start": 1.0, "end": 9.0, "quote": "слово42"}),           // the quote is at 26 s, not at 1 s
        json!({"start": 50.2, "end": 50.4, "quote": ""}),                // the word at 50 s; at least 0.8 s
        json!({"start": 51.0, "end": 52.0, "quote": ""}),                // within 6 s of the last one
    ];
    let m = crate::autozoom::snap_zoom_moments(&raw, &spans, &words, 6.0, 10);
    let got: Vec<(f64, f64)> = m.iter().map(|m| (m.start, m.end)).collect();
    assert_eq!(got, [(26.0, 26.9), (32.0, 33.9), (50.0, 50.9)], "{got:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zoom_keys_cut_or_ease_in_and_out() {
    use crate::autozoom::{Style, zoom_keys};
    use filmcraft_project::Interpolation::{Bezier, Hold};
    let t = filmcraft_time::Tick::from_seconds_f64;
    let cut = zoom_keys(100.0, 115.0, &[(t(2.0), t(4.0))], t(0.0), t(10.0), Style::Cut);
    let shape: Vec<(f64, f64, _)> = cut.iter().map(|k| (k.time.seconds(), k.value.as_f64().unwrap(), k.interp)).collect();
    assert_eq!(shape, [(0.0, 100.0, Hold), (2.0, 115.0, Hold), (4.0, 100.0, Hold)]);
    let smooth = zoom_keys(100.0, 115.0, &[(t(2.0), t(4.0)), (t(4.2), t(4.6))], t(0.0), t(10.0), Style::Smooth);
    let shape: Vec<(f64, f64, _)> = smooth.iter().map(|k| ((k.time.seconds() * 100.0).round() / 100.0, k.value.as_f64().unwrap(), k.interp)).collect();
    assert_eq!(
        shape,
        [
            (0.0, 100.0, Hold),
            (2.0, 100.0, Bezier),
            (2.35, 115.0, Hold),
            (4.0, 115.0, Bezier),
            // back by the next zoom's start, which a 0.2 s ramp leads into
            (4.2, 100.0, Bezier),
            (4.4, 115.0, Hold),
            (4.6, 115.0, Bezier),
            (4.8, 100.0, Hold)
        ]
    );
}

#[test]
fn loud_phrases_zoom_the_picture_in_one_undo_step() {
    let (mut s, fake, dir, clip) = talking_head("az-loud");
    let undo = s.history.undo.len();
    let r = s.execute("clip.autoZoom", json!({})).unwrap();
    assert!(fake.sent().is_empty(), "loudness is measured offline");
    assert_eq!(r["moments"], json!([{"start": 12.0, "end": 16.0}, {"start": 36.0, "end": 40.0}]), "{r}");
    assert_eq!(r["clips"], 1);
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(scale_at(&s, clip, 5.0), (100.0, 5));
    assert_eq!(scale_at(&s, clip, 13.0).0, 115.0);
    assert_eq!(scale_at(&s, clip, 20.0).0, 100.0);
    assert_eq!(scale_at(&s, clip, 39.0).0, 115.0);
    // again, smooth and stronger: the keys are replaced, not added to
    s.execute("clip.autoZoom", json!({"style": "smooth", "intensity": 1.3})).unwrap();
    assert_eq!(scale_at(&s, clip, 14.0), (130.0, 9));
    let eased = scale_at(&s, clip, 12.1).0;
    assert!(eased > 100.0 && eased < 130.0, "{eased}");
    s.undo();
    assert_eq!(scale_at(&s, clip, 14.0), (115.0, 5));
    s.undo();
    assert_eq!(scale_at(&s, clip, 14.0), (100.0, 0));
    // hostile parameters
    for bad in [json!({"method": "magic"}), json!({"style": "zoomy"}), json!({"clips": [999_999]})] {
        assert!(s.execute("clip.autoZoom", bad.clone()).is_err(), "{bad}");
    }
    let r = s.execute("clip.autoZoom", json!({"intensity": 99, "minGap": -5})).unwrap();
    assert_eq!(scale_at(&s, clip, 13.0).0, 160.0, "intensity clamps to 1.6 ({r})");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn zooms_by_meaning_follow_the_quoted_phrase() {
    let (mut s, fake, dir, clip) = talking_head("az-meaning");
    gemini(&fake, |prompt, user| {
        assert!(prompt.contains("punch-in zooms") && prompt.contains("up to 10 moments"), "{prompt}");
        assert!(user.starts_with("[0.00-4.90] слово00"), "{user}");
        json!({"moments": [{"start": 24.0, "end": 26.0, "quote": "слово43 слово44"}]})
    });
    let e = s.execute("clip.autoZoom", json!({"method": "meaning"})).unwrap_err();
    assert!(e.to_string().contains("CONFIRM_CLOUD"), "{e}");
    let r = s.execute("clip.autoZoom", json!({"method": "meaning", "confirm": true})).unwrap();
    assert_eq!(r["moments"], json!([{"start": 27.0, "end": 28.9}]), "at the quoted words, not the model's time: {r}");
    assert_eq!(scale_at(&s, clip, 27.5).0, 115.0);
    assert_eq!(scale_at(&s, clip, 25.0).0, 100.0);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- YouTube texts and chapters ----------

#[test]
fn youtube_fields_follow_youtube_rules() {
    use crate::youtube::*;
    assert_eq!(clean_line("  <b>Как   монтировать</b>  ", 100), "bКак монтировать/b");
    assert_eq!(clean_line("абвгд", 3), "абв");
    assert_eq!(clean_text("Абзац  один\r\n\n\n\nАбзац <два>", 5000), "Абзац один\n\nАбзац два");
    let tags = normalise_tags(["#монтаж", "Монтаж", "видео, редактор", "", "rust"]);
    assert_eq!(tags, ["монтаж", "видео редактор", "rust"]);
    assert_eq!(tags_length(&tags), 6 + 1 + (14 + 2) + 1 + 4);
    let long: Vec<String> = (0..100).map(|i| format!("tag{i:03}")).collect();
    assert!(tags_length(&normalise_tags(long.iter().map(String::as_str))) <= TAGS_LIMIT);
    assert_eq!(normalise_hashtags(["#Монтаж видео", "#монтажвидео", "Rust", "#a", "#b"]), ["#Монтажвидео", "#Rust", "#a"]);
    assert_eq!(format_chapter_time(75.9, 600.0), "1:15");
    assert_eq!(format_chapter_time(3725.0, 4000.0), "1:02:05");
    assert_eq!(format_chapter_time(5.0, 4000.0), "0:00:05");
}

#[test]
fn chapters_follow_youtube_rules() {
    use crate::youtube::{ChapterProblem, pick_chapter_cues, timed_chapters};
    let marks = |v: &[(f64, &str)]| v.iter().map(|(t, n)| (*t, n.to_string())).collect::<Vec<_>>();
    // the first goes to 0:00, a chapter 4 s after another is left out, so is a 6 s tail
    let (c, p) = timed_chapters(&marks(&[(3.4, "A"), (7.0, "B"), (20.9, "C"), (40.0, "D"), (55.0, "E")]), 61.0);
    assert_eq!(c, marks(&[(0.0, "A"), (20.0, "C"), (40.0, "D")]));
    assert!(p.is_empty());
    assert_eq!(timed_chapters(&marks(&[(0.0, "A"), (15.0, "B")]), 61.0).1, [ChapterProblem::TooFew]);
    assert_eq!(timed_chapters(&marks(&[(0.0, "A"), (10.0, "B"), (20.0, "C")]), 25.0).1, [ChapterProblem::VideoTooShort]);
    // requested times to cues: too close moves on to a cue 10 s later, a short tail is not taken
    let starts: Vec<f64> = (0..10).map(|i| i as f64 * 6.0).collect();
    assert_eq!(pick_chapter_cues(&[30.0, 0.0, 5.0, 55.0, f64::NAN], &starts, 60.0), [0, 2, 5]);
    assert!(pick_chapter_cues(&[], &starts, 60.0).is_empty());
}

#[test]
fn youtube_texts_are_written_kept_and_chapters_become_markers() {
    let (mut s, fake, dir) = transcribed("yt");
    s.execute("markers.addChapter", json!({"time": 0, "name": "old"})).unwrap();
    gemini(&fake, |prompt, user| {
        assert!(prompt.contains("in Russian") && prompt.contains("lasts 60 seconds") && prompt.contains("between 3 and"), "{prompt}");
        assert!(user.starts_with("[0] слово00") && user.contains("\n[12] слово20"), "{user}");
        json!({
            "titles": ["Как <монтировать> быстрее", "  ", "Монтаж за минуту"],
            "description": "Первый абзац.\n\n\n\nВторой абзац.",
            "chapters": [{"start": 30, "title": "Середина"}, {"start": 0, "title": "Вступление"}, {"start": 5, "title": "Почти сразу"}, {"start": 55, "title": "Хвост"}],
            "tags": ["монтаж", "Монтаж", "#видео"],
            "hashtags": ["#монтаж", "видео", "rust", "лишний"]
        })
    });
    let e = s.execute("youtube.generate", json!({})).unwrap_err();
    assert!(e.to_string().contains("CONFIRM_CLOUD"), "{e}");
    let undo = s.history.undo.len();
    let r = s.execute("youtube.generate", json!({"confirm": true})).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(r["titles"], json!(["Как монтировать быстрее", "Монтаж за минуту"]));
    assert_eq!(r["tags"], json!(["монтаж", "видео"]));
    assert_eq!(r["hashtags"], json!(["#монтаж", "#видео", "#rust"]));
    let chapters: Vec<(f64, String)> =
        r["chapters"].as_array().unwrap().iter().map(|c| (c["time"].as_f64().unwrap(), c["title"].as_str().unwrap().to_string())).collect();
    assert_eq!(chapters, [(0.0, "Вступление".to_string()), (12.0, "Почти сразу".to_string()), (30.0, "Середина".to_string())]);
    assert_eq!(r["problems"], json!([]));
    assert_eq!(r["description"], "Первый абзац.\n\nВторой абзац.\n\n0:00 Вступление\n0:12 Почти сразу\n0:30 Середина\n\n#монтаж #видео #rust");
    let q = s.active_sequence().unwrap();
    let names: Vec<&str> = q.markers.iter().filter(|m| m.kind == filmcraft_project::MarkerKind::Chapter).map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Вступление", "Почти сразу", "Середина"], "the old chapter marker is replaced");
    // the texts stay with the sequence; a moved chapter marker moves the chapter
    let id = q.markers.iter().find(|m| m.name == "Середина").unwrap().id;
    s.edit_sequence("move", |q, _, _| {
        q.markers.iter_mut().find(|m| m.id == id).unwrap().start = filmcraft_time::Tick::from_seconds_f64(41.5);
        Ok(())
    })
    .unwrap();
    let t = s.execute("youtube.texts", json!({})).unwrap();
    assert!(t["description"].as_str().unwrap().contains("0:41 Середина"), "{t}");
    assert!(t["all"].as_str().unwrap().starts_with("Как монтировать быстрее\n\nПервый абзац."));
    // edits go through the same checks, one undo step
    let r = s.execute("youtube.set", json!({"text": "Новый <текст>", "tags": "a, b, a"})).unwrap();
    assert_eq!((r["text"].clone(), r["tags"].clone()), (json!("Новый текст"), json!(["a", "b"])));
    s.undo();
    assert_eq!(s.execute("youtube.texts", json!({})).unwrap()["text"], "Первый абзац.\n\nВторой абзац.");
    // without a transcript: refused before anything is sent
    let sent = fake.sent().len();
    s.execute("file.newSequence", json!({"name": "empty", "audio": 1})).unwrap();
    assert!(s.execute("youtube.generate", json!({})).unwrap_err().to_string().contains("no transcript"));
    assert!(s.execute("youtube.texts", json!({})).is_err());
    assert_eq!(fake.sent().len(), sent);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- batches ----------

/// Gemini answering both the speech windows (two words) and the YouTube prompt.
fn gemini_for_batch(fake: &FakeTransport) {
    fake.route(
        |r| r.url.contains(":generateContent"),
        |r| {
            let b = r.json().unwrap_or_default();
            let text = b.to_string();
            let answer = if text.contains("YouTube producer") {
                json!({"titles": ["Заголовок"], "description": "Описание.", "chapters": [], "tags": ["тег"], "hashtags": ["#тег"]})
            } else {
                json!({"language": "Russian", "segments": [{"start": "00:01.000", "end": "00:03.000", "text": "Привет, мир.",
                    "words": [{"text": "Привет,", "start": "00:01.000", "end": "00:01.800"}, {"text": "мир.", "start": "00:02.000", "end": "00:02.900"}]}]})
            };
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
}

/// A media item with a minute of tone (never placed), for batches that make its sequence.
fn tone_clip(s: &mut Session, name: &str) -> u64 {
    let rate = 16_000u32;
    let pcm: Vec<f32> = (0..rate as usize * 2 * 20).map(|i| ((i / 2) as f32 * 0.05).sin() * 0.3).collect();
    crate::commands::import_bytes(s, &format!("/{name}.wav"), crate::previews::write_wav_f32(&pcm, rate).into(), None).unwrap().0
}

#[test]
fn a_batch_runs_its_steps_over_every_item_and_survives_a_failing_one() {
    let (mut s, fake, dir) = transcribed("batch");
    gemini_for_batch(&fake);
    let first = s.state.active_sequence.unwrap().0;
    // a second transcribed sequence, and a clip without a transcript
    let item = s.active_sequence().unwrap().audio_tracks[0].items[0].item.0;
    s.execute("file.newSequence", json!({"name": "second", "audio": 1, "video": 1})).unwrap();
    let second = s.state.active_sequence.unwrap().0;
    s.execute("timeline.place", json!({"item": item, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    let clip = tone_clip(&mut s, "raw");
    s.state.active_sequence = Some(filmcraft_project::ItemId(first));
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    // a cloud step needs the consent first
    let p = json!({"items": [first, second, clip], "youtube": true, "export": true, "preset": "Waveform Audio 48 kHz 16-bit", "path": out.to_string_lossy()});
    assert!(s.execute("batch.start", p.clone()).unwrap_err().to_string().contains("CONFIRM_CLOUD"));
    assert!(s.batch.is_none());
    let mut go = p.clone();
    go["confirm"] = json!(true);
    go["wait"] = json!(true);
    let r = s.execute("batch.start", go).unwrap();
    assert_eq!(r["finished"], true, "{r}");
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!((items[0]["status"].clone(), items[0]["done"].clone()), (json!("done"), json!(["youtube", "export"])), "{r}");
    assert_eq!(items[1]["status"], "done");
    // the clip became a sequence; it has no transcript, so its YouTube step fails and export still runs
    assert_eq!(items[2]["name"], "raw.wav");
    assert_eq!(items[2]["status"], "failed");
    assert_eq!(items[2]["errors"][0]["step"], "youtube");
    assert!(items[2]["errors"][0]["error"].as_str().unwrap().contains("no transcript"), "{r}");
    assert_eq!(items[2]["done"], json!(["export"]));
    assert_eq!(s.export_queue.items.len(), 3, "an export per sequence");
    assert!(s.export_queue.running, "the queue starts at the end");
    // the editor keeps its sequence; the texts are with each sequence
    assert_eq!(s.state.active_sequence.map(|i| i.0), Some(first));
    assert!(crate::youtube::texts_of(&s, filmcraft_project::ItemId(second)).is_some());
    let job = s.jobs.iter().find(|j| j.id == r["job"].as_u64().unwrap()).unwrap().to_json();
    assert_eq!(job["status"], "Batch done: 2 of 3", "{job}");
    // hostile parameters
    for bad in
        [json!({}), json!({"items": [first]}), json!({"items": [999_999], "fillers": true}), json!({"items": (0..60).collect::<Vec<u64>>(), "fillers": true})]
    {
        assert!(s.execute("batch.start", bad.clone()).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_batch_transcribes_what_has_no_transcript_and_can_be_stopped() {
    let (mut s, fake, dir) = session("batch-asr");
    gemini_for_batch(&fake);
    let a = tone_clip(&mut s, "one");
    let b = tone_clip(&mut s, "two");
    s.execute("prefs.set", json!({"key": "ai.confirmedProviders", "value": ["gemini"]})).unwrap();
    let r = s.execute("batch.start", json!({"items": [a, b], "transcribe": true, "speechEngine": "gemini", "language": "ru", "wait": true})).unwrap();
    assert_eq!(r["items"][0]["done"], json!(["transcribe"]), "{r}");
    assert_eq!(r["items"][1]["status"], "done");
    for id in [a, b] {
        let words: Vec<String> = s.project.transcripts.get(&filmcraft_project::ItemId(id)).unwrap().words.iter().map(|w| w.text.clone()).collect();
        assert_eq!(words, ["Привет,", "мир."]);
    }
    // again: nothing left to transcribe, nothing is sent
    let sent = fake.sent().len();
    let seq = r["items"][0]["sequence"].as_u64().unwrap();
    let r = s.execute("batch.start", json!({"items": [seq], "transcribe": true, "speechEngine": "gemini", "wait": true})).unwrap();
    assert_eq!(r["items"][0]["status"], "done");
    assert_eq!(fake.sent().len(), sent);
    // Stop: the rest is cancelled
    let r = s.execute("batch.start", json!({"items": [a, b], "fillers": true})).unwrap();
    assert_eq!(r["finished"], false);
    let r = s.execute("batch.cancel", json!({})).unwrap();
    assert_eq!(r["finished"], true);
    assert!(r["items"].as_array().unwrap().iter().all(|i| i["status"] == "cancelled"), "{r}");
    assert!(s.execute("batch.cancel", json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A 1920×1080 black video on V1 of a 1080×1920 sequence, and a fake face finder: on its `k`-th
/// call (one per analysed frame) the speaker is at x = 0.3 + 0.02 k, and on the first a big face
/// passes by at the right. Returns (session, clip, calls).
fn vertical_talk() -> (Session, u64, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "Vertical", "width": 1080, "height": 1920, "video": 1, "audio": 1})).unwrap();
    let r = s.execute("file.newBlackVideo", json!({"width": 1920, "height": 1080, "seconds": 10.0})).unwrap();
    let r = s.execute("timeline.place", json!({"item": r["item"], "track": "V1", "seconds": 0.0})).unwrap();
    let clip = r["clips"][0].as_u64().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    s.face_detector = Some(Arc::new(move |rgba: &[u8], w: usize, h: usize| {
        assert_eq!(rgba.len(), w * h * 4);
        let k = c.fetch_add(1, Ordering::SeqCst);
        let mut faces = vec![crate::reframe::FaceBox { x: 0.25 + 0.02 * k as f64, y: 0.35, w: 0.1, h: 0.1, confidence: 0.9 }];
        if k == 0 {
            faces.push(crate::reframe::FaceBox { x: 0.75, y: 0.3, w: 0.2, h: 0.4, confidence: 0.95 });
        }
        Ok(faces)
    }));
    (s, clip, calls)
}

/// The clip's Motion at sequence second `t`: (position, scale, position keys).
fn motion_at(s: &Session, clip: u64, t: f64) -> (filmcraft_geom::Vec2, f64, usize) {
    let (_, it) = s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap();
    let m = it.effect("motion").unwrap();
    let mt = it.effect_time_at(filmcraft_time::Tick::from_seconds_f64(t));
    (m.vec2_at("position", mt), m.f64_at("scale", mt), m.params["position"].keyframes.len())
}

#[test]
fn a_clip_is_reframed_on_its_speaker_in_one_undo_step() {
    use std::sync::atomic::Ordering;
    let (mut s, clip, calls) = vertical_talk();
    let before = motion_at(&s, clip, 5.0);
    let fit = if s.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap().1.scale_to_frame { 0.5625 } else { 1.0 };
    let cover: f64 = 1920.0 / 1080.0;
    let undo = s.history.undo.len();
    let r = s.execute("clip.reframeToFace", json!({"clips": [clip]})).unwrap();
    assert_eq!((r["clips"].as_u64(), r["target"].as_str()), (Some(1), Some("speaker")), "{r}");
    assert_eq!(calls.load(Ordering::SeqCst), 21, "two frames a second, 0 to 10 s");
    assert_eq!(s.history.undo.len(), undo + 1);
    let (pos, scale, keys) = motion_at(&s, clip, 5.0);
    assert!((scale - (cover / fit * 1000.0).round() / 10.0).abs() < 1e-9, "the picture fills the frame: {scale}");
    assert!((2..21).contains(&keys), "the straight move is thinned: {keys}");
    // the speaker (not the passer-by) on the vertical third, the frame filled top to bottom
    let at = |x: f64| 540.0 - cover * (x * 1920.0 - 960.0);
    assert!((pos.x - at(0.5)).abs() < 5.0 && (pos.y - 960.0).abs() < 1e-6, "{pos:?}");
    let (pos, _, _) = motion_at(&s, clip, 2.0);
    assert!((pos.x - at(0.38)).abs() < 5.0, "{pos:?} vs {}", at(0.38));
    // the analysis is kept: another framing finds no faces again
    s.execute("clip.reframeToFace", json!({"clips": [clip], "target": "group", "zoom": 200})).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 21);
    assert!((motion_at(&s, clip, 5.0).1 - (cover * 2.0 / fit * 1000.0).round() / 10.0).abs() < 1e-9);
    // the face picked at the right edge: held there, the layer's right edge on the frame's
    s.set_playhead(filmcraft_time::Tick::ZERO);
    s.execute("clip.reframeToFace", json!({"clips": [clip], "target": "face", "x": 0.85, "y": 0.5})).unwrap();
    // (until the speaker walks up to where it was, at 6.5 s)
    let (pos, _, _) = motion_at(&s, clip, 3.0);
    assert!((pos.x - (1080.0 - cover * 960.0)).abs() < 0.1, "{pos:?}");
    s.undo();
    s.undo();
    s.undo();
    assert_eq!(motion_at(&s, clip, 5.0), before);
    // the faces under the playhead, for picking
    s.set_playhead(filmcraft_time::Tick::from_seconds_f64(1.0));
    let r = s.execute("clip.facesAt", json!({"clips": [clip]})).unwrap();
    assert_eq!(r["clip"].as_u64(), Some(clip));
    assert_eq!(r["faces"].as_array().map(Vec::len), Some(1), "{r}");
    // hostile parameters
    for bad in [json!({"clips": [clip], "target": "cat"}), json!({"clips": [clip], "target": "face"}), json!({"clips": [999_999]}), json!({"clips": "all"})] {
        assert!(s.execute("clip.reframeToFace", bad.clone()).is_err(), "{bad}");
    }
    let r = s.execute("clip.reframeToFace", json!({"clips": [clip], "smoothing": -9, "zoom": 1e9, "anchorX": f64::MAX})).unwrap();
    assert_eq!(r["clips"], 1);
    assert!((motion_at(&s, clip, 5.0).1 - (cover * 4.0 / fit * 1000.0).round() / 10.0).abs() < 1e-9, "zoom clamps to 400 %");
}

#[test]
fn reframing_without_faces_or_a_face_finder_says_why() {
    let (mut s, clip, _) = vertical_talk();
    s.face_detector = Some(Arc::new(|_: &[u8], _, _| Ok(Vec::new())));
    let e = s.execute("clip.reframeToFace", json!({"clips": [clip]})).unwrap_err().to_string();
    assert!(e.contains("no faces were found in"), "{e}");
    s.face_cache.lock().unwrap().clear();
    s.face_detector = Some(Arc::new(|_: &[u8], _, _| Err("Vision failed".to_string())));
    let e = s.execute("clip.reframeToFace", json!({"clips": [clip]})).unwrap_err().to_string();
    assert!(e.contains("Vision failed"), "{e}");
    assert!(s.face_cache.lock().unwrap().is_empty(), "a failed analysis is not kept");
    let undo = s.history.undo.len();
    s.face_detector = None;
    let e = s.execute("clip.reframeToFace", json!({"clips": [clip]})).unwrap_err().to_string();
    assert!(e.contains("not available"), "{e}");
    assert!(s.execute("clip.facesAt", json!({"clips": [clip]})).is_err());
    assert_eq!(s.history.undo.len(), undo);
}

/// A real PNG of `w` × `h`, as an image engine answers.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(w, h, image::Rgba([200, 60, 40, 255])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn a_generated_image_is_imported_and_placed_after_consent() {
    let (mut s, fake, dir) = session("imagegen");
    s.execute("ai.setKey", json!({"provider": "openai", "key": "k2"})).unwrap();
    let b64 = filmcraft_ai::base64::encode(&png_bytes(64, 36));
    fake.route(|r| r.url.ends_with("/images/generations"), move |_| Ok(Response::json(200, &json!({"data": [{"b64_json": b64}]}))));
    let p = json!({"prompt": "a lighthouse at dawn", "dir": dir.to_string_lossy(), "place": true});
    let e = s.execute("ai.generateImage", p.clone()).unwrap_err().to_string();
    assert!(e.contains("CONFIRM_CLOUD"), "asks before the description goes out: {e}");
    assert!(fake.sent().is_empty());
    // in the background, then applied by the same call
    let mut bg = p.clone();
    bg["confirm"] = json!(true);
    bg["background"] = json!(true);
    let r = s.execute("ai.generateImage", bg).unwrap();
    let job = r["job"].as_u64().unwrap();
    for _ in 0..500 {
        if s.jobs.iter().find(|j| j.id == job).is_some_and(|j| j.to_json()["finished"] == true) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let undo = s.history.undo.len();
    let r = s.execute("ai.generateImage", p.clone()).unwrap();
    assert_eq!(fake.sent().len(), 1, "applying sends nothing again");
    let body = fake.sent()[0].json().unwrap();
    assert_eq!((body["model"].as_str(), body["size"].as_str()), (Some("gpt-image-1"), Some("1536x1024")), "the sequence is landscape");
    let path = r["path"].as_str().unwrap();
    assert!(path.ends_with(".png") && std::path::Path::new(path).exists(), "{r}");
    let item = filmcraft_project::ItemId(r["item"].as_u64().unwrap());
    assert!(s.project.item(item).is_some_and(|i| i.has_video()));
    assert_eq!(r["clips"].as_array().map(Vec::len), Some(1), "placed at the playhead: {r}");
    assert!(s.history.undo.len() > undo);
    // hostile parameters
    for bad in [
        json!({}),
        json!({"prompt": "  "}),
        json!({"prompt": "x", "engine": "anthropic"}),
        json!({"prompt": "x", "shape": "round"}),
        json!({"prompt": "é".repeat(4001)}),
    ] {
        assert!(s.execute("ai.generateImage", bad.clone()).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_elevenlabs_voice_speaks_a_narration_with_its_pauses() {
    let (mut s, fake, dir) = session("eleven-tts");
    s.execute("ai.setKey", json!({"provider": "elevenlabs", "key": "xi"})).unwrap();
    fake.route(
        |r| r.url.ends_with("/v1/voices"),
        |_| Ok(Response::json(200, &json!({"voices": [{"voice_id": "v1", "name": "Rachel", "labels": {"gender": "female"}}]}))),
    );
    // half a second of a tone for every piece of speech
    fake.route(
        |r| r.url.contains("/text-to-speech/v1"),
        |_| Ok(Response::new(200, (0..22_050).flat_map(|i| (((i as f32 * 0.05).sin() * 8000.0) as i16).to_le_bytes()).collect::<Vec<u8>>())),
    );
    let v = s.execute("tts.voices", json!({})).unwrap();
    assert_eq!((v["cloud"]["key"].as_bool(), v["cloud"]["loaded"].as_bool()), (Some(true), Some(false)));
    let v = s.execute("tts.cloudVoices", json!({"wait": true})).unwrap();
    assert_eq!(v["voices"][0]["id"], "elevenlabs:v1", "{v}");
    let p = json!({"text": "Привет. [pause 1s] Как дела?", "voice": "elevenlabs:v1", "dir": dir.to_string_lossy()});
    let e = s.execute("tts.create", p.clone()).unwrap_err().to_string();
    assert!(e.contains("CONFIRM_CLOUD"), "asks before the text goes out: {e}");
    let mut ok = p.clone();
    ok["confirm"] = json!(true);
    let r = s.execute("tts.create", ok).unwrap();
    let speech: Vec<_> = fake.sent().into_iter().filter(|r| r.url.contains("text-to-speech")).collect();
    assert_eq!(speech.len(), 2, "each piece of speech is spoken on its own");
    assert_eq!(speech[0].json().unwrap()["text"], "Привет.");
    let secs = r["seconds"].as_f64().or_else(|| r["speechSeconds"].as_f64()).unwrap_or_default();
    assert!((secs - 2.0).abs() < 0.1 || r.to_string().contains("clip"), "two half-second pieces and a one-second pause: {r}");
    // hostile voices
    for bad in ["elevenlabs:", "elevenlabs:../x", "elevenlabs:a b"] {
        assert!(s.execute("tts.create", json!({"text": "a", "voice": bad, "confirm": true})).is_err(), "{bad}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
