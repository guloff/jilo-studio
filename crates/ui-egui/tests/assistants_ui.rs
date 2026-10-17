//! Settings ▸ AI ▸ Allow AI assistants: the assistants' control channel opens when the setting is
//! turned on (no restart), only once, answers requests while it is on and refuses them once it is
//! off; the AI page shows its state and the Connect buttons.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::{AssistantChannel, FilmcraftApp};
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    /// Requests "from an assistant", once the app opened the channel.
    assistant: Rc<std::cell::RefCell<Option<Sender<ControlRequest>>>>,
    opened: Rc<Cell<usize>>,
}

impl Driver {
    fn new() -> Self {
        Self::build(None)
    }

    fn with_control(rx: Receiver<ControlRequest>) -> Self {
        Self::build(Some(rx))
    }

    fn build(control: Option<Receiver<ControlRequest>>) -> Self {
        let assistant: Rc<std::cell::RefCell<Option<Sender<ControlRequest>>>> = Rc::default();
        let opened = Rc::new(Cell::new(0));
        let mut app = FilmcraftApp::new(Session::default());
        if let Some(rx) = control {
            app = app.with_control(rx);
        }
        let (a, o) = (assistant.clone(), opened.clone());
        app.hooks.assistant_channel = Some(Box::new(move || {
            o.set(o.get() + 1);
            let (tx, rx) = channel();
            *a.borrow_mut() = Some(tx);
            Ok(Some(rx))
        }));
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        // `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>`: render offscreen with wgpu and write the AI page there
        if std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, assistant, opened };
        d.frames(3);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            // synthetic input (ui.scroll, ui.click) goes in through the app's input hook
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn set_allowed(&mut self, on: bool) {
        self.app().session.execute("prefs.set", json!({"key": "ai.allowAssistants", "value": on})).unwrap();
        self.frames(2);
    }

    /// A request on the assistants' channel, and its reply.
    fn ask(&mut self, method: &str, params: Value) -> Value {
        let (req, reply): (ControlRequest, Receiver<Value>) = ControlRequest::new(method, params);
        self.assistant.borrow().as_ref().expect("the channel is open").send(req).unwrap();
        for _ in 0..200 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method}");
    }
}

#[test]
fn the_channel_opens_when_allowed_and_refuses_when_not() {
    let mut d = Driver::new();
    // off by default: nothing is opened
    assert_eq!(d.opened.get(), 0);
    assert_eq!(d.app().assistant_channel, AssistantChannel::Closed);
    // turned on: opened at once, without a restart
    d.set_allowed(true);
    assert_eq!(d.opened.get(), 1);
    assert_eq!(d.app().assistant_channel, AssistantChannel::Open);
    let r = d.ask("engine.execute", json!({"command": "file.openDemoProject", "params": {}}));
    assert_eq!(r["ok"], true, "{r}");
    assert!(d.app().session.active_sequence().is_some(), "the request ran");
    // turned off: the channel refuses, and says how to allow it
    d.set_allowed(false);
    let r = d.ask("engine.execute", json!({"command": "edit.undo", "params": {}}));
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("Allow AI assistants"), "{r}");
    assert!(d.app().session.active_sequence().is_some(), "nothing ran");
    // on again: the same channel answers; it is not opened twice
    d.set_allowed(true);
    assert_eq!(d.ask("ui.inspect", json!({}))["ok"], true);
    assert_eq!(d.opened.get(), 1);
}

#[test]
fn a_channel_that_cannot_open_says_why() {
    let mut app = FilmcraftApp::new(Session::default());
    app.hooks.assistant_channel = Some(Box::new(|| Err("control server failed to bind 127.0.0.1:47821: Address already in use".into())));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).build_eframe(move |_cc| app);
    h.step();
    h.state_mut().session.execute("prefs.set", json!({"key": "ai.allowAssistants", "value": true})).unwrap();
    h.step();
    h.step();
    assert!(matches!(&h.state().assistant_channel, AssistantChannel::Failed(e) if e.contains("in use")));
}

#[test]
fn the_ai_page_shows_the_setting_and_the_channel() {
    let (tx, rx) = channel();
    let mut d = Driver::with_control(rx);
    d.set_allowed(true);
    let call = |d: &mut Driver, method: &str, params: Value| {
        let (req, reply) = ControlRequest::new(method, params);
        tx.send(req).unwrap();
        for _ in 0..100 {
            d.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method}");
    };
    assert_eq!(call(&mut d, "ui.menu.invoke", json!({"id": "app.settings.ai"}))["ok"], true);
    d.frames(4);
    let ids: Vec<String> = call(&mut d, "ui.elements", json!({"prefix": "settings."}))["result"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e["id"].as_str().map(str::to_string))
        .collect();
    assert!(ids.iter().any(|i| i == "settings.ai.allowAssistants"), "{ids:?}");
    if let Some(dir) = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from) {
        // the section is at the end of the page
        for _ in 0..6 {
            call(&mut d, "ui.scroll", json!({"x": 900.0, "y": 500.0, "dx": 0.0, "dy": -2000.0}));
        }
        d.frames(4);
        match d.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                img.save(dir.join("assistants-ai-page.png")).unwrap();
            }
            Err(e) => eprintln!("snapshot skipped: {e}"),
        }
    }
    // with the MCP server next to the app (an installed copy), the Connect buttons; a test build
    // has none, and the page says where it comes from instead
    let st = d.app().session.execute("ai.assistants.status", json!({})).unwrap();
    let buttons = ids.iter().any(|i| i == "settings.aiAssistants.claudeDesktop") && ids.iter().any(|i| i == "settings.aiAssistants.claudeCode");
    assert_eq!(buttons, st["cli"].is_string(), "{ids:?} {st}");
}
