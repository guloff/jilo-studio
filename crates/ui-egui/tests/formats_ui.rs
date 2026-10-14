//! Headless UI test of File ▸ Export ▸ Export in All Formats…: the dialog opens from the menu,
//! the chosen formats become copies of the sequence in the export queue, the original stays open.

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
fn export_in_all_formats_queues_a_copy_per_chosen_format() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let src = s.state.active_sequence.unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.menu.invoke", json!({"command": "export.allFormats"}));
    d.frames(4);
    assert!(d.has("formats.dialog"), "the dialog is open");
    // 16:9, 9:16 and 1:1 are chosen to start with; add 4:5, leave out 1:1
    d.click("formats.aspect.4:5");
    d.click("formats.aspect.1:1");
    d.app().ui.formats.preset = "Waveform Audio 48 kHz 16-bit".into();
    d.click("formats.start");
    d.click("formats.go");
    d.frames(2);
    assert!(!d.has("formats.dialog"), "the dialog closes");
    let app = d.app();
    assert_eq!(app.session.state.active_sequence, Some(src), "the original stays open");
    let frames: Vec<(u32, u32)> =
        app.session.export_queue.items.iter().filter_map(|i| app.session.project.sequence(i.sequence).map(|q| (q.settings.width, q.settings.height))).collect();
    assert_eq!(frames, [(1920, 1080), (1080, 1920), (1080, 1350)], "{}", app.ui.status);
    assert!(!app.session.export_queue.running, "the formats wait in the queue");
    assert!(app.session.export_queue.items.iter().all(|i| i.preset == "Waveform Audio 48 kHz 16-bit"));
}
