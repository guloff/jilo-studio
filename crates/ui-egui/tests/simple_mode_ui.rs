//! Headless UI test of Simple mode (View ▸ Simple Mode, the workspace menu, Settings ▸ General):
//! the Simple workspace and four tools, then back to the workspace before it; and the AI Editing
//! workspace. `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `simple-mode.png` and `ai-editing.png`
//! (`FILMCRAFT_UI_LANG=ru`: Russian).

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
fn simple_mode_shows_fewer_panels_and_tools_and_gives_the_workspace_back() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "window.workspace.audio"}));
    d.frames(2);
    assert!(d.has("tools.Ripple"));
    d.ok("ui.menu.invoke", json!({"id": "view.simpleMode"}));
    d.frames(3);
    snapshot(&mut d, "simple-mode");
    assert_eq!(d.app().ui.workspace, "Simple");
    assert!(d.app().session.prefs.general.simple_mode);
    for t in ["Selection", "Razor", "Hand", "Type"] {
        assert!(d.has(&format!("tools.{t}")), "{t}");
    }
    for t in ["Ripple", "Slip", "Pen", "TrackSelectForward"] {
        assert!(!d.has(&format!("tools.{t}")), "{t} is a Pro tool");
    }
    // off again: the workspace before it
    d.ok("ui.menu.invoke", json!({"id": "view.simpleMode"}));
    d.frames(2);
    assert_eq!(d.app().ui.workspace, "Audio");
    assert!(d.has("tools.Ripple"));
    // the AI Editing workspace
    d.ok("ui.menu.invoke", json!({"id": "window.workspace.aiediting"}));
    d.frames(3);
    snapshot(&mut d, "ai-editing");
    assert_eq!(d.app().ui.workspace, "AI Editing");
}
