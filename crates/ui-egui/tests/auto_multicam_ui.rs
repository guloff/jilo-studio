//! Headless UI test of AI ▸ Automatic Editing ▸ Auto Multi-Camera by Speaker…: the dialog cuts a
//! multi-camera clip where the transcript's speaker changes, as one undo step.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ItemKind;
use filmcraft_time::Tick;
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
fn the_dialog_cuts_where_the_speaker_changes() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    // two of the demo's clips as cameras of a multi-camera source, cut into a new sequence
    let cams: Vec<u64> = s.project.items.iter().filter(|(_, i)| matches!(i.kind, ItemKind::Media(_)) && i.has_video()).map(|(id, _)| id.0).take(2).collect();
    s.execute("project.select", json!({"items": cams})).unwrap();
    let r = s.execute("clip.createMulticam", json!({"items": cams, "method": "in"})).unwrap();
    let src = r["sequence"].as_u64().unwrap();
    s.execute("file.newSequence", json!({"name": "Talk", "video": 1, "audio": 1})).unwrap();
    s.execute("source.open", json!({"item": src})).unwrap();
    s.execute("source.overwrite", json!({})).unwrap();
    let w =
        |text: &str, sp: u32, a: f64, b: f64| json!({"text": text, "start": Tick::from_seconds_f64(a).0, "end": Tick::from_seconds_f64(b).0, "speaker": sp});
    let words = json!([w("One.", 0, 0.0, 1.9), w("Two.", 1, 2.0, 3.9)]);
    s.execute("transcript.set", json!({"item": cams[0], "transcript": {"language": "en", "speakers": [{"name": "A"}, {"name": "B"}], "words": words}}))
        .unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.menu.invoke", json!({"command": "multicam.autoSwitch"}));
    d.frames(3);
    assert!(d.has("autoMulticam.dialog"), "{}", d.app().ui.status);
    d.app().ui.auto_multicam.min_shot = 1.0;
    d.app().ui.auto_multicam.lead = 0.0;
    let undo = d.app().session.history.undo.len();
    d.click("autoMulticam.go");
    d.frames(2);
    let app = d.app();
    assert_eq!(app.session.history.undo.len(), undo + 1, "{}", app.ui.status);
    let q = app.session.active_sequence().unwrap();
    let angles: Vec<(f64, u32)> = q.video_tracks[0].items.iter().map(|i| (i.start.seconds(), i.multicam.unwrap().angle)).collect();
    assert_eq!(angles.len(), 2, "{angles:?}");
    assert_eq!(angles[0].1, 0);
    assert_eq!(angles[1].1, 1);
    assert!((angles[1].0 - 2.0).abs() < 0.05, "{angles:?}");
}
