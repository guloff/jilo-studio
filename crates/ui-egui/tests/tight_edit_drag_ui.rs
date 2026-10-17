//! Moving clips on a tight edit (cut to cut, many short shots) at "zoom to sequence": a narrow
//! clip's middle moves it rather than trimming a neighbour, snapping does not pull a short move
//! back to where it started, and a move or trim held at a limit says so in the status bar.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ClipId;
use filmcraft_time::Tick;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    fn new(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(r) = reply.try_recv() {
                assert_eq!(r["ok"], true, "{method} {params}: {r}");
                self.frames(2);
                return r["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn rect(&mut self, id: &str) -> [f32; 4] {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id}")).clone();
        let r: Vec<f32> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
        [r[0], r[1], r[2], r[3]]
    }
    fn send(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }
    /// A left-button drag in 12 steps.
    fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        self.send(egui::Event::PointerMoved(from));
        self.send(button(from, true));
        for k in 1..=12 {
            self.send(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
        }
        self.send(button(to, false));
        self.frames(4);
    }
    fn rate(&self) -> filmcraft_time::FrameRate {
        self.harness.state().session.active_sequence().unwrap().settings.frame_rate
    }
    fn clip(&self, id: ClipId) -> Option<(Tick, Tick)> {
        self.harness.state().session.active_sequence().unwrap().find_item(id).map(|(_, it)| (it.start, it.end()))
    }
    fn status(&self) -> String {
        self.harness.state().ui.status.clone()
    }
    fn pps(&self) -> f32 {
        self.harness.state().ui.timeline.pps as f32
    }
}

fn centre(r: [f32; 4]) -> egui::Pos2 {
    egui::pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

fn sec(s: f64) -> Tick {
    Tick::from_seconds_f64(s)
}

/// The demo project with V1 cut to cut: A 0–8 s, B 8–9 s (the narrow shot), C 9–17 s and D
/// 17–131 s; nothing on other tracks, no markers or in/out. Snapping on, the playhead out of the way at 60 s, the
/// Timeline maximized and zoomed to the sequence: about 12 px a second, B about 12 px wide.
fn tight() -> (Driver, [ClipId; 3]) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).expect("demo project");
    let spans = [(0.0, 8.0), (8.0, 9.0), (9.0, 17.0), (17.0, 131.0)];
    let ids = s
        .edit_sequence("fixture", |seq, ctx, _| {
            let proto = seq.video_tracks[0].items[0].clone();
            for tr in seq.all_tracks_mut() {
                tr.items.clear();
                tr.transitions.clear();
            }
            // nothing else to snap to near the shots
            seq.markers.clear();
            (seq.mark_in, seq.mark_out) = (None, None);
            let mut ids = Vec::new();
            for (a, b) in &spans {
                let mut it = proto.clone();
                *ctx.next_id += 1;
                it.id = ClipId(*ctx.next_id);
                it.name = format!("shot {a}");
                (it.start, it.duration, it.source_in) = (sec(*a), sec(b - a), Tick::ZERO);
                (it.link, it.group, it.speed, it.reverse, it.frame_hold) = (None, None, 1.0, false, None);
                ids.push(it.id);
                seq.video_tracks[0].items.push(it);
            }
            Ok(ids)
        })
        .expect("fixture");
    s.state.snapping = true;
    s.set_playhead(sec(60.0));
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"focused": "Timeline"}));
    d.ok("ui.menu.invoke", json!({"id": "window.maximizeFrame"}));
    d.ok("ui.menu.invoke", json!({"id": "view.zoomToSequence"}));
    d.frames(6);
    (d, [ids[0], ids[1], ids[2]])
}

/// (a) A 1 s shot between 8 s ones, about 12 px wide: dragged 12 frames left and then right by
/// its middle, it moves, and covers part of the neighbour it moves onto.
#[test]
fn a_narrow_clip_between_neighbours_moves_by_twelve_frames_at_fit_zoom() {
    let (mut d, [a, b, _]) = tight();
    assert!((9.0..16.0).contains(&d.pps()), "zoomed out to the whole sequence: {} px/s", d.pps());
    let shift = d.rate().tick_of(12);
    let px = shift.seconds() as f32 * d.pps();
    let from = centre(d.rect(&format!("timeline.clip.{}", b.0)));
    d.drag(from, from - egui::vec2(px, 0.0));
    assert_eq!(d.clip(b), Some((sec(8.0) - shift, sec(9.0) - shift)), "moved left: {}", d.status());
    assert_eq!(d.clip(a).map(|x| x.1), Some(sec(8.0) - shift), "A's end is overwritten");
    // and from the start again, to the right
    let (mut d, [_, b, _]) = tight();
    let from = centre(d.rect(&format!("timeline.clip.{}", b.0)));
    d.drag(from, from + egui::vec2(px, 0.0));
    assert_eq!(d.clip(b), Some((sec(8.0) + shift, sec(9.0) + shift)), "moved right: {}", d.status());
    // C's head is overwritten (what is left of it may be a new clip)
    let v1: Vec<(Tick, Tick)> = d.harness.state().session.active_sequence().unwrap().video_tracks[0].items.iter().map(|it| (it.start, it.end())).collect();
    assert!(v1.contains(&(sec(9.0) + shift, sec(17.0))), "C's start is overwritten: {v1:?}");
}

