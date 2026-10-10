//! Headless UI test of the window after a run that ended without quitting (`crash::begin_session`).
//! Its own test binary: the report it sets is process-global, and any other window test running
//! at the same time would take it.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        // FILMCRAFT_UI_SNAPSHOT_DIR=<dir>: also render the window with wgpu and write ai-*.png there
        let snapshots = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
        if let Ok(lang) = std::env::var("FILMCRAFT_UI_SNAPSHOT_LANG") {
            d.ok("engine.execute", json!({"command": "prefs.set", "params": {"key": "general.interfaceLanguage", "value": lang}}));
        }
        // FILMCRAFT_UI_SNAPSHOT_THEME=light|dark: the appearance the snapshots are taken in
        if let Ok(mode) = std::env::var("FILMCRAFT_UI_SNAPSHOT_THEME") {
            d.ok("engine.execute", json!({"command": "prefs.set", "params": {"key": "appearance.appearanceMode", "value": mode}}));
        }
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

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(3);
        match self.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                img.save(dir.join(format!("ai-{name}.png"))).unwrap();
            }
            Err(e) => eprintln!("snapshot {name} skipped: {e}"),
        }
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }
}

/// The window about a previous run that ended without quitting: its report, Copy Report, Close.
#[test]
fn a_previous_crash_is_reported_in_a_window() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    filmcraft_ui_egui::crash::set_previous(Some(
        "Jilo Studio 0.6.0 (started at unix 1791000000) quit unexpectedly.\nThis run: Jilo Studio 0.6.0 on macOS.\n\n=== filmcraft FilmCraft 0.6.0 (unix 1791000420)\nindex out of bounds: the len is 3 but the index is 7 (at crates/render/src/lib.rs:812, thread filmcraft-frame-2)\n   0: std::backtrace::Backtrace::force_capture\n   1: filmcraft_ui_egui::crash::install::{{closure}}\n\nmacOS crash report: ~/Library/Logs/DiagnosticReports/Jilo Studio-2026-10-11-101502.ips".into(),
    ));
    let mut d = Driver::new(s);
    d.frames(3);
    assert!(d.ids("crash.").contains(&"crash.copy".to_string()));
    d.snapshot("crash-report");
    d.click("crash.copy");
    d.click("crash.close");
    assert!(d.ids("crash.").is_empty());
    assert!(filmcraft_ui_egui::crash::take_previous().is_none(), "shown once");
}
