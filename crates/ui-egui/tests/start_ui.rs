//! Headless UI test of the start from a video: clips offered on the home screen get "What do you
//! want to make?"; without a Whisper model the choices that transcribe say so and Just Edit opens
//! a sequence from the clips; with one, a choice starts the batch for it.
//! `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `start-dialog.png` (`FILMCRAFT_UI_LANG=ru`: Russian).

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::state::Mode;
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

fn session() -> (Session, u64) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let ocean = s.project.items.iter().find(|(_, i)| i.name.starts_with("Ocean")).map(|(id, _)| id.0).unwrap();
    (s, ocean)
}

#[test]
fn just_edit_opens_a_sequence_from_the_clips() {
    let (s, ocean) = session();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.app().ui.mode = Mode::Import;
    d.app().ui.start.speech_ready = Some(false);
    filmcraft_ui_egui::panels::start::offer(d.app(), vec![ocean]);
    d.frames(3);
    assert!(d.has("start.dialog"));
    snapshot(&mut d, "start-dialog");
    let before = d.app().session.state.active_sequence;
    d.click("start.choice.short");
    assert!(d.has("start.dialog"), "needs Whisper: not chosen");
    d.click("start.choice.edit");
    let app = d.app();
    assert!(app.ui.start.items.is_empty());
    assert_eq!(app.ui.mode, Mode::Edit);
    let q = app.session.active_sequence().unwrap();
    assert_ne!(app.session.state.active_sequence, before, "a new sequence is open");
    assert!(q.video_tracks[0].items.iter().any(|i| i.item.0 == ocean));
}

#[test]
fn a_choice_starts_its_batch() {
    let (s, ocean) = session();
    let mut d = Driver::new(s);
    d.app().ui.mode = Mode::Import;
    d.app().ui.start.speech_ready = Some(true);
    filmcraft_ui_egui::panels::start::offer(d.app(), vec![ocean]);
    d.frames(3);
    d.click("start.choice.short");
    let app = d.app();
    assert!(app.ui.start.items.is_empty());
    // with a Whisper model the batch runs; this build's test has none, so batch.start says why
    match app.session.execute("batch.status", json!({})) {
        Ok(st) => assert_eq!(st["steps"], json!(["transcribe", "pauses", "captions", "formats"]), "{st}"),
        Err(_) => assert!(!app.ui.status.is_empty(), "the reason is shown"),
    }
    let p = filmcraft_ui_egui::panels::start::steps("short").unwrap();
    assert_eq!((p["formats"].clone(), p["aspects"].clone(), p["captionPreset"].clone()), (json!(true), json!(["9:16"]), json!("karaoke-yellow")));
    let p = filmcraft_ui_egui::panels::start::steps("talk").unwrap();
    assert_eq!((p["fillers"].clone(), p["formats"].clone()), (json!(true), Value::Null));
    assert!(filmcraft_ui_egui::panels::start::steps("edit").is_none());
}