/// (b) With snapping on, a move of two frames is not pulled back to the neighbours' edges where
/// the clip started.
#[test]
fn a_two_frame_move_is_not_snapped_back_home() {
    let (mut d, [_, b, _]) = tight();
    let two = d.rate().tick_of(2);
    let px = two.seconds() as f32 * d.pps();
    assert!(px < 9.0, "two frames are inside the snap radius: {px} px");
    let from = centre(d.rect(&format!("timeline.clip.{}", b.0)));
    d.drag(from, from + egui::vec2(px, 0.0));
    assert_eq!(d.clip(b), Some((sec(8.0) + two, sec(9.0) + two)), "{}", d.status());
}

/// (c) A selection with a clip at the sequence start dragged left stays put, and the status bar
/// says why.
#[test]
fn a_move_held_at_the_sequence_start_says_so() {
    let (mut d, [a, b, _]) = tight();
    d.ok("engine.execute", json!({"command": "timeline.select", "params": {"clips": [a.0, b.0]}}));
    let from = centre(d.rect(&format!("timeline.clip.{}", a.0)));
    d.drag(from, from - egui::vec2(60.0, 0.0));
    assert_eq!(d.clip(a), Some((sec(0.0), sec(8.0))));
    assert!(d.status().contains("start of the sequence"), "status: {}", d.status());
}

/// A regular trim pulled into the next clip stops at it, and the status bar names the modifiers
/// that trim past it.
#[test]
fn a_trim_held_at_the_next_clip_says_so() {
    let (mut d, [a, ..]) = tight();
    let r = d.rect(&format!("timeline.clip.{}", a.0));
    let from = egui::pos2(r[0] + r[2] - 2.0, r[1] + r[3] / 2.0);
    d.drag(from, from + egui::vec2(50.0, 0.0));
    assert_eq!(d.clip(a), Some((sec(0.0), sec(8.0))), "the edge stays at the cut");
    assert!(d.status().contains("next clip"), "status: {}", d.status());
}

/// (e) A project saved with two captions overlapping on a caption track (an older build, a script,
/// a hand edit) still edits: a clip dragged on the picture track stays where it is dropped instead
/// of going back, and so do deleting and trimming. Only a new overlap is refused.
#[test]
fn an_old_caption_overlap_does_not_undo_every_edit() {
    let (mut d, [a, b, _]) = tight();
    let s = &mut d.harness.state_mut().session;
    s.execute("captions.newTrack", json!({})).expect("caption track");
    s.set_playhead(sec(20.0));
    s.execute("captions.add", json!({"text": "first"})).expect("caption");
    s.set_playhead(sec(30.0));
    s.execute("captions.add", json!({"text": "second"})).expect("caption");
    s.set_playhead(sec(60.0));
    // the first one runs into the second, as in the file
    s.edit("overlap", |p, st| {
        let q = p.sequence_mut(st.active_sequence.expect("sequence")).expect("sequence");
        let caps = &mut q.caption_tracks[0].captions;
        caps[0].duration = caps[1].start - caps[0].start + sec(1.0);
        Ok(())
    })
    .expect("overlap");
    let old = s.active_sequence().unwrap().check().expect_err("the sequence holds an overlap");
    assert!(old.contains("overlap"), "{old}");
    d.frames(2);
    // a move by dragging is kept
    let shift = d.rate().tick_of(12);
    let px = shift.seconds() as f32 * d.pps();
    let from = centre(d.rect(&format!("timeline.clip.{}", b.0)));
    d.drag(from, from + egui::vec2(px, 0.0));
    assert_eq!(d.clip(b), Some((sec(8.0) + shift, sec(9.0) + shift)), "the dropped clip stays: {}", d.status());
    // deleting and trimming too
    let s = &mut d.harness.state_mut().session;
    s.state.selection = vec![b];
    s.execute("edit.rippleDelete", json!({"clips": [b.0]})).expect("ripple delete");
    s.execute("timeline.trim", json!({"clip": a.0, "edge": "out", "delta": -sec(1.0).0})).expect("trim");
    // the old overlap is still there (nothing is changed behind the user's back)
    let caps = s.active_sequence().unwrap().caption_tracks[0].captions.clone();
    assert!(caps[0].end() > caps[1].start, "the old overlap is kept as it was");
    // a new one is refused: the second caption stretched over a third
    s.set_playhead(sec(40.0));
    s.execute("captions.add", json!({"text": "third"})).expect("caption in free space");
    let third = s.active_sequence().unwrap().caption_tracks[0].captions[2].clone();
    let r = s.execute("captions.setTimes", json!({"caption": caps[1].id.0, "endTime": (third.start + sec(1.0)).0}));
    assert!(r.is_err() || s.active_sequence().unwrap().caption_tracks[0].captions[1].end() <= third.start, "no new overlap: {r:?}");
}

