//! Headless UI test of the caption style preview: hovering a preset tile in the Caption Style panel
//! shows its look in the paused Program monitor without changing the project; moving away ends it.
//! `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `caption-hover.png` (`FILMCRAFT_UI_LANG=ru`: Russian).

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
fn hovering_a_style_previews_it_in_the_program_monitor() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let at = s.playhead().seconds();
    s.execute("captions.add", json!({"seconds": at - 0.5, "durationSeconds": 3.0, "text": "Preview me"})).unwrap();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "graphics.captionStyles"}));
    d.frames(4);
    let before = d.app().session.project.clone();
    d.ok("ui.move", json!({"id": "captionStyle.preset.yellow-pop"}));
    d.frames(4);
    snapshot(&mut d, "caption-hover");
    let app = d.app();
    let (_, preview) = app.ui.caption_hover_project.clone().expect("the monitor renders a preview");
    let seq = app.session.state.active_sequence.unwrap();
    let want = filmcraft_engine::caption_presets::find("yellow-pop").unwrap().style;
    assert_eq!(preview.sequence(seq).unwrap().caption_tracks[0].style, want);
    assert!(std::sync::Arc::ptr_eq(&app.session.project, &before), "the project itself is unchanged");
    // away from the tiles: the preview ends
    d.ok("ui.move", json!({"x": 700.0, "y": 940.0}));
    d.frames(4);
    assert!(d.app().ui.caption_hover.is_none());
}
