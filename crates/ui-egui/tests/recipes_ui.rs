//! Headless UI tests of AI ▸ Recipes…: the window lists the built-in recipes, Run applies one to the
//! open sequence (one undo step), and recording counts steps and can be discarded.

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::{Transcript, Word};
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
    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(2);
    }
    fn has(&mut self, id: &str) -> bool {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == json!(id))
    }
    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }
}

/// The demo project with a fake transcriber for its first A1 clip.
fn session() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let sin = s.active_sequence().unwrap().audio_tracks[0].items[0].source_in;
    let mut t = Transcript { language: "en".into(), ..Default::default() };
    for (text, a, b) in [("Hello", 0.2, 0.5), ("there", 0.6, 0.9), ("world.", 1.0, 1.4)] {
        t.words.push(Word::new(text, sin + Tick::from_seconds_f64(a), sin + Tick::from_seconds_f64(b)));
    }
    t.normalize();
    s.transcriber = Some(Arc::new(filmcraft_speech::FixedTranscriber { transcript: t, id: "fixed".into() }));
    s
}

#[test]
fn run_a_built_in_recipe_and_discard_a_recording() {
    let mut d = Driver::new(session());
    d.ok("ui.menu.invoke", json!({"id": "ai.recipes"}));
    d.frames(3);
    assert!(d.has("recipes.item.Karaoke Captions") && d.has("recipes.item.Shorts Pack"));
    d.click("recipes.item.Karaoke Captions");
    assert!(d.has("recipes.step.2.enabled"), "three steps listed");
    let undo0 = d.app().session.history.undo.len();
    d.click("recipes.run");
    for _ in 0..400 {
        d.frames(1);
        if d.app().ui.recipes.job.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let status = d.exec("recipes.status", json!({}));
    let app = d.app();
    assert!(app.ui.recipes.message.contains("done"), "message {:?}, status {status}, ui status {:?}", app.ui.recipes.message, app.ui.status);
    let q = app.session.active_sequence().unwrap();
    assert_eq!(q.caption_tracks[0].style, filmcraft_engine::caption_presets::find("karaoke-yellow").unwrap().style);
    assert_eq!(app.session.history.undo.len(), undo0 + 1, "one undo step");

    // recording: steps are counted; Stop and Save needs a name; Discard keeps nothing
    d.click("recipes.record");
    assert!(d.app().session.recording.is_some());
    d.exec("captions.newTrack", json!({}));
    d.frames(2);
    assert_eq!(d.app().session.recording.as_ref().map(Vec::len), Some(1));
    assert!(d.app().ui.recipes.record_name.is_empty());
    d.click("recipes.discardRecording");
    assert!(d.app().session.recording.is_none());
}
