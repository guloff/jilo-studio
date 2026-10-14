//! Headless UI tests of AI ▸ Batch Process…'s newer parts: a copy in 9:16 made and exported with
//! each sequence, and the steps kept for the inbox folder with nothing selected.

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
fn a_batch_exports_each_sequence_with_its_vertical_copy() {
    let dir = std::env::temp_dir().join(format!("jilo-batch-formats-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let main = s.state.active_sequence.unwrap();
    s.execute("project.select", json!({"items": [main.0]})).unwrap();
    // exports go next to the project: the scratch folder, not the user's Movies
    s.path = Some(dir.join("batch.fcproj").to_string_lossy().to_string());
    let mut d = Driver::new(s);
    d.ok("ui.menu.invoke", json!({"id": "batch.start"}));
    d.frames(3);
    assert!(d.has("batch.dialog"));
    // transcribe off; copies in other formats (9:16 to start with) and export on
    d.click("batch.step.transcribe");
    d.click("batch.step.formats");
    assert!(d.has("batch.format.9:16"));
    d.app().ui.batch.preset = "Waveform Audio 48 kHz 16-bit".into();
    snapshot(&mut d, "batch-steps");
    d.click("batch.go");
    for _ in 0..300 {
        d.frames(1);
        if d.app().session.batch.as_ref().is_some_and(|b| b.to_json()["finished"] == true) {
            break;
        }
    }
    let app = d.app();
    let st = app.session.batch.as_ref().unwrap().to_json();
    assert_eq!(st["items"][0]["done"], json!(["formats", "export"]), "{st}");
    let copy = st["items"][0]["formats"][0].as_u64().expect("a copy");
    let q = app.session.project.sequence(filmcraft_project::ItemId(copy)).unwrap();
    assert_eq!((q.settings.width, q.settings.height), (1080, 1920));
    let queued: Vec<u64> = app.session.export_queue.items.iter().map(|i| i.sequence.0).collect();
    assert_eq!(queued, [main.0, copy]);
    // stop the exports the batch started
    for id in app.session.export_queue.items.iter().map(|i| i.id).collect::<Vec<_>>() {
        let _ = app.session.execute("export.queue.cancel", json!({"id": id}));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_steps_are_kept_for_the_inbox_folder() {
    let dir = std::env::temp_dir().join(format!("jilo-inbox-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.state.project_selection.clear();
    let mut d = Driver::new(s);
    d.ok("ui.menu.invoke", json!({"id": "batch.start"}));
    d.frames(3);
    assert!(d.has("batch.dialog"), "opens with nothing selected, for the inbox");
    d.click("batch.step.fillers");
    d.click("batch.inbox");
    d.app().ui.batch.inbox_folder = dir.to_string_lossy().to_string();
    d.frames(1);
    d.click("batch.keep");
    let app = d.app();
    let inbox = &app.session.prefs.inbox;
    assert!(inbox.enabled, "{}", app.ui.status);
    assert_eq!(inbox.folder, dir.to_string_lossy());
    assert_eq!(inbox.batch["fillers"], true);
    assert_eq!(inbox.batch["transcribe"], true);
    assert!(inbox.batch.get("items").is_none());
    assert!(!app.ui.batch.dialog);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_steps_become_a_finder_quick_action() {
    let dir = std::env::temp_dir().join(format!("jilo-qa-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.menu.invoke", json!({"id": "batch.start"}));
    d.frames(3);
    d.click("batch.step.transcribe");
    d.click("batch.step.formats");
    d.app().ui.batch.quick_action = "Vertical Copy".into();
    d.app().ui.batch.quick_action_dir = Some(dir.to_string_lossy().to_string());
    d.frames(1);
    d.click("batch.quickAction");
    let doc = std::fs::read_to_string(dir.join("Vertical Copy.workflow/Contents/document.wflow")).unwrap();
    assert!(doc.contains("--process") && doc.contains("&quot;formats&quot;:true") && doc.contains("&quot;export&quot;:true"), "{doc}");
    assert!(d.app().ui.status.contains("Vertical Copy"), "{}", d.app().ui.status);
    let _ = std::fs::remove_dir_all(&dir);
}
