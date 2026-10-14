//! Headless UI test of Settings ▸ AI ▸ on this computer: the local models with their sizes and a
//! Download or Delete button, and the cloud consents forgotten with Ask Again.
//! `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `ai-local.png` (`FILMCRAFT_UI_LANG=ru`: Russian).

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
fn local_models_and_consents_in_ai_settings() {
    let mut s = Session::default();
    s.prefs.ai.confirmed_providers = vec!["gemini".into()];
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "ai.settings"}));
    d.frames(4);
    let models = d.app().session.execute("ai.localModels", json!({})).unwrap();
    for m in models["models"].as_array().unwrap() {
        let id = m["id"].as_str().unwrap();
        let what = if m["installed"] == true { "delete" } else { "download" };
        assert!(d.has(&format!("settings.aiLocal.{id}.{what}")), "{id} {what}");
    }
    // scroll the page (from a point in it) until Ask Again is in the dialog's view
    for _ in 0..20 {
        let r = d.ok("ui.elements", json!({"prefix": "settings.aiLocal.resetConsents"}));
        let y = r[0]["rect"][1].as_f64().unwrap_or(0.0);
        if (200.0..740.0).contains(&y) {
            break;
        }
        let dy = if y >= 740.0 { -120.0 } else { 120.0 };
        d.ok("ui.scroll", json!({"x": 880.0, "y": 480.0, "dy": dy}));
        d.frames(2);
    }
    snapshot(&mut d, "ai-local");
    d.click("settings.aiLocal.resetConsents");
    let msg = d.app().ui.settings.as_ref().map(|s| s.message.clone()).unwrap_or_default();
    assert!(d.app().session.prefs.ai.confirmed_providers.is_empty(), "{msg}");
    let draft = d.app().ui.settings.as_ref().map(|s| s.values["ai"]["confirmedProviders"].clone());
    assert_eq!(draft, Some(json!([])), "OK will not bring them back");
}
