//! Headless UI tests of the command palette (⌘J) — it opens, finds commands, panels and recipes by a
//! few letters, runs the chosen one with Return and closes with Escape — and of the quick actions
//! over the Program monitor for the selection, and the AI assistant reached from the palette (over a
//! scripted cloud engine).

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
    // Speed: the presets in a menu
    d.click("quick.clip.speedDuration");
    assert!(d.has("quick.speed.2") && d.has("quick.speed.freeze") && d.has("quick.speed.dialog"), "the speed menu");
    d.click("quick.speed.2");
    assert!((d.app().session.active_sequence().unwrap().find_item(clip).unwrap().1.speed - 2.0).abs() < 1e-9, "{}", d.app().ui.status);
    assert!(!d.has("quick.speed.2"), "the menu closes");
    d.exec("edit.undo", json!({}));
    d.frames(2);
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

#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn the_assistant_plans_from_the_palette_and_runs_the_plan_as_one_undo_step() {
    use filmcraft_ai::Response;
    use filmcraft_ai::transport::FakeTransport;
    let dir = std::env::temp_dir().join(format!("jilo-assistant-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = std::sync::Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |_| {
            let plan = json!({"reply": "Yellow karaoke captions.", "steps": [
                {"command": "captions.applyStylePreset", "params": "{\"preset\": \"karaoke-yellow\", \"track\": \"all\"}", "why": "the style"}]});
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": plan.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(std::sync::Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "test-key"})).unwrap();
    s.execute("prefs.set", json!({"key": "ai.textEngine", "value": "gemini"})).unwrap();
    s.execute("captions.add", json!({"text": "hello", "seconds": 1})).unwrap();
    let mut d = Driver::new(s);
    key(&mut d, "Cmd+J");
    d.ok("ui.type", json!({"text": "qqq make the captions yellow karaoke"}));
    d.frames(3);
    assert!(d.has("palette.ask"), "nothing matches: ask the assistant");
    d.click("palette.ask");
    d.frames(3);
    assert!(d.has("aiTask.consent.send"), "asks once before sending");
    d.click("aiTask.consent.send");
    for _ in 0..400 {
        d.frames(1);
        if d.has("assistant.plan.1.run") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(d.has("assistant.plan.1.step.0"), "the plan is shown before anything changes");
    assert_ne!(d.app().session.active_sequence().unwrap().caption_tracks[0].style, filmcraft_engine::caption_presets::find("karaoke-yellow").unwrap().style);
    let undo = d.app().session.history.undo.len();
    d.click("assistant.plan.1.run");
    for _ in 0..200 {
        d.frames(1);
        if d.app().session.recipe.as_ref().is_some_and(|r| r.finished) {
            break;
        }
    }
    let app = d.app();
    assert_eq!(app.session.active_sequence().unwrap().caption_tracks[0].style, filmcraft_engine::caption_presets::find("karaoke-yellow").unwrap().style);
    assert_eq!(app.session.history.undo.len(), undo + 1);
    let _ = std::fs::remove_dir_all(&dir);
}
