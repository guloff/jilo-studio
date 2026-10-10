//! Tests of [`crate::silence_tools`]: pauses found by sound and removed as one undo step.

use std::sync::Arc;

use serde_json::json;

use super::*;

const RATE: u32 = 48_000;

/// Stereo audio: tone, 2 s pause, tone, 0.3 s pause (too short), tone.
fn wav() -> Arc<[u8]> {
    let parts = [(2.0, 0.3f32), (2.0, 0.0), (2.0, 0.3), (0.3, 0.0), (1.7, 0.3)];
    let mut mono = Vec::new();
    for (secs, amp) in parts {
        let n = (secs * RATE as f64) as usize;
        mono.extend((0..n).map(|i| amp * (i as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin()));
    }
    let inter: Vec<f32> = mono.iter().flat_map(|v| [*v, *v]).collect();
    crate::previews::write_wav_f32(&inter, RATE).into()
}

fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "talk", "audio": 2, "video": 1})).unwrap();
    let item = crate::commands::import_bytes(&mut s, "/talk.wav", wav(), None).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    s
}

fn seconds(s: &Session) -> f64 {
    s.active_sequence().unwrap().duration().seconds()
}

#[test]
fn finds_the_long_pause_only() {
    let mut s = session();
    let r = s.execute("audio.findSilences", json!({})).unwrap();
    let sil = r["silences"].as_array().unwrap();
    assert_eq!(sil.len(), 1, "{r}");
    assert!((sil[0]["start"].as_f64().unwrap() - 2.0).abs() < 0.02 && (sil[0]["end"].as_f64().unwrap() - 4.0).abs() < 0.02, "{r}");
    // a lower threshold or a shorter minimum changes what counts
    let short = s.execute("audio.findSilences", json!({"minSeconds": 0.25})).unwrap();
    assert_eq!(short["silences"].as_array().unwrap().len(), 2);
}

#[test]
fn removing_ripples_and_undoes_in_one_step() {
    let mut s = session();
    let before = seconds(&s);
    let r = s.execute("audio.removeSilences", json!({"keepSeconds": 0.15})).unwrap();
    assert_eq!(r["removed"], 1);
    let cut = before - seconds(&s);
    assert!((cut - 1.7).abs() < 0.1, "removed {cut} s");
    s.execute("edit.undo", json!({})).unwrap();
    assert!((seconds(&s) - before).abs() < 1e-6);
}

#[test]
fn no_audio_or_all_silence_is_refused() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "empty"})).unwrap();
    assert!(s.execute("audio.findSilences", json!({})).unwrap_err().to_string().contains("no audio clips"));
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "quiet", "audio": 1, "video": 1})).unwrap();
    let silent: Vec<f32> = vec![0.0; RATE as usize * 4];
    let item = crate::commands::import_bytes(&mut s, "/quiet.wav", crate::previews::write_wav_f32(&silent, RATE).into(), None).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    assert!(s.execute("audio.removeSilences", json!({})).unwrap_err().to_string().contains("whole sequence is silent"));
}

/// Filler words of the transcript's language: Russian hesitations are chosen, discourse words not.
#[test]
fn russian_fillers_are_found_and_hesitations_removed_by_default() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name": "ru", "audio": 1, "video": 1})).unwrap();
    let item = crate::commands::import_bytes(&mut s, "/ru.wav", wav(), None).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "audioTrack": "A1", "seconds": 0.0})).unwrap();
    let tick = |x: f64| filmcraft_time::Tick::from_seconds_f64(x).0;
    let words: Vec<_> = ["Сегодня", "э", "мы", "ну", "монтируем", "ээ,", "интервью"]
        .iter()
        .enumerate()
        .map(|(i, w)| json!({"text": w, "start": tick(i as f64), "end": tick(i as f64 + 0.8)}))
        .collect();
    s.execute("transcript.set", json!({"item": item.0, "transcript": {"language": "ru", "speakers": [], "words": words}})).unwrap();
    let r = s.execute("transcript.fillers", json!({})).unwrap();
    assert_eq!(r["language"], "ru");
    let list = r["fillers"].as_array().unwrap();
    let get = |p: &str| list.iter().find(|f| f["phrase"] == p).cloned().unwrap();
    assert_eq!((get("э")["count"].clone(), get("э")["selected"].clone()), (json!(1), json!(true)));
    assert_eq!(get("ээ")["kind"], "hesitation");
    assert_eq!((get("ну")["kind"].clone(), get("ну")["selected"].clone()), (json!("discourse"), json!(false)));
    let removed = s.execute("transcript.removeFillers", json!({})).unwrap();
    assert_eq!(removed["removed"], 2, "э and ээ, not ну");
    let left: Vec<String> = crate::transcript::sequence_words(&s).iter().map(|w| w.text.clone()).collect();
    assert_eq!(left, ["Сегодня", "мы", "ну", "монтируем", "интервью"]);
}
