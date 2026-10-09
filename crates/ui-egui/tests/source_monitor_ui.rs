//! Headless UI tests of the Source monitor items in #316: Play starts playback, Insert places
//! only the In–Out range, Step Forward moves one frame.

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

fn source_state(d: &mut Driver) -> Value {
    d.ok("engine.execute", json!({"command": "source.inspect", "params": {}}))
}

#[test]
fn play_starts_source_playback() {
    let mut d = Driver::new();
    d.open_source();
    d.click("source.transport.source.playback.toggle");
    assert!(d.harness.state().source_playback.clock.playing, "Play starts the Source monitor");
    d.click("source.transport.source.playback.toggle");
    assert!(!d.harness.state().source_playback.clock.playing);
}

#[test]
fn step_forward_moves_one_frame() {
    let mut d = Driver::new();
    d.open_source();
    d.ok("engine.execute", json!({"command": "source.setPlayhead", "params": {"frame": 10}}));
    d.frames(2);
    let st = source_state(&mut d);
    let (before, fps) = (st["playhead"].as_i64().unwrap(), st["fps"].as_f64().unwrap());
    let frame = (254_016_000_000.0 / fps).round() as i64;
    d.click("source.transport.src.stepFwd");
    let after = source_state(&mut d)["playhead"].as_i64().unwrap();
    assert!((after - before - frame).abs() <= 1, "one frame forward: {before} → {after}");
    d.click("source.transport.src.stepBack");
    assert!((source_state(&mut d)["playhead"].as_i64().unwrap() - before).abs() <= 1, "and back");
}

#[test]
fn insert_places_only_the_in_to_out_range() {
    let mut d = Driver::new();
    d.open_source();
    d.ok("engine.execute", json!({"command": "source.setPlayhead", "params": {"frame": 12}}));
    d.click("source.transport.src.markIn");
    d.ok("engine.execute", json!({"command": "source.setPlayhead", "params": {"frame": 36}}));
    d.click("source.transport.src.markOut");
    let st = source_state(&mut d);
    let item = st["item"].as_u64().unwrap();
    let in_ticks = st["markIn"].as_i64().unwrap();
    let before = all_clips(&mut d);
    d.click("source.transport.source.insert");
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    // inserting mid-clip also splits the clip under the playhead: find the inserted one by source
    let inserted: Vec<(i64, i64)> = seq["video"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["items"].as_array().unwrap())
        .filter(|c| !before.contains(&c["clip"].as_u64().unwrap()) && c["item"].as_u64() == Some(item) && c["sourceIn"].as_i64() == Some(in_ticks))
        .map(|c| (c["durationFrames"].as_i64().unwrap(), c["sourceIn"].as_i64().unwrap()))
        .collect();
    assert_eq!(inserted.len(), 1, "one clip from the Source In point: {inserted:?}");
    assert_eq!(inserted[0].0, 25, "In 12 to Out 36 inclusive, not the whole clip");
}

fn all_clips(d: &mut Driver) -> Vec<u64> {
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    seq["video"].as_array().unwrap().iter().flat_map(|t| t["items"].as_array().unwrap().iter().map(|c| c["clip"].as_u64().unwrap())).collect()
}
