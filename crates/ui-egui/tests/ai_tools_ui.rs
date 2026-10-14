//! Headless UI test of the AI Tools panel: each feature's card says whether it can run and why
//! not; Do It runs the one-click ones (Auto Mix here) and Customize… opens the dialog.
//! `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `ai-tools.png` (`FILMCRAFT_UI_LANG=ru`: Russian).

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
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
fn the_ai_panel_runs_and_opens_features() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "ai.tools"}));
    d.frames(4);
    assert!(d.has("aiTools.card.audio.autoMix"));
    // no transcript: Captions from the Transcript cannot run
    let r = d.ok("ui.elements", json!({"prefix": "aiTools.do.transcript.createCaptions"}));
    assert!(!r.as_array().unwrap().is_empty());
    snapshot(&mut d, "ai-tools");
    // scroll the panel to the Sound group
    for _ in 0..30 {
        let r = d.ok("ui.elements", json!({"prefix": "aiTools.do.audio.autoMix"}));
        let y = r[0]["rect"][1].as_f64().unwrap_or(0.0);
        if (90.0..520.0).contains(&y) {
            break;
        }
        let rect = d.ok("ui.elements", json!({"prefix": "panel.AiTools"}));
        let (x, y0) = (rect[0]["rect"][0].as_f64().unwrap_or(1200.0) + 100.0, rect[0]["rect"][1].as_f64().unwrap_or(100.0) + 200.0);
        d.ok("ui.scroll", json!({"x": x, "y": y0, "dy": -120.0}));
        d.frames(2);
    }
    let undo = d.app().session.history.undo.len();
    d.click("aiTools.do.audio.autoMix");
    d.frames(2);
    assert_eq!(d.app().session.history.undo.len(), undo + 1, "{}", d.app().ui.status);
    let app = d.app();
    assert_ne!(app.session.active_sequence().unwrap().master_volume_db, 0.0, "{}", app.ui.status);
    // Customize… shows the feature's dialog
    d.click("aiTools.open.audio.autoMix");
    d.frames(2);
    assert!(d.app().ui.auto_mix.dialog);
}
