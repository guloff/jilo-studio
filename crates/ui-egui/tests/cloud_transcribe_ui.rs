//! Headless UI test of cloud transcription in the Text panel: the engine choice, the note on what
//! is sent, the one-time consent, the background recognition with its progress, and the transcript
//! it adds. The cloud is a scripted transport: no network.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_ai::Response;
use filmcraft_ai::transport::FakeTransport;
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

    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().unwrap().iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }
}

fn mmss(t: f64) -> String {
    format!("{:02}:{:06.3}", (t / 60.0) as u64, t % 60.0)
}

/// Gemini answering every window with a word every half second over the whole window.
fn fake_gemini() -> Arc<FakeTransport> {
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |r| {
            let body = r.json().unwrap_or_default();
            let prompt = body["contents"][0]["parts"][1]["text"].as_str().unwrap_or_default().to_string();
            let len: f64 = prompt.split("this clip is ").nth(1).and_then(|x| x.split(' ').next()).and_then(|x| x.parse().ok()).unwrap_or(10.0);
            let mut segs = Vec::new();
            let mut t = 0.0;
            while t + 2.0 <= len {
                segs.push(json!({"start": mmss(t), "end": mmss(t + 2.0), "text": "салом дӯстон", "words": [
                    {"text": "салом", "start": mmss(t), "end": mmss(t + 0.9)}, {"text": "дӯстон", "start": mmss(t + 1.0), "end": mmss(t + 1.95)}]}));
                t += 2.0;
            }
            let answer = json!({"language": "Tajik", "segments": segs});
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    fake
}

#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn cloud_transcription_asks_once_runs_in_the_background_and_adds_the_transcript() {
    let dir = std::env::temp_dir().join(format!("filmcraft-cloud-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = fake_gemini();
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "test-key"})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Transcript");
    // choose Gemini and Tajik in the panel
    d.click("text.transcript.engine");
    d.click("text.transcript.engine.gemini");
    d.click("text.transcript.language");
    d.click("text.transcript.language.tg");
    let prefs = d.ok("engine.execute", json!({"command": "prefs.get", "params": json!({"key": "ai.speechEngine"})}));
    assert_eq!(prefs, "gemini", "{prefs}");
    // Transcribe asks before anything is sent
    d.click("text.transcript.generate");
    assert!(d.ids("transcribe.consent").contains(&"transcribe.consent.send".to_string()));
    assert!(fake.sent().is_empty(), "nothing goes out before the user agrees");
    // Cancel leaves everything as it was
    d.click("transcribe.consent.cancel");
    assert!(d.ids("transcribe.consent").is_empty());
    assert!(fake.sent().is_empty());
    // Send: the recognition runs in the background and its transcript is added
    d.click("text.transcript.generate");
    d.click("transcribe.consent.send");
    let mut added = false;
    for _ in 0..400 {
        d.frames(2);
        let t = d.ok("engine.execute", json!({"command": "transcript.inspect", "params": json!({})}));
        if t["words"].as_array().is_some_and(|w| !w.is_empty()) {
            added = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(added, "the transcript was added");
    assert!(!fake.sent().is_empty());
    let t = d.ok("engine.execute", json!({"command": "transcript.inspect", "params": json!({})}));
    assert_eq!(t["items"][0]["language"], "tg");
    assert!(t["items"][0]["source"].as_str().unwrap().starts_with("gemini:"));
    // the agreement is remembered: the next cloud transcription does not ask
    let confirmed = d.ok("engine.execute", json!({"command": "prefs.get", "params": json!({"key": "ai.confirmedProviders"})}));
    assert_eq!(confirmed, json!(["gemini"]));
    let _ = std::fs::remove_dir_all(&dir);
}
