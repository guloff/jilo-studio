//! Headless UI tests of the command palette (⌘J) — it opens, finds commands, panels and recipes by a
//! few letters, runs the chosen one with Return and closes with Escape — and of the quick actions
//! over the Program monitor for the selection.

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

fn key(d: &mut Driver, k: &str) {
    d.ok("ui.key", json!({"key": k}));
    d.frames(2);
}

#[test]
fn palette_finds_and_runs_commands() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    key(&mut d, "Cmd+J");
    assert!(d.app().ui.palette.open, "⌘J opens the palette");
    d.ok("ui.type", json!({"text": "caption sty"}));
    d.frames(3);
    let first = d.ok("ui.elements", json!({"prefix": "palette.item.0"}));
    let chosen = first[0]["label"].as_str().unwrap().to_string();
    assert!(chosen.starts_with("Caption Style"), "{first}");
    key(&mut d, "Enter");
    assert!(!d.app().ui.palette.open, "Return runs and closes");
    let inspect = d.ok("ui.inspect", json!({}));
    assert!(inspect.to_string().contains("CaptionStyle"), "the Caption Style panel is shown");
    // letters with gaps, recipes, and Escape
    key(&mut d, "Cmd+J");
    d.ok("ui.type", json!({"text": "rmpaus"}));
    d.frames(3);
    let first = d.ok("ui.elements", json!({"prefix": "palette.item.0"}));
    // available commands come first (Remove Pauses needs a transcript; the demo has none)
    assert_eq!(first[0]["label"], "Remove Silent Pauses", "{first}");
    d.ok("ui.key", json!({"key": "Cmd+A"}));
    d.ok("ui.type", json!({"text": "karaoke capt"}));
    d.frames(3);
    let labels = d.ok("ui.elements", json!({"prefix": "palette.item."}));
    assert!(labels.to_string().contains("Run Recipe: Karaoke Captions"), "{labels}");
    key(&mut d, "Escape");
    assert!(!d.app().ui.palette.open);
    // the command run first comes first with nothing typed
    key(&mut d, "Cmd+J");
    let first = d.ok("ui.elements", json!({"prefix": "palette.item.0"}));
    assert_eq!(first[0]["label"].as_str().unwrap(), chosen, "{first}");
}

#[test]
fn quick_actions_follow_the_selection() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    let mut d = Driver::new(s);
    d.exec("playhead.set", json!({"seconds": 1}));
    d.frames(3);
    assert!(!d.has("quick.sequence.addEdit"), "nothing selected, no row");
    d.exec("timeline.select", json!({"clips": [clip.0]}));
    d.frames(3);
    assert!(d.has("quick.sequence.addEdit") && d.has("quick.clip.speedDuration") && d.has("quick.clip.reframeToFace"));
    let n = d.app().session.active_sequence().unwrap().video_tracks[0].items.len();
    d.click("quick.sequence.addEdit");
    assert!(d.app().session.active_sequence().unwrap().video_tracks[0].items.len() > n, "Split cut the clip at the playhead");
    // captions get their own actions
    d.exec("captions.add", json!({"text": "Hi", "seconds": 1}));
    let cap = d.app().session.active_sequence().unwrap().caption_tracks[0].captions[0].id;
    d.exec("captions.select", json!({"captions": [cap.0]}));
    d.frames(3);
    assert!(d.has("quick.graphics.captionStyles") && d.has("quick.captions.translate"));
    // View ▸ Quick Actions turns the row off
    d.ok("ui.menu.invoke", json!({"id": "view.quickActions"}));
    d.frames(3);
    assert!(!d.has("quick.graphics.captionStyles"));
}
