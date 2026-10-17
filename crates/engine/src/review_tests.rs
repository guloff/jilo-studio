use super::*;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn kinds(r: &Value) -> Vec<String> {
    r["problems"].as_array().unwrap().iter().map(|p| p["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_demo_sequence_is_clean() {
    let mut s = demo();
    let r = s.execute("sequence.check", json!({})).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["errors"], 0);
}

#[test]
fn problems_are_found_where_they_are() {
    let mut s = demo();
    let rate = s.active_sequence().unwrap().settings.frame_rate;
    let f = rate.frame_duration();
    let (v1, a1) = {
        let q = s.active_sequence().unwrap();
        (q.video_tracks[0].items.clone(), q.audio_tracks[0].items.clone())
    };
    s.edit_sequence("damage", |q, _, _| {
        // a 2-frame hole after the first shot (nothing above it there), a 2-frame shot later on,
        // the second shot's sound 3 frames late
        let first_end = v1[0].end();
        let t = &mut q.video_tracks[0];
        t.items[0].duration -= Tick(2 * f.0);
        t.transitions.clear();
        let last = t.items.len() - 1;
        t.items[last].duration = Tick(2 * f.0);
        q.video_tracks[1].items.retain(|i| i.start >= first_end || i.end() <= first_end - Tick(2 * f.0));
        q.video_tracks[1].transitions.clear();
        let second = a1[1].id;
        let a = q.audio_tracks[0].items.iter_mut().find(|i| i.id == second).unwrap();
        a.source_in -= Tick(3 * f.0);
        q.audio_tracks[0].transitions.clear();
        Ok(())
    })
    .unwrap();
    s.execute("captions.newTrack", json!({})).unwrap();
    s.execute("captions.add", json!({"seconds": 1.0, "durationSeconds": 0.5, "text": "A caption far too long to read in half a second, and far too long to fit on two lines of a screen", "split": false}))
        .unwrap();
    s.execute("captions.add", json!({"seconds": 600.0, "durationSeconds": 2.0, "text": "late", "split": false})).unwrap();
    // one long line wraps onto two: fine; three lines' worth is not
    s.execute(
        "captions.add",
        json!({"seconds": 20.0, "durationSeconds": 4.0, "text": "Today let's talk about the basic principles of fact-checking.", "split": false}),
    )
    .unwrap();
    let r = s.execute("sequence.check", json!({})).unwrap();
    assert!(!r["problems"].as_array().unwrap().iter().any(|p| p["kind"] == "captionTooLong" && p["message"].as_str().unwrap().contains("Today")), "{r}");
    let k = kinds(&r);
    for want in ["black", "flashFrame", "outOfSync", "captionTooFast", "captionTooLong", "captionPastEnd"] {
        assert!(k.contains(&want.to_string()), "{want} not found: {r}");
    }
    assert_eq!(r["ok"], false);
    let black = r["problems"].as_array().unwrap().iter().find(|p| p["kind"] == "black").unwrap();
    assert!((black["at"].as_f64().unwrap() - (v1[0].end() - Tick(2 * f.0)).seconds()).abs() < 0.002, "{black}");
    assert!(black["message"].as_str().unwrap().starts_with("2 frames of black"), "{black}");
    let sync = r["problems"].as_array().unwrap().iter().find(|p| p["kind"] == "outOfSync").unwrap();
    assert!(sync["message"].as_str().unwrap().contains("3 frame(s)"), "{sync}");
}

#[test]
fn frames_come_as_one_sheet_without_moving_the_playhead() {
    let mut s = demo();
    s.set_playhead(Tick::from_seconds_f64(7.0));
    let dir = std::env::temp_dir().join(format!("filmcraft-review-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.png");
    let r = s.execute("program.renderFrames", json!({"seconds": [0.5, 3, 6, 9, 12], "maxSide": 160, "columns": 3, "path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["frames"].as_array().unwrap().len(), 5);
    assert_eq!(r["columns"], 3);
    let img = image::open(&path).unwrap();
    assert_eq!((img.width(), img.height()), (3 * 160 + 2 * 4, 2 * 90 + 4), "{r}");
    assert!((s.playhead().seconds() - 7.0).abs() < 0.05, "the playhead stays");
    assert!(s.execute("program.renderFrames", json!({"seconds": []})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
