//! Headless UI tests of the monitors' Button Editor (#431): the "+" under the Source and Program
//! monitors opens it, ticking a button adds it to that monitor's transport row only, Cancel keeps
//! the row, Reset Layout brings back the default.

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

impl Driver {
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(s).with_control(rx);
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

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }

    /// Load a clip into the Source monitor (its transport row shows only then).
    fn open_source(&mut self) {
        let item = self.harness.state().session.project.items.values().find(|i| i.as_media().is_some()).unwrap().id.0;
        self.ok("engine.execute", json!({"command": "source.open", "params": {"item": item}}));
        self.frames(3);
        assert!(self.has("source.transport.buttonEditor"));
    }

    fn has(&mut self, id: &str) -> bool {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == json!(id))
    }
}

#[test]
fn plus_opens_the_editor_and_ok_adds_the_button_to_that_monitor_only() {
    let mut d = Driver::new();
    d.open_source();
    assert!(!d.has("program.transport.playback.loop"), "Loop is not in the default row");
    d.click("program.transport.buttonEditor");
    assert!(d.has("buttonEditor.ok"), "the + opens the Button Editor");
    d.click("buttonEditor.item.playback.loop");
    d.click("buttonEditor.ok");
    assert!(!d.has("buttonEditor.ok"), "OK closes it");
    assert!(d.has("program.transport.playback.loop"), "Loop is on the Program row");
    assert!(!d.has("source.transport.playback.loop"), "the Source row is unchanged");
    // the new button works
    d.click("program.transport.playback.loop");
    assert!(d.harness.state().playback.looping);
    // and the layout is UI state (saved with the workspace, visible to agents)
    let ui = d.ok("ui.inspect", json!({}));
    assert!(ui.to_string().contains("playback.loop"), "{ui}");
}

#[test]
fn cancel_keeps_the_row_and_reset_restores_the_default() {
    let mut d = Driver::new();
    d.open_source();
    d.click("source.transport.buttonEditor");
    d.click("buttonEditor.item.source.insert");
    d.click("buttonEditor.cancel");
    assert!(d.has("source.transport.source.insert"), "Cancel leaves the row as it was");

    d.click("source.transport.buttonEditor");
    d.click("buttonEditor.item.source.insert");
    d.click("buttonEditor.ok");
    assert!(!d.has("source.transport.source.insert"), "unticked buttons leave the row");

    d.click("source.transport.buttonEditor");
    d.click("buttonEditor.reset");
    d.click("buttonEditor.ok");
    assert!(d.has("source.transport.source.insert"));
    assert!(d.harness.state().ui.source.buttons.is_none(), "the default row is stored as the default");
}
