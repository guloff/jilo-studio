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
    snapshots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new(session: Session) -> Self {
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        // FILMCRAFT_UI_SNAPSHOT_DIR=<dir>: also render the window with wgpu and write ai-*.png there
        let snapshots = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        if snapshots.is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, snapshots };
        if let Ok(lang) = std::env::var("FILMCRAFT_UI_SNAPSHOT_LANG") {
            d.ok("engine.execute", json!({"command": "prefs.set", "params": {"key": "general.interfaceLanguage", "value": lang}}));
        }
        // FILMCRAFT_UI_SNAPSHOT_THEME=light|dark: the appearance the snapshots are taken in
        if let Ok(mode) = std::env::var("FILMCRAFT_UI_SNAPSHOT_THEME") {
            d.ok("engine.execute", json!({"command": "prefs.set", "params": {"key": "appearance.appearanceMode", "value": mode}}));
        }
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

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = self.snapshots.clone() else { return };
        self.frames(3);
        match self.harness.render() {
            Ok(img) => {
                std::fs::create_dir_all(&dir).unwrap();
                img.save(dir.join(format!("ai-{name}.png"))).unwrap();
            }
            Err(e) => eprintln!("snapshot {name} skipped: {e}"),
        }
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }
}

/// Russian transcripts for the media of the demo's first two A1 clips: sentences of four words, a
/// pause after each.
fn russian_transcript(d: &mut Driver) {
    // Russian transcripts for the media of the first two A1 clips: sentences of four words, a pause after each
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let tick = filmcraft_time::TICKS_PER_SECOND / 10; // 0.1 s
    let text = [
        ["Сегодня", "мы", "монтируем", "интервью.", "Сначала", "убираем", "лишние", "паузы."],
        ["Потом", "ищем", "лучшие", "моменты.", "И", "собираем", "короткий", "ролик."],
    ];
    for (k, sentences) in text.iter().enumerate() {
        let clip = &seq["audio"][0]["items"][k];
        let item = clip["item"].as_u64().unwrap();
        let start = clip["sourceIn"].as_i64().unwrap_or(0);
        let words: Vec<Value> = sentences
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let at = start + tick * (3 * i as i64 + 10 * (i as i64 / 4));
                json!({"text": w, "start": at, "end": at + tick * 2})
            })
            .collect();
        d.ok(
            "engine.execute",
            json!({"command": "transcript.set", "params": {"item": item, "transcript": {"language": "ru", "speakers": [], "words": words}}}),
        );
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
    d.snapshot("transcribe-empty");
    let prefs = d.ok("engine.execute", json!({"command": "prefs.get", "params": json!({"key": "ai.speechEngine"})}));
    assert_eq!(prefs, "gemini", "{prefs}");
    // Transcribe asks before anything is sent
    d.click("text.transcript.generate");
    assert!(d.ids("transcribe.consent").contains(&"transcribe.consent.send".to_string()));
    d.snapshot("transcribe-consent");
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
    d.snapshot("transcribe-done");
    assert!(!fake.sent().is_empty());
    let t = d.ok("engine.execute", json!({"command": "transcript.inspect", "params": json!({})}));
    assert_eq!(t["items"][0]["language"], "tg");
    assert!(t["items"][0]["source"].as_str().unwrap().starts_with("gemini:"));
    // the agreement is remembered: the next cloud transcription does not ask
    let confirmed = d.ok("engine.execute", json!({"command": "prefs.get", "params": json!({"key": "ai.confirmedProviders"})}));
    assert_eq!(confirmed, json!(["gemini"]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Remove filler words: the dialog lists the transcript's fillers with hesitations chosen, and
/// removes only what stays chosen.
#[test]
fn filler_words_are_chosen_in_a_dialog() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Transcript");
    // a Russian transcript for the media of the first A1 clip
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let item = seq["audio"][0]["items"][0]["item"].as_u64().unwrap();
    let start = seq["audio"][0]["items"][0]["sourceIn"].as_i64().unwrap_or(0);
    let tick = filmcraft_time::TICKS_PER_SECOND / 10; // 0.1 s
    let words: Vec<Value> = ["Сегодня", "э", "мы", "ну", "монтируем"]
        .iter()
        .enumerate()
        .map(|(i, w)| json!({"text": w, "start": start + tick * 5 * i as i64, "end": start + tick * (5 * i as i64 + 4)}))
        .collect();
    d.ok("engine.execute", json!({"command": "transcript.set", "params": {"item": item, "transcript": {"language": "ru", "speakers": [], "words": words}}}));
    d.frames(3);
    d.click("text.transcript.removeFillers");
    let ids = d.ids("fillers.");
    assert!(ids.contains(&"fillers.item.э".to_string()) && ids.contains(&"fillers.item.ну".to_string()), "{ids:?}");
    d.snapshot("fillers");
    // choose "ну" too, then remove
    d.click("fillers.item.ну");
    d.click("fillers.remove");
    assert!(d.ids("fillers.").is_empty(), "the dialog closes");
    let t = d.ok("engine.execute", json!({"command": "transcript.inspect", "params": {}}));
    let left: Vec<&str> = t["words"].as_array().unwrap().iter().filter_map(|w| w["text"].as_str()).collect();
    assert_eq!(left, ["Сегодня", "мы", "монтируем"]);
}

/// Captions ▸ Translate: the language, the consent for the caption text, the background job, the
/// new track.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn captions_are_translated_from_the_captions_tab() {
    let dir = std::env::temp_dir().join(format!("filmcraft-translate-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |r| {
            let b = r.json().unwrap_or_default();
            let user: Value = serde_json::from_str(b["contents"][0]["parts"][1]["text"].as_str().unwrap_or("{}")).unwrap_or_default();
            let items: Vec<Value> = user["items"].as_array().cloned().unwrap_or_default().iter().map(|i| json!({"id": i["id"], "text": "Привет"})).collect();
            let answer = json!({"items": items});
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "k"})).unwrap();
    s.execute("captions.newTrack", json!({"language": "en", "name": "English"})).unwrap();
    s.execute("captions.add", json!({"text": "Hello", "seconds": 1.0})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Captions");
    d.click("text.captions.translate");
    d.snapshot("translate-dialog");
    d.click("translate.to");
    d.click("translate.to.ru");
    d.click("translate.go");
    assert!(d.ids("aiTask.consent").contains(&"aiTask.consent.send".to_string()), "asks before the text goes out");
    d.snapshot("translate-consent");
    assert!(fake.sent().is_empty());
    d.click("aiTask.consent.send");
    let mut tracks = 0;
    for _ in 0..400 {
        d.frames(2);
        let q = d.ok("engine.execute", json!({"command": "captions.list", "params": {}}));
        tracks = q["tracks"].as_array().map_or(0, Vec::len);
        if tracks == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(tracks, 2, "the translated track was added");
    d.snapshot("translate-done");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Markers ▸ Find Highlights: the search dialog, the consent for the transcript, the background
/// search, the range markers and the results window that goes to a highlight and assembles them.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn highlights_are_found_listed_and_assembled() {
    let dir = std::env::temp_dir().join(format!("filmcraft-highlights-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |r| {
            // two highlights: the first half of the cues and the second
            let b = r.json().unwrap_or_default();
            let user = b["contents"][0]["parts"][1]["text"].as_str().unwrap_or_default().to_string();
            let cues: Vec<(f64, f64)> = user
                .lines()
                .filter_map(|l| {
                    let (a, b) = l.strip_prefix('[')?.split(']').next()?.split_once('-')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                })
                .collect();
            let mid = cues.len() / 2;
            let answer = json!({"fragments": [
                {"start": cues[mid].0, "end": cues[cues.len() - 1].1, "title": "Как монтировать быстрее", "score": 74, "reason": "Готовый совет со своим выводом."},
                {"start": cues[0].0, "end": cues[mid - 1].1, "title": "Главная мысль интервью", "score": 91, "reason": "Законченная мысль: завязка и вывод в одном фрагменте."}
            ]});
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "k"})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Transcript");
    russian_transcript(&mut d);
    d.frames(3);
    // the Transcript toolbar opens the search dialog
    d.click("text.transcript.highlights");
    assert!(d.ids("highlights.").contains(&"highlights.go".to_string()));
    d.click("highlights.mode.meaningful");
    d.snapshot("highlights-dialog");
    d.click("highlights.go");
    assert!(d.ids("aiTask.consent").contains(&"aiTask.consent.send".to_string()), "asks before the transcript goes out");
    d.snapshot("highlights-consent");
    assert!(fake.sent().is_empty());
    d.click("aiTask.consent.send");
    let mut listed = false;
    for _ in 0..400 {
        d.frames(2);
        if d.ids("highlights.item.").iter().any(|i| i == "highlights.item.1") {
            listed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(listed, "the results window lists the highlights");
    let sent = fake.sent();
    let body = sent[0].json().unwrap_or_default();
    assert!(body["contents"][0]["parts"][0]["text"].as_str().unwrap().contains("meaningful self-contained ideas"));
    d.snapshot("highlights-results");
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    // the demo's own markers, and the two highlights: range markers in the Meaningful colour
    let markers: Vec<Value> = seq["markers"].as_array().cloned().unwrap_or_default().into_iter().filter(|m| m["duration"] != 0).collect();
    assert_eq!(markers.len(), 2, "{markers:?}");
    assert!(markers.iter().all(|m| m["color"] == "Cerulean"));
    assert!(markers.iter().any(|m| m["name"] == "Главная мысль интервью"));
    // a title goes to its highlight
    d.click("highlights.item.1");
    let ph = d.ok("engine.execute", json!({"command": "state.inspect", "params": {}}));
    assert!(ph.to_string().contains("playhead"));
    // Subsequences: one per chosen highlight; untick the second
    d.click("highlights.item.1.chosen");
    d.click("highlights.subsequences");
    assert!(d.ids("highlights.item.").is_empty(), "the window closes");
    let project = d.ok("engine.execute", json!({"command": "project.inspect", "params": {}})).to_string();
    // listed best first: the second (unticked) is the lower-scored one
    assert!(project.contains("Main Edit · Главная мысль интервью") && !project.contains("· Как монтировать"), "{project}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Clip ▸ Auto Zoom: the dialog, by meaning with the consent for the transcript, and Scale
/// keyframes on the V1 clips where the quoted words are said.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn auto_zoom_by_meaning_keys_the_scale_on_the_quoted_words() {
    let dir = std::env::temp_dir().join(format!("filmcraft-autozoom-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |_| {
            let answer = json!({"moments": [{"start": 0.0, "end": 1.0, "quote": "лишние паузы"}, {"start": 6.0, "end": 7.0, "quote": "лучшие моменты"}]});
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "k"})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Transcript");
    russian_transcript(&mut d);
    d.frames(3);
    // the demo's clips are short: zooms 2 s apart (the stresses are 2.8 s apart)
    d.harness.state_mut().ui.auto_zoom.min_gap = 2.0;
    d.click("text.transcript.autoZoom");
    assert!(d.ids("autoZoom.").contains(&"autoZoom.go".to_string()));
    d.click("autoZoom.method.meaning");
    d.click("autoZoom.style.smooth");
    d.snapshot("autozoom-dialog");
    d.click("autoZoom.go");
    assert!(d.ids("aiTask.consent").contains(&"aiTask.consent.send".to_string()), "asks before the transcript goes out");
    assert!(fake.sent().is_empty());
    d.click("aiTask.consent.send");
    let scale_keys = |d: &mut Driver| {
        let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
        seq["video"][0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|it| {
                let motion = it["effects"].as_array().unwrap().iter().find(|e| e["effect"] == "motion").cloned().unwrap_or_default();
                motion["params"]["scale"]["keyframes"].as_u64().unwrap_or(0) as usize
            })
            .collect::<Vec<usize>>()
    };
    let mut keyed = Vec::new();
    for _ in 0..400 {
        d.frames(2);
        keyed = scale_keys(&mut d);
        if keyed.iter().any(|n| *n > 0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // a zoom in each of the first two clips (one quote each), smooth: 5 keys apiece; the rest untouched
    assert_eq!(&keyed[..3], &[5, 5, 0], "{keyed:?}");
    d.snapshot("autozoom-done");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Sequence ▸ YouTube: the form, the consent, the texts window with copy buttons, an edit kept with
/// the sequence, and the chapters as chapter markers.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn youtube_texts_are_written_shown_and_edited() {
    let dir = std::env::temp_dir().join(format!("filmcraft-youtube-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |_| {
            let answer = json!({
                "titles": ["Как смонтировать интервью за вечер", "Монтаж интервью: паузы и лучшие моменты", "Быстрый монтаж разговорного видео"],
                "description": "Показываю, как быстро смонтировать интервью: убрать лишние паузы и найти лучшие моменты.\n\nВсё делаем по расшифровке, без ручной перемотки.",
                "chapters": [{"start": 0, "title": "Вступление"}, {"start": 4, "title": "Лучшие моменты"}],
                "tags": ["монтаж интервью", "монтаж видео", "расшифровка", "паузы"],
                "hashtags": ["#монтаж", "#интервью", "#видео"]
            });
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "k"})).unwrap();
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Transcript");
    russian_transcript(&mut d);
    d.frames(3);
    d.click("text.transcript.youtube");
    assert!(d.ids("youtube.").contains(&"youtube.generate".to_string()), "no texts yet: the form");
    d.snapshot("youtube-form");
    d.click("youtube.generate");
    assert!(d.ids("aiTask.consent").contains(&"aiTask.consent.send".to_string()));
    assert!(fake.sent().is_empty());
    d.click("aiTask.consent.send");
    let mut shown = false;
    for _ in 0..400 {
        d.frames(2);
        if d.ids("youtube.").iter().any(|i| i == "youtube.copyAll") {
            shown = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(shown, "the texts window opens when they are written");
    d.snapshot("youtube-texts");
    let t = d.ok("engine.execute", json!({"command": "youtube.texts", "params": {}}));
    assert_eq!(t["titles"][1], "Монтаж интервью: паузы и лучшие моменты");
    // the demo is 26 s: YouTube would not show its chapters, but they are chapter markers
    assert_eq!(t["problems"], json!(["videoTooShort"]));
    let seq = d.ok("engine.execute", json!({"command": "sequence.inspect", "params": {}}));
    let chapters: Vec<&str> = seq["markers"].as_array().unwrap().iter().filter(|m| m["kind"] == "Chapter").filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(chapters, ["Вступление"], "the second asked chapter is under 10 s from the first, so it moves on to no cue");
    // choose the second title and copy everything
    d.click("youtube.title.1");
    let before = d.harness.state().ui.status.clone();
    d.click("youtube.copyAll");
    let after = d.harness.state().ui.status.clone();
    assert!(after != before && !after.is_empty(), "the status bar says what was copied: {after}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// File ▸ Batch Process: the dialog for the Project panel's selection, the cloud note, the batch
/// as a job, and the summary with the item that failed a step.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn a_batch_runs_from_the_dialog_and_ends_with_a_summary() {
    let dir = std::env::temp_dir().join(format!("filmcraft-batch-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.contains(":generateContent"),
        |_| {
            let answer = json!({"titles": ["Заголовок"], "description": "Описание.", "chapters": [], "tags": ["тег"], "hashtags": ["#тег"]});
            Ok(Response::json(200, &json!({"candidates": [{"content": {"parts": [{"text": answer.to_string()}]}, "finishReason": "STOP"}]})))
        },
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "gemini", "key": "k"})).unwrap();
    s.path = Some(dir.join("Demo.fcproj").to_string_lossy().into_owned());
    let mut d = Driver::new(s);
    d.ok("ui.set", json!({"workspace": "Captions and Graphics"}));
    d.frames(2);
    d.click("text.tab.Transcript");
    russian_transcript(&mut d);
    // Main Edit (transcribed) and Rough Cut (empty: its YouTube step fails)
    let project = d.ok("engine.execute", json!({"command": "project.inspect", "params": {}})).to_string();
    let v: Value = serde_json::from_str(&project).unwrap();
    let seqs: Vec<u64> = v["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["children"].as_array().cloned().unwrap_or_default())
        .filter(|i| i["type"] == "Sequence")
        .filter_map(|i| i["item"].as_u64())
        .collect();
    assert_eq!(seqs.len(), 2);
    d.ok("engine.execute", json!({"command": "project.select", "params": {"items": seqs}}));
    d.ok("ui.menu.invoke", json!({"id": "batch.start"}));
    d.frames(3);
    assert!(d.ids("batch.").contains(&"batch.go".to_string()));
    // transcribe and export are on by default; choose YouTube and the WAV preset
    d.click("batch.step.transcribe");
    d.click("batch.step.youtube");
    d.harness.state_mut().ui.batch.preset = "Waveform Audio 48 kHz 16-bit".into();
    d.snapshot("batch-dialog");
    d.click("batch.go");
    let mut done = false;
    for _ in 0..600 {
        d.frames(2);
        if d.ids("batch.summary").iter().any(|i| i == "batch.summary.close") {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(done, "the summary opens when the batch ends");
    d.snapshot("batch-summary");
    let st = d.ok("engine.execute", json!({"command": "batch.status", "params": {}}));
    let status: Vec<&str> = st["items"].as_array().unwrap().iter().filter_map(|i| i["status"].as_str()).collect();
    assert_eq!(status, ["done", "failed"], "{st}");
    d.click("batch.summary.close");
    assert!(d.ids("batch.summary").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Clip ▸ Reframe to Face… on a clip in a vertical sequence: the dialog, the frame at the playhead
/// with its faces boxed, a click picking the other face, the analysis in the background and the
/// framing it leaves in Motion.
#[test]
fn a_clip_is_reframed_on_a_face_picked_on_the_frame() {
    use filmcraft_engine::reframe::FaceBox;
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.face_detector = Some(Arc::new(|_: &[u8], _, _| {
        Ok(vec![FaceBox { x: 0.22, y: 0.25, w: 0.12, h: 0.22, confidence: 0.9 }, FaceBox { x: 0.62, y: 0.3, w: 0.09, h: 0.17, confidence: 0.8 }])
    }));
    let item = s.project.items.values().find(|i| i.has_video() && i.as_media().is_some()).map(|i| i.id.0).unwrap();
    s.execute("file.newSequence", json!({"name": "Vertical", "width": 1080, "height": 1920, "video": 1, "audio": 1})).unwrap();
    let r =
        s.execute("timeline.place", json!({"item": item, "track": "V1", "seconds": 0.0, "duration": filmcraft_time::Tick::from_seconds_f64(6.0).0})).unwrap();
    let clip = r["clips"][0].as_u64().unwrap();
    s.state.selection = vec![filmcraft_project::ClipId(clip)];
    s.set_playhead(filmcraft_time::Tick::from_seconds_f64(1.0));
    let mut d = Driver::new(s);
    d.frames(2);
    d.ok("ui.menu.invoke", json!({"id": "clip.reframeToFace"}));
    d.frames(3);
    assert!(d.ids("reframe.").contains(&"reframe.go".to_string()));
    d.snapshot("reframe-dialog");
    d.click("reframe.target.face");
    d.frames(3);
    let picked = |d: &mut Driver| d.harness.state().ui.reframe.picker.as_ref().and_then(|p| p.picked);
    assert_eq!(picked(&mut d), Some(0), "the biggest face to start with");
    d.click("reframe.frame");
    assert_eq!(picked(&mut d), Some(1), "a click picks the face nearest to it");
    d.frames(2);
    d.snapshot("reframe-face");
    d.click("reframe.go");
    for _ in 0..600 {
        d.frames(2);
        if d.harness.state().ui.reframe.job.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(d.harness.state().ui.reframe.job.is_none(), "the framing is applied when the faces are found");
    let app = d.harness.state();
    let (_, it) = app.session.active_sequence().unwrap().find_item(filmcraft_project::ClipId(clip)).unwrap();
    let m = it.effect("motion").unwrap();
    let pos = m.vec2_at("position", it.effect_time_at(filmcraft_time::Tick::from_seconds_f64(3.0)));
    // the second face (centre 0.665 of 1920) on the middle of the 1080 frame, the picture 1920/1080 up
    let want = 540.0 - 1920.0 / 1080.0 * (0.665 * 1920.0 - 960.0);
    assert!((pos.x - want).abs() < 0.2 && m.params["position"].keyframes.is_empty(), "{pos:?} vs {want}");
    assert!(app.ui.status.contains('1'), "{}", app.ui.status);
    // the Program monitor renders the new framing in the background
    for _ in 0..40 {
        d.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    d.snapshot("reframe-done");
}

/// Help ▸ Check for Updates…: the check in the background, the window with a newer release and its
/// notes, and the up-to-date answer.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn updates_are_checked_on_request_and_shown_in_a_window() {
    let fake = Arc::new(FakeTransport::new());
    let page = format!("{}/tag/v9.1.0", filmcraft_engine::updates::RELEASES_PAGE);
    let tag = Arc::new(std::sync::Mutex::new("v9.1.0".to_string()));
    let t = tag.clone();
    fake.route(
        |r| r.url == filmcraft_engine::updates::RELEASES_API,
        move |_| {
            let body = json!({
                "tag_name": t.lock().unwrap().clone(),
                "html_url": page.clone(),
                "body": include_str!("../../../docs/releases/0.6.0.md"),
                "published_at": "2026-10-20T10:00:00Z",
            });
            Ok(Response::json(200, &body))
        },
    );
    let mut s = Session::default();
    s.ai_transport = Some(fake.clone());
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    d.frames(2);
    assert!(fake.sent().is_empty(), "nothing is checked unless asked");
    let wait = |d: &mut Driver, id: &str| {
        for _ in 0..400 {
            d.frames(2);
            if d.ids("updates.").iter().any(|i| i == id) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no {id}");
    };
    d.ok("ui.menu.invoke", json!({"id": "app.checkUpdates"}));
    // this release has no disk image: its page is offered
    wait(&mut d, "updates.page");
    assert_eq!(fake.sent().len(), 1);
    d.snapshot("updates-available");
    d.click("updates.later");
    assert!(d.ids("updates.").is_empty());
    *tag.lock().unwrap() = format!("v{}", env!("CARGO_PKG_VERSION"));
    d.ok("ui.menu.invoke", json!({"id": "app.checkUpdates"}));
    wait(&mut d, "updates.ok");
    d.snapshot("updates-latest");
    d.click("updates.ok");
    assert!(d.ids("updates.").is_empty());
}

/// Settings ▸ General ▸ Check for updates once a day: a newer release is offered by itself, once.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn the_daily_check_offers_a_newer_release_once() {
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url == filmcraft_engine::updates::RELEASES_API,
        |_| {
            Ok(Response::json(
                200,
                &json!({"tag_name": "v9.2.0", "html_url": format!("{}/tag/v9.2.0", filmcraft_engine::updates::RELEASES_PAGE), "body": "Notes"}),
            ))
        },
    );
    assert!(filmcraft_engine::settings::GeneralPrefs::default().check_for_updates, "on by default");
    let mut s = Session::default();
    s.ai_transport = Some(fake.clone());
    s.prefs.general.check_for_updates = true;
    let mut d = Driver::new(s);
    d.harness.state_mut().ui.updates.host = true;
    let mut shown = false;
    for _ in 0..400 {
        d.frames(2);
        // a release without a disk image: its page is offered
        if d.ids("updates.").iter().any(|i| i == "updates.page") {
            shown = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(shown, "the daily check offers the newer release");
    assert_eq!(fake.sent().len(), 1);
    assert_eq!(d.harness.state().session.prefs.general.update_offered, "9.2.0");
    assert!(d.harness.state().session.prefs.general.last_update_check > 0);
    d.click("updates.later");
    // a day later the same release is not offered again
    d.harness.state_mut().session.prefs.general.last_update_check = 1;
    d.harness.state_mut().ui.updates.next_look = None;
    for _ in 0..60 {
        d.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(fake.sent().len(), 2, "checked again after a day");
    assert!(d.ids("updates.").is_empty(), "but the release it offered is not offered twice");
}

/// File ▸ New ▸ Generate Image…: the dialog, the consent, the picture imported and placed.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn an_image_is_generated_from_a_description() {
    let dir = std::env::temp_dir().join(format!("filmcraft-imagegen-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(96, 54, |x, y| image::Rgba([(x * 2) as u8, 90, (y * 4) as u8, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let b64 = filmcraft_ai::base64::encode(png.get_ref());
    let fake = Arc::new(FakeTransport::new());
    fake.route(|r| r.url.ends_with("/images/generations"), move |_| Ok(Response::json(200, &json!({"data": [{"b64_json": b64}]}))));
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("ai.setKey", json!({"provider": "openai", "key": "k"})).unwrap();
    s.path = Some(dir.join("Demo.fcproj").to_string_lossy().into_owned());
    let items_before = s.project.items.len();
    let mut d = Driver::new(s);
    d.ok("ui.menu.invoke", json!({"id": "ai.generateImage"}));
    d.frames(3);
    d.harness.state_mut().ui.imagegen.prompt = "A lighthouse on a cliff at dawn, soft mist, wide shot".into();
    d.frames(2);
    d.snapshot("imagegen-dialog");
    d.click("imagegen.go");
    assert!(d.ids("aiTask.consent").contains(&"aiTask.consent.send".to_string()), "asks before the description goes out");
    d.snapshot("imagegen-consent");
    d.click("aiTask.consent.send");
    let mut added = false;
    for _ in 0..400 {
        d.frames(2);
        if d.harness.state().session.project.items.len() > items_before {
            added = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(added, "the picture is imported");
    assert_eq!(fake.sent().len(), 1);
    for _ in 0..20 {
        d.frames(2);
    }
    let app = d.harness.state();
    let item = app.session.project.items.values().filter(|i| i.name.starts_with("Generated Image")).map(|i| i.id).next().expect("a generated still");
    let placed = app.session.active_sequence().unwrap().video_tracks.iter().flat_map(|t| &t.items).any(|c| c.item == item);
    assert!(placed, "the picture is on the timeline at the playhead");
    d.snapshot("imagegen-done");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Text to Speech with an ElevenLabs voice: the account's voices are loaded on request, the panel
/// asks once before the text goes out, and the narration is spoken by ElevenLabs.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn an_elevenlabs_voice_is_chosen_allowed_and_speaks_the_narration() {
    let dir = std::env::temp_dir().join(format!("filmcraft-eleven-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = Arc::new(FakeTransport::new());
    fake.route(
        |r| r.url.ends_with("/v1/voices"),
        |_| {
            Ok(Response::json(
                200,
                &json!({"voices": [{"voice_id": "v1", "name": "Rachel", "labels": {"gender": "female"}}, {"voice_id": "v2", "name": "Adam"}]}),
            ))
        },
    );
    // half a second of a tone for every piece of speech
    fake.route(
        |r| r.url.contains("/text-to-speech/v1"),
        |_| Ok(Response::new(200, (0..22_050).flat_map(|i| (((i as f32 * 0.05).sin() * 8000.0) as i16).to_le_bytes()).collect::<Vec<u8>>())),
    );
    let mut s = Session::default();
    s.ai_keys_file = Some(dir.join("ai.json"));
    s.ai_transport = Some(fake.clone());
    s.ai_sleep = Some(Arc::new(|_, _| {}));
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.projectSettings.scratchDisks", json!({"captured": dir.to_string_lossy()})).unwrap();
    s.execute("ai.setKey", json!({"provider": "elevenlabs", "key": "xi-test"})).unwrap();
    let mut d = Driver::new(s);
    d.ok("engine.execute", json!({"command": "window.panel.TextToSpeech", "params": {}}));
    d.frames(3);
    // the panel offers to load the account's voices
    d.snapshot("tts-eleven-load");
    d.click("tts.cloud.load");
    let mut loaded = false;
    for _ in 0..400 {
        d.frames(1);
        let v = d.ok("engine.execute", json!({"command": "tts.voices", "params": {}}));
        if v["cloud"]["loaded"] == json!(true) {
            loaded = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(loaded, "the ElevenLabs voices were loaded");
    d.frames(3);
    if !d.ids("tts.voice.").iter().any(|i| i == "tts.voice.elevenlabs:v1") {
        d.click("tts.voice");
    }
    d.snapshot("tts-eleven-voices");
    d.click("tts.voice.elevenlabs:v1");
    let draft = d.ok("ui.inspect", json!({}))["ui"]["tts"].clone();
    assert_eq!(draft["voice"], "elevenlabs:v1", "{draft}");
    // nothing is spoken before the user allows ElevenLabs
    assert!(d.ids("tts.cloud").contains(&"tts.cloud.allow".to_string()));
    d.snapshot("tts-eleven-consent");
    d.click("tts.hearVoice");
    assert!(!fake.sent().iter().any(|r| r.url.contains("text-to-speech")), "nothing goes out before the user agrees");
    d.click("tts.cloud.allow");
    let confirmed = d.ok("engine.execute", json!({"command": "prefs.get", "params": {"key": "ai.confirmedProviders"}}));
    assert_eq!(confirmed, json!(["elevenlabs"]));
    assert!(d.ids("tts.cloud").is_empty());
    // type a script and add it to the timeline
    d.click("tts.text");
    d.ok("ui.type", json!({"text": "Hello from the cloud."}));
    d.frames(3);
    d.click("tts.save");
    let mut added = false;
    for _ in 0..400 {
        d.frames(1);
        if d.ok("ui.inspect", json!({}))["ui"]["tts"]["pending"].is_null() && fake.sent().iter().any(|r| r.url.contains("text-to-speech")) {
            added = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(added, "the narration was spoken");
    d.frames(3);
    d.snapshot("tts-eleven-added");
    let status = d.ok("ui.inspect", json!({}))["ui"]["status"].as_str().unwrap_or_default().to_string();
    assert!(!status.contains("CONFIRM_CLOUD") && !status.contains("rror"), "{status}");
    let speech: Vec<_> = fake.sent().into_iter().filter(|r| r.url.contains("text-to-speech")).collect();
    assert_eq!(speech.last().unwrap().json().unwrap()["text"], "Hello from the cloud.");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Download and Install keeps the window open: the download's progress, then what comes next — a
/// test binary is no installed app, so the disk image to open by hand; a checked update offers
/// Restart and Install or Install When I Quit; and the launch after an update says it is installed.
#[test]
#[allow(clippy::field_reassign_with_default)] // Session has private fields
fn an_update_downloads_with_progress_and_says_what_comes_next() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let dir = std::env::temp_dir().join(format!("filmcraft-update-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let image: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let digest: String = {
        use sha2::Digest;
        sha2::Sha256::digest(&image).iter().map(|b| format!("{b:02x}")).collect()
    };
    let name = "jilo-studio-9.3.0-macos-aarch64.dmg";
    let url = format!("https://github.com/{}/releases/download/v9.3.0/{name}", filmcraft_engine::updates::REPO);
    let release = json!({"tag_name": "v9.3.0", "html_url": format!("{}/tag/v9.3.0", filmcraft_engine::updates::RELEASES_PAGE),
        "body": include_str!("../../../docs/releases/0.6.3.md"),
        "assets": [{"name": name, "browser_download_url": url, "size": image.len(), "digest": format!("sha256:{digest}")}]});
    let fake = Arc::new(FakeTransport::new());
    fake.route(|r| r.url == filmcraft_engine::updates::RELEASES_API, move |_| Ok(Response::json(200, &release)));
    // the download waits until the test lets it finish
    let gate = Arc::new(AtomicBool::new(false));
    let g = gate.clone();
    let bytes = image.clone();
    fake.route(
        move |r| r.url == url,
        move |_| {
            while !g.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(Response::new(200, bytes.clone()))
        },
    );
    let mut s = Session::default();
    s.ai_transport = Some(fake.clone());
    s.updates_dir = Some(dir.clone());
    s.execute("file.openDemoProject", json!({})).unwrap();
    let mut d = Driver::new(s);
    let wait = |d: &mut Driver, id: &str| {
        for _ in 0..600 {
            d.frames(2);
            if d.ids("updates.").iter().any(|i| i == id) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no {id} in {:?}", d.ids("updates."));
    };
    d.ok("ui.menu.invoke", json!({"id": "app.checkUpdates"}));
    wait(&mut d, "updates.download");
    d.snapshot("update-available");
    d.click("updates.download");
    wait(&mut d, "updates.progress");
    let ids = d.ids("updates.");
    assert!(ids.contains(&"updates.hide".to_string()) && ids.contains(&"updates.stop".to_string()), "{ids:?}");
    // halfway, as a slow line would show it
    if let Some(j) = d.harness.state().session.jobs.last() {
        j.progress.done.store(image.len() as u64 / 2, Ordering::Relaxed);
    }
    d.frames(3);
    d.snapshot("update-downloading");
    // hidden, it goes on and comes back when done
    d.click("updates.hide");
    assert!(d.ids("updates.").is_empty());
    gate.store(true, Ordering::Relaxed);
    wait(&mut d, "updates.openImage");
    d.snapshot("update-manual");
    let st = d.ok("engine.execute", json!({"command": "app.updateStatus", "params": {}}));
    assert_eq!(st["download"]["phase"], "manual", "{st}");
    assert_eq!(std::fs::read(dir.join(name)).unwrap(), image);
    d.click("updates.close");
    // a checked update (as the app itself gets): Restart and Install or Install When I Quit
    *d.harness.state().session.update_download.lock().unwrap() = Some(filmcraft_engine::updater::UpdateDownload {
        version: "9.3.0".into(),
        phase: "ready".into(),
        staged: Some(dir.join("Staged/Jilo Studio.app").to_string_lossy().into_owned()),
        target: Some("/Applications/Jilo Studio.app".into()),
        ..Default::default()
    });
    d.ok("ui.menu.invoke", json!({"id": "app.checkUpdates"}));
    wait(&mut d, "updates.restart");
    d.snapshot("update-ready");
    let sent = fake.sent().len();
    d.click("updates.onQuit");
    wait(&mut d, "updates.ok");
    assert_eq!(fake.sent().len(), sent, "showing a ready update checks nothing");
    assert_eq!(d.harness.state().session.prefs.general.update_installing, "9.3.0");
    d.snapshot("update-on-quit");
    d.click("updates.ok");
    // nothing is installed when this test ends
    d.ok("engine.execute", json!({"command": "app.installUpdate", "params": {"cancel": true}}));
    // the launch after an update
    {
        let app = d.harness.state_mut();
        app.session.prefs.general.update_installing = env!("CARGO_PKG_VERSION").into();
        app.session.prefs.general.check_for_updates = false;
        app.ui.updates.host = true;
        app.ui.updates.launch_seen = false;
    }
    wait(&mut d, "updates.ok");
    d.snapshot("update-installed");
    assert_eq!(d.harness.state().session.prefs.general.update_installing, "");
    d.harness.state_mut().ui.updates.host = false;
    d.click("updates.ok");
    let _ = std::fs::remove_dir_all(&dir);
}
