//! Headless UI tests of the unsaved-changes prompt on quit (#266): closing the window with
//! unsaved changes is cancelled and asks Save / Don't Save / Cancel; a clean project closes at
//! once; agents quitting with `app.quit` are never stopped by the prompt.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::{Dialog, FilmcraftApp};
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    /// Viewport commands the app sent, oldest first.
    commands: Vec<egui::ViewportCommand>,
}

impl Driver {
    fn new() -> Self {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(s).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, commands: Vec::new() };
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
            if let Some(out) = self.harness.output().viewport_output.get(&egui::ViewportId::ROOT) {
                self.commands.extend(out.commands.iter().cloned());
            }
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

    fn has(&mut self, id: &str) -> bool {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == json!(id))
    }

    /// The window's close button (or Cmd+Q through egui): a close request for the root viewport.
    fn request_close(&mut self) {
        self.commands.clear();
        self.harness.input_mut().viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        // a few frames: the modal fades in and its buttons get their final rects
        self.frames(5);
    }

    fn sent(&self, c: &egui::ViewportCommand) -> bool {
        self.commands.iter().any(|x| x == c)
    }

    fn edit(&mut self) {
        self.ok("engine.execute", json!({"command": "sequence.addEdit", "params": {"seconds": 1.0}}));
        assert!(self.harness.state().session.is_dirty());
    }
}

#[test]
fn closing_a_clean_project_does_not_ask() {
    let mut d = Driver::new();
    assert!(!d.harness.state().session.is_dirty());
    d.request_close();
    assert!(!d.sent(&egui::ViewportCommand::CancelClose));
    assert_eq!(d.harness.state().dialog, None);
}

#[test]
fn closing_with_unsaved_changes_asks_and_cancel_stays() {
    let mut d = Driver::new();
    d.edit();
    d.request_close();
    assert!(d.sent(&egui::ViewportCommand::CancelClose), "the close must be held back: {:?}", d.commands);
    assert_eq!(d.harness.state().dialog, Some(Dialog::QuitConfirm));
    assert!(d.has("quit.save") && d.has("quit.dontSave") && d.has("quit.cancel"));

    d.commands.clear();
    d.click("quit.cancel");
    assert_eq!(d.harness.state().dialog, None);
    assert!(!d.sent(&egui::ViewportCommand::Close));
    assert!(d.harness.state().session.is_dirty(), "Cancel keeps the changes");
    assert!(!d.harness.state().quit_confirmed);
}

#[test]
fn dont_save_quits_and_drops_the_recovery_journal() {
    let mut d = Driver::new();
    d.edit();
    d.request_close();
    d.commands.clear();
    d.click("quit.dontSave");
    assert!(d.sent(&egui::ViewportCommand::Close), "{:?} dialog {:?} status {:?}", d.commands, d.harness.state().dialog, d.harness.state().ui.status);
    let app = d.harness.state();
    assert!(app.quit_confirmed && app.discard_on_exit);
    // The close the app sent itself goes through.
    d.request_close();
    assert!(!d.sent(&egui::ViewportCommand::CancelClose));
}

#[test]
fn save_writes_the_project_then_quits() {
    let mut d = Driver::new();
    let dir = std::env::temp_dir().join(format!("fc-quit-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("quit.fcproj");
    let picked = path.to_string_lossy().to_string();
    // Untitled: Save goes through the Save As dialog.
    d.harness.state_mut().hooks.pick_save = Some(Box::new(move |_| Some(picked.clone())));
    d.edit();
    d.request_close();
    d.commands.clear();
    d.click("quit.save");
    assert!(path.exists(), "Save As wrote the project");
    assert!(!d.harness.state().session.is_dirty());
    assert!(d.sent(&egui::ViewportCommand::Close), "{:?} dialog {:?} status {:?}", d.commands, d.harness.state().dialog, d.harness.state().ui.status);
    assert!(!d.harness.state().discard_on_exit);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cancelling_save_as_keeps_the_app_open() {
    let mut d = Driver::new();
    d.harness.state_mut().hooks.pick_save = Some(Box::new(|_| None));
    d.edit();
    d.request_close();
    d.commands.clear();
    d.click("quit.save");
    assert!(!d.sent(&egui::ViewportCommand::Close));
    assert!(d.harness.state().session.is_dirty());
    assert!(!d.harness.state().quit_confirmed);
}

#[test]
fn agents_quit_without_the_prompt() {
    let mut d = Driver::new();
    d.edit();
    d.commands.clear();
    d.ok("app.quit", json!({}));
    d.frames(1);
    assert!(d.sent(&egui::ViewportCommand::Close), "{:?} dialog {:?} status {:?}", d.commands, d.harness.state().dialog, d.harness.state().ui.status);
    assert!(d.harness.state().quit_confirmed);
    assert!(!d.harness.state().discard_on_exit, "without `discard` the changes stay in the journal");
    d.request_close();
    assert!(!d.sent(&egui::ViewportCommand::CancelClose));
    assert_ne!(d.harness.state().dialog, Some(Dialog::QuitConfirm));
}

#[test]
fn agents_quit_with_save_needs_a_file() {
    let mut d = Driver::new();
    d.edit();
    let v = d.call("app.quit", json!({"save": true}));
    assert_eq!(v["ok"], json!(false), "{v}");
    assert!(!d.harness.state().quit_confirmed);
}
