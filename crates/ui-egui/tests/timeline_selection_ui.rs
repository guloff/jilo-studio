//! Selecting in the Timeline as in Premiere Pro: a marquee can start on any empty space, also off
//! the tracks (above the video tracks, below the audio ones), and takes every clip it touches.

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
    /// The demo project, the Timeline maximized and zoomed to the sequence.
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.state.selection.clear();
        s.set_playhead(Tick::from_seconds_f64(20.0));
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d.ok("ui.set", json!({"focused": "Timeline"}));
        d.ok("ui.menu.invoke", json!({"id": "window.maximizeFrame"}));
        d.ok("ui.menu.invoke", json!({"id": "view.zoomToSequence"}));
        d.frames(6);
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

    fn hit(&mut self, p: egui::Pos2) -> String {
        self.ok("ui.timeline.hit", json!({"x": p.x, "y": p.y}))["hit"].as_str().unwrap_or_default().to_string()
    }

    fn send(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    /// Press, move in steps, release (a click when `from` and `to` are the same point).
    fn gesture(&mut self, from: egui::Pos2, to: egui::Pos2) {
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        self.send(egui::Event::PointerMoved(from));
        self.send(button(from, true));
        if from != to {
            for k in 1..=12 {
                self.send(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
            }
        }
        self.send(button(to, false));
        self.frames(3);
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn selection(&mut self) -> Vec<ClipId> {
        self.app().session.state.selection.clone()
    }

    /// The first clip of a video (`V`) or audio (`A`) track, and its rect on screen.
    fn first_clip(&mut self, video: bool, track: usize) -> (ClipId, [f32; 4]) {
        let seq = self.app().session.active_sequence().unwrap().clone();
        let tracks = if video { &seq.video_tracks } else { &seq.audio_tracks };
        let id = tracks[track].items.iter().min_by_key(|i| i.start).unwrap().id;
        (id, self.rect(&format!("timeline.clip.{}", id.0)))
    }
}

#[test]
fn a_marquee_starts_above_the_video_tracks() {
    let mut d = Driver::new();
    let (clip, r) = d.first_clip(true, 0);
    let top_track = d.app().session.active_sequence().unwrap().video_tracks.len();
    let top = d.rect(&format!("timeline.track.V{top_track}.sourcePatch"));
    // empty space above the highest video track, off every track
    let from = egui::pos2(r[0] + 4.0, top[1] - 12.0);
    assert_eq!(d.hit(from), "none", "the marquee starts off the tracks");
    let to = egui::pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    d.gesture(from, to);
    assert!(d.selection().contains(&clip), "the V1 clip the marquee touches is selected: {:?} ({})", d.selection(), d.app().ui.status);
}

#[test]
fn a_marquee_starts_below_the_audio_tracks() {
    let mut d = Driver::new();
    let (clip, r) = d.first_clip(false, 0);
    let n = d.app().session.active_sequence().unwrap().audio_tracks.len();
    let last = d.rect(&format!("timeline.track.A{n}.sourcePatch"));
    let from = egui::pos2(r[0] + 4.0, last[1] + last[3] + 14.0);
    assert_eq!(d.hit(from), "none", "the marquee starts off the tracks");
    let to = egui::pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    d.gesture(from, to);
    assert!(d.selection().contains(&clip), "the A1 clip the marquee touches is selected: {:?} ({})", d.selection(), d.app().ui.status);
}

#[test]
fn a_click_off_the_tracks_clears_the_selection() {
    let mut d = Driver::new();
    let (clip, r) = d.first_clip(true, 0);
    d.app().session.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
    let top_track = d.app().session.active_sequence().unwrap().video_tracks.len();
    let top = d.rect(&format!("timeline.track.V{top_track}.sourcePatch"));
    let p = egui::pos2(r[0] + 4.0, top[1] - 12.0);
    d.gesture(p, p);
    assert!(d.selection().is_empty(), "{:?}", d.selection());
}
