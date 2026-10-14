use serde_json::json;

use super::*;
use filmcraft_project::CaptionStyle;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[test]
fn built_in_recipes_name_real_commands() {
    for r in builtin() {
        assert!(!r.steps.is_empty(), "{}", r.name);
        for st in &r.steps {
            assert!(crate::find_command(&st.command).is_some(), "{}: {}", r.name, st.command);
        }
    }
}

#[test]
fn recording_keeps_edits_without_navigation_or_ids() {
    let mut s = demo();
    s.execute("recipes.recordStart", json!({})).unwrap();
    assert!(s.execute("recipes.recordStart", json!({})).is_err(), "one recording at a time");
    let t = s.execute("captions.newTrack", json!({})).unwrap()["track"].as_u64().unwrap();
    s.execute("playhead.set", json!({"seconds": 2})).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
    s.execute("captions.add", json!({"text": "Hi", "track": t, "seconds": 1})).unwrap();
    s.execute("captions.applyStylePreset", json!({"preset": "dark-pill", "track": t})).unwrap();
    s.execute("edit.undo", json!({})).unwrap();
    let r = s.execute("recipes.recordStop", json!({})).unwrap();
    let cmds: Vec<&str> = r["steps"].as_array().unwrap().iter().map(|st| st["command"].as_str().unwrap()).collect();
    assert_eq!(cmds, ["captions.newTrack", "captions.add", "captions.applyStylePreset"]);
    // the track id of this project is left out: the next run uses the first caption track
    assert!(r["steps"][1]["params"].get("track").is_none());
    assert_eq!(r["steps"][1]["params"]["text"], "Hi");
    assert!(s.recording.is_none());
    assert!(s.execute("recipes.recordStop", json!({})).is_err());
}

#[test]
fn a_run_is_one_undo_step_and_stops_at_a_failing_step() {
    let mut s = demo();
    let undo0 = s.history.undo.len();
    let steps = json!([
        {"command": "captions.newTrack"},
        {"command": "captions.add", "params": {"text": "One", "seconds": 1}},
        {"command": "captions.add", "params": {"text": "Off", "seconds": 8}, "enabled": false},
        {"command": "captions.applyStylePreset", "params": {"preset": "yellow-pop", "track": "all"}},
    ]);
    let r = s.execute("recipes.run", json!({"name": "Test", "steps": steps, "wait": true})).unwrap();
    assert_eq!(r["finished"], true, "{r}");
    assert!(r["error"].is_null(), "{r}");
    let q = s.active_sequence().unwrap();
    assert_eq!(q.caption_tracks.len(), 1);
    assert_eq!(q.caption_tracks[0].captions.len(), 1, "the step that is off did not run");
    assert_eq!(q.caption_tracks[0].style, crate::caption_presets::find("yellow-pop").unwrap().style);
    assert_eq!(s.history.undo.len(), undo0 + 1);
    assert_eq!(s.history.undo.last().unwrap().0, "Recipe: Test");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks.is_empty(), "one undo takes the whole run back");

    // a failing step stops the run and says which
    let bad_steps =
        json!([{"command": "captions.newTrack"}, {"command": "captions.applyStylePreset", "params": {"preset": "nope"}}, {"command": "captions.newTrack"}]);
    let r = s.execute("recipes.run", json!({"steps": bad_steps, "wait": true})).unwrap();
    assert!(r["error"].as_str().unwrap().contains("Apply Caption Style Preset"), "{r}");
    assert_eq!(s.active_sequence().unwrap().caption_tracks.len(), 1, "the third step did not run");
    assert!(s.execute("recipes.run", json!({"steps": [{"command": "no.such"}]})).is_err());
    assert_ne!(CaptionStyle::default(), crate::caption_presets::find("yellow-pop").unwrap().style);
}

#[test]
fn karaoke_captions_recipe_transcribes_in_the_background_then_styles() {
    use filmcraft_project::{Transcript, Word};
    let mut s = demo();
    let q = s.active_sequence().unwrap();
    let a = &q.audio_tracks[0].items[0];
    let sin = a.source_in;
    let mut t = Transcript { language: "en".into(), ..Default::default() };
    for (text, x, y) in [("Hello", 0.2, 0.5), ("big", 0.6, 0.9), ("world.", 1.0, 1.4)] {
        t.words.push(Word::new(text, sin + filmcraft_time::Tick::from_seconds_f64(x), sin + filmcraft_time::Tick::from_seconds_f64(y)));
    }
    t.normalize();
    s.transcriber = Some(std::sync::Arc::new(filmcraft_speech::FixedTranscriber { transcript: t, id: "fixed".into() }));
    let r = s.execute("recipes.run", json!({"name": "Karaoke Captions", "wait": true})).unwrap();
    assert!(r["error"].is_null(), "{r}");
    let q = s.active_sequence().unwrap();
    let tr = q.caption_tracks.first().expect("captions made");
    assert_eq!(tr.style, crate::caption_presets::find("karaoke-yellow").unwrap().style);
    assert!(!tr.captions[0].words.is_empty(), "word times kept for the highlight");
}

#[test]
fn saved_recipes_round_trip_and_replace_built_ins_by_name() {
    let d = std::env::temp_dir().join(format!("jilo-recipes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let r = Recipe {
        format: format(),
        version: version(),
        name: "Мой рецепт / 1".into(),
        description: "x".into(),
        steps: vec![step("captions.newTrack", json!({}))],
        builtin: false,
    };
    let out = write_in(&d, &r).unwrap();
    assert!(out["path"].as_str().unwrap().ends_with("мой-рецепт---1.json"), "{out}");
    assert_eq!(saved_in(&d), vec![r]);
    let _ = std::fs::remove_dir_all(&d);
}