/// (f) A title alone on V2 with a fade in and out: dragged along its track, both fades go with it
/// (they used to vanish on release).
#[test]
fn a_clips_fades_go_with_it_when_dragged() {
    let (mut d, [a, _, _]) = tight();
    let s = &mut d.harness.state_mut().session;
    let g = s
        .edit_sequence("title", |seq, ctx, _| {
            let mut it = seq.video_tracks[0].items.iter().find(|i| i.id == a).expect("A").clone();
            *ctx.next_id += 1;
            it.id = ClipId(*ctx.next_id);
            (it.start, it.duration) = (sec(40.0), sec(10.0));
            seq.video_tracks[1].items.push(it.clone());
            Ok(it.id)
        })
        .expect("title");
    s.execute("trim.applyDefaultTransition", json!({"clips": [g.0]})).expect("fades");
    let fades = |d: &mut Driver| {
        let q = d.harness.state().session.active_sequence().unwrap().clone();
        let mut v: Vec<(Option<ClipId>, Option<ClipId>, Tick)> = q.video_tracks[1].transitions.iter().map(|t| (t.from, t.to, t.start)).collect();
        v.sort_by_key(|x| x.2);
        v
    };
    let before = fades(&mut d);
    assert_eq!(before.len(), 2, "a fade in and a fade out: {before:?}");
    d.frames(3);
    let shift = sec(2.0);
    let px = shift.seconds() as f32 * d.pps();
    let r = d.rect(&format!("timeline.clip.{}", g.0));
    let from = centre(r);
    d.drag(from, from + egui::vec2(px, 0.0));
    let (start, _) = d.clip(g).expect("the title");
    let moved = start - sec(40.0);
    assert!((moved - shift).0.abs() <= d.rate().frame_duration().0, "moved about 2 s: {:?} ({})", moved.seconds(), d.status());
    let after = fades(&mut d);
    assert_eq!(after, before.iter().map(|(f, t, s)| (*f, *t, *s + moved)).collect::<Vec<_>>(), "the fades went with it");
}

/// (g) A shot with its sound (V1 + A1, linked) and music under it on A2: the shot dragged up to V2
/// goes there, its sound stays on A1, and the music on A2 is not cut (the sound used to follow
/// the picture one track down, onto A2, and overwrite part of the music).
#[test]
fn a_shot_dragged_up_a_track_leaves_its_sound_on_its_track() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).expect("demo project");
    s.execute("sequence.addTracks", json!({"video": 1, "audio": 1})).expect("tracks");
    let (shot, sound, music) = s
        .edit_sequence("fixture", |seq, ctx, _| {
            let vproto = seq.video_tracks[0].items[0].clone();
            let aproto = seq.audio_tracks.iter().flat_map(|t| t.items.iter()).next().expect("an audio clip").clone();
            for tr in seq.all_tracks_mut() {
                tr.items.clear();
                tr.transitions.clear();
            }
            seq.markers.clear();
            (seq.mark_in, seq.mark_out) = (None, None);
            let mut next = || {
                *ctx.next_id += 1;
                ClipId(*ctx.next_id)
            };
            let (shot, sound, music) = (next(), next(), next());
            let mut v = vproto.clone();
            (v.id, v.start, v.duration, v.source_in, v.link, v.group) = (shot, sec(10.0), sec(8.0), Tick::ZERO, Some(7), None);
            let mut a = aproto.clone();
            (a.id, a.start, a.duration, a.source_in, a.link, a.group) = (sound, sec(10.0), sec(8.0), Tick::ZERO, Some(7), None);
            let mut m = aproto.clone();
            (m.id, m.start, m.duration, m.source_in, m.link, m.group, m.name) = (music, sec(0.0), sec(40.0), Tick::ZERO, None, None, "music".into());
            seq.video_tracks[0].items.push(v);
            seq.audio_tracks[0].items.push(a);
            seq.audio_tracks[1].items.push(m);
            Ok((shot, sound, music))
        })
        .expect("fixture");
    s.state.snapping = true;
    s.set_playhead(sec(60.0));
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"focused": "Timeline"}));
    d.ok("ui.menu.invoke", json!({"id": "window.maximizeFrame"}));
    d.ok("ui.menu.invoke", json!({"id": "view.zoomToSequence"}));
    d.frames(6);
    let from = centre(d.rect(&format!("timeline.clip.{}", shot.0)));
    let v2 = centre(d.rect("timeline.track.V2.target"));
    d.drag(from, egui::pos2(from.x, v2.y));
    let seq = d.harness.state().session.active_sequence().unwrap().clone();
    let on = |id: ClipId| seq.find_item(id).map(|(t, it)| (t, it.start, it.duration)).expect("still there");
    assert_eq!(on(shot), (seq.video_tracks[1].id, sec(10.0), sec(8.0)), "the shot went up to V2 ({})", d.status());
    assert_eq!(on(sound), (seq.audio_tracks[0].id, sec(10.0), sec(8.0)), "its sound stayed on A1");
    assert_eq!(on(music), (seq.audio_tracks[1].id, sec(0.0), sec(40.0)), "the music on A2 is whole");
    assert_eq!(seq.audio_tracks[1].items.len(), 1, "nothing was cut out of the music");
}
