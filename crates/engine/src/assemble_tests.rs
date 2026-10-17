use super::*;
use filmcraft_project::{ItemKind, MarkerKind};

fn item_named(s: &Session, name: &str) -> ItemId {
    s.project.items.values().find(|i| i.name == name).map(|i| i.id).unwrap_or_else(|| panic!("no item {name}"))
}

fn sec(t: Tick) -> f64 {
    (t.seconds() * 1000.0).round() / 1000.0
}

/// A small PNG card in a fresh folder.
fn card(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("filmcraft-assemble-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("card.png");
    image::RgbaImage::from_pixel(64, 36, image::Rgba([200, 40, 40, 255])).save(&path).unwrap();
    path.to_string_lossy().to_string()
}

fn rate_snap(q: &filmcraft_project::Sequence, t: f64) -> Tick {
    q.settings.frame_rate.snap_nearest(Tick::from_seconds_f64(t))
}

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[test]
fn a_plan_becomes_a_sequence_in_one_undo_step() {
    let mut s = demo();
    let (ocean, aurora, score) = (item_named(&s, "Ocean_Sunset.mp4"), item_named(&s, "Aurora_Timelapse.mp4"), item_named(&s, "Ambient_Score.wav"));
    let png = card("plan");
    let undo = s.history.undo.len();
    let sequences = s.project.items.values().filter(|i| matches!(i.kind, ItemKind::Sequence(_))).count();
    let r = s
        .execute(
            "sequence.assemble",
            json!({
                "name": "YouTube", "width": 1280, "height": 720, "fps": 25,
                "video": [
                    [
                        {"item": ocean.0, "in": 1.0, "out": 3.0, "name": "Шот 1", "label": "Iris"},
                        {"item": aurora.0, "in": 0.5, "out": 2.5, "scale": 120, "position": [600, 340]}
                    ],
                    [{"media": png, "at": 0.4, "duration": 1.6, "opacity": 80, "label": "Violet"}]
                ],
                "audio": [[], [{"item": score.0, "in": 0, "duration": 4, "gainDb": -6}]],
                "captions": [{"name": "RU", "language": "ru", "cues": [{"start": 0, "end": 1.6, "text": "Привет"}, {"start": 1.6, "end": 3.0, "text": "мир"}]}],
                "markers": [{"at": 2, "name": "Глава 2", "kind": "chapter"}, {"at": 3.2, "name": "проверить", "comment": "звук", "color": "Mango"}]
            }),
        )
        .unwrap();
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step: {r}");
    assert_eq!(s.history.undo.last().unwrap().0, "Assemble Sequence: YouTube");
    let q = s.active_sequence().unwrap().clone();
    assert_eq!((q.settings.width, q.settings.height), (1280, 720));
    assert_eq!(r["sequence"].as_u64(), s.state.active_sequence.map(|i| i.0), "the new sequence is open");
    // V1: back to back from 0, the sound of each on A1, linked
    let v1: Vec<(f64, f64, String)> = q.video_tracks[0].items.iter().map(|i| (sec(i.start), sec(i.duration), i.name.clone())).collect();
    assert_eq!(v1[0], (0.0, 2.0, "Шот 1".into()));
    assert_eq!((v1[1].0, v1[1].1), (2.0, 2.0));
    assert_eq!(q.video_tracks[0].items[0].label, Label::Iris);
    assert_eq!(sec(q.video_tracks[0].items[0].source_in), 1.0);
    let a1: Vec<(f64, Option<u64>)> = q.audio_tracks[0].items.iter().map(|i| (sec(i.start), i.link)).collect();
    assert_eq!(a1.len(), 2, "{a1:?}");
    assert_eq!(a1[0].1, q.video_tracks[0].items[0].link, "linked to its picture");
    // framing on the second shot
    let motion = q.video_tracks[0].items[1].effects.iter().find(|e| e.effect == "motion").unwrap();
    assert_eq!(motion.params["scale"].value.as_f64(), Some(120.0));
    // V2: the card, no sound
    let v2 = &q.video_tracks[1].items;
    assert_eq!((v2.len(), sec(v2[0].start), sec(v2[0].duration), v2[0].label), (1, 0.4, 1.6, Label::Violet));
    // times land on frames: 0.5 s at 25 fps is half a frame
    assert_eq!(sec(rate_snap(&q, 0.5)), 0.48);
    // A2: the score, 6 dB down
    let a2 = &q.audio_tracks[1].items;
    assert_eq!((a2.len(), sec(a2[0].duration)), (1, 4.0));
    assert!((a2[0].gain_db + 6.0).abs() < 1e-9, "{}", a2[0].gain_db);
    // captions and markers
    assert_eq!(q.caption_tracks.len(), 1);
    let caps: Vec<(f64, String)> = q.caption_tracks[0].captions.iter().map(|c| (sec(c.start), c.text.clone())).collect();
    assert_eq!(caps, vec![(0.0, "Привет".into()), (1.6, "мир".into())]);
    assert_eq!(r["captions"], 2);
    assert!(q.markers.iter().any(|m| m.kind == MarkerKind::Chapter && m.name == "Глава 2" && sec(m.start) == 2.0), "{:?}", q.markers);
    assert!(q.markers.iter().any(|m| m.name == "проверить" && m.comment == "звук"));
    // one undo takes it all back: the sequence and the imported card
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.values().filter(|i| matches!(i.kind, ItemKind::Sequence(_))).count(), sequences);
    assert!(!s.project.items.values().any(|i| i.name == "card.png"));
}

#[test]
fn a_bad_plan_changes_nothing_and_says_where() {
    let mut s = demo();
    let ocean = item_named(&s, "Ocean_Sunset.mp4");
    let before = (s.history.undo.len(), s.project.items.len());
    let cases = [
        (json!({"video": [[{"item": ocean.0, "out": 2}]]}), "`name`"),
        (json!({"name": "x", "video": [[{"media": "/no/such/file.mov", "out": 2}]]}), "V1 clip 1: no file"),
        (json!({"name": "x", "video": [[{"media": "relative.mov", "out": 2}]]}), "absolute path"),
        (json!({"name": "x", "video": [[{"item": ocean.0, "in": 3, "out": 2}]]}), "no length"),
        (json!({"name": "x", "video": [[{"item": ocean.0}]]}), "`out`"),
        (json!({"name": "x", "video": [[{"item": ocean.0, "out": 2, "label": "Pink"}]]}), "unknown label"),
        (json!({"name": "x", "video": [[{"item": ocean.0, "out": 2}]], "captions": [{"cues": [{"start": 2, "end": 1, "text": "a"}]}]}), "cue 1"),
        (json!({"name": "x", "video": [[{"item": ocean.0, "out": 2}]], "markers": [{"name": "m"}]}), "marker 1"),
        (json!({"name": "x"}), "no clips"),
    ];
    for (plan, says) in cases {
        let e = s.execute("sequence.assemble", plan.clone()).unwrap_err().to_string();
        assert!(e.contains(says), "{plan}: {e}");
    }
    // a clip that fails once making has begun (an item that is not there): all of it is taken back
    let e = s.execute("sequence.assemble", json!({"name": "x", "video": [[{"item": ocean.0, "out": 2}, {"item": 999_999, "out": 1}]]})).unwrap_err();
    assert!(e.to_string().contains("no item 999999"), "{e}");
    assert_eq!((s.history.undo.len(), s.project.items.len()), before, "nothing was left behind");
}
