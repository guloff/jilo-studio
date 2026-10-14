//! Headless UI test of the notification when a long job ends while the app is in the background:
//! a fake host hook receives it; in front, or with the setting off, nothing is posted.

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    background: bool,
}

impl Driver {
    fn new(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, background: false };
        d.frames(4);
        d
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            if self.background {
                self.harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().focused = Some(false);
            }
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

/// Export the demo sequence as WAV into `dir` and wait for the job to end.
fn export(d: &mut Driver, dir: &std::path::Path, name: &str) {
    let path = dir.join(name);
    d.app().session.execute("export.queue.add", json!({"preset": "Waveform Audio 48 kHz 16-bit", "path": path.to_string_lossy()})).unwrap();
    d.app().session.execute("export.queue.start", json!({})).unwrap();
    for _ in 0..2000 {
        d.frames(1);
        let jobs: Vec<Option<u64>> = d.harness.state().session.export_queue.items.iter().map(|i| i.job).collect();
        if jobs.iter().all(|j| j.is_some_and(|j| d_finished(d, j))) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    d.frames(2);
}

fn d_finished(d: &Driver, job: u64) -> bool {
    d.harness.state().session.jobs.iter().find(|j| j.id == job).is_some_and(|j| j.to_json()["finished"] == true)
}

#[test]
fn a_job_ending_in_the_background_posts_a_notification() {
    let dir = std::env::temp_dir().join(format!("jilo-notify-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    let posted: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = posted.clone();
    d.app().hooks.notify = Some(Box::new(move |t: &str, b: &str| sink.lock().unwrap().push((t.to_string(), b.to_string()))));
    d.app().notify_watch.min_seconds = 0.0;
    // in front: nothing
    export(&mut d, &dir, "front.wav");
    assert!(posted.lock().unwrap().is_empty());
    // in the background: one notification
    d.background = true;
    export(&mut d, &dir, "back.wav");
    {
        let p = posted.lock().unwrap();
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].0, "Jilo Studio");
        assert!(p[0].1.contains("done"), "{:?}", p[0]);
    }
    // the setting off: nothing more
    d.app().session.prefs.general.notify_when_done = false;
    export(&mut d, &dir, "off.wav");
    assert_eq!(posted.lock().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
