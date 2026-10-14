//! Headless UI test of AI ▸ Automatic Editing ▸ Auto B-roll…: with the clips chosen in the Project
//! panel, the dialog places the one named after what is said above the phrase, one undo step.
//!
//! `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `broll-dialog.png` (`FILMCRAFT_UI_LANG=ru`: Russian).

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_time::Tick;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

fn snapshot(d: &mut Driver, name: &str) {
    let Some(dir) = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from) else { return };
    d.frames(2);
    match d.harness.render() {
        Ok(img) => {
            std::fs::create_dir_all(&dir).unwrap();
            img.save(dir.join(format!("{name}.png"))).unwrap();
        }
        Err(e) => eprintln!("snapshot {name} skipped: {e}"),
    }
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

#[test]
fn the_dialog_places_footage_that_fits_the_phrase() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let a1 = s.active_sequence().unwrap().audio_tracks[0].items[0].clone();
    let at = |x: f64| (a1.source_in + Tick::from_seconds_f64(x)).0;
    let words = json!([
        {"text": "Look", "start": at(0.0), "end": at(0.3)}, {"text": "at", "start": at(0.3), "end": at(0.4)},
        {"text": "the", "start": at(0.4), "end": at(0.5)}, {"text": "aurora.", "start": at(0.5), "end": at(1.2)},
    ]);
    s.execute("transcript.set", json!({"item": a1.item.0, "transcript": {"language": "en", "speakers": [], "words": words}})).unwrap();
    let aurora = s.project.items.iter().find(|(_, i)| i.name.starts_with("Aurora")).map(|(id, _)| id.0).unwrap();
    let ocean = s.project.items.iter().find(|(_, i)| i.name.starts_with("Ocean")).map(|(id, _)| id.0).unwrap();
    s.execute("project.select", json!({"items": [ocean, aurora]})).unwrap();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"command": "broll.auto"}));
    d.frames(3);
    assert!(d.has("broll.dialog"), "{}", d.app().ui.status);
    snapshot(&mut d, "broll-dialog");
    let undo = d.app().session.history.undo.len();
    d.click("broll.go");
    d.frames(2);
    let app = d.app();
    assert_eq!(app.session.history.undo.len(), undo + 1, "{}", app.ui.status);
    let q = app.session.active_sequence().unwrap();
    let placed: Vec<u64> = q.video_tracks[1].items.iter().filter(|i| i.start < a1.start + Tick::from_seconds_f64(1.0)).map(|i| i.item.0).collect();
    assert_eq!(placed, [aurora], "{}", app.ui.status);
}
