//! Headless UI test of captions placed by hand: the selected caption's box in the Program monitor
//! drags the caption block anywhere (one undo step), the Caption Style panel puts it back, and
//! "Only the selected phrases" gives one phrase a style of its own. `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>`
//! writes `caption-drag.png` (`FILMCRAFT_UI_LANG=ru`: Russian).
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
    /// Whether `id` is on screen inside the panel (not scrolled away).
    fn visible(&mut self, id: &str) -> bool {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == json!(id) && e["rect"][1].as_f64().is_some_and(|y| (110.0..480.0).contains(&y)))
    }
    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }
}

#[test]
fn a_caption_is_dragged_in_the_monitor_and_one_phrase_styled_alone() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let at = s.playhead().seconds();
    s.execute("captions.add", json!({"seconds": at - 0.5, "durationSeconds": 3.0, "text": "Drag me"})).unwrap();
    s.execute("captions.add", json!({"seconds": at + 3.0, "durationSeconds": 3.0, "text": "And me"})).unwrap();
    let q = s.active_sequence().unwrap();
    let (first, second) = (q.caption_tracks[0].captions[0].id, q.caption_tracks[0].captions[1].id);
    s.state.caption_selection = vec![first];
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "graphics.captionStyles"}));
    d.frames(4);
    let id = format!("program.caption.{}.box", first.0);
    assert!(d.has(&id), "the selected caption has a box in the monitor");
    let undo = d.app().session.history.undo.len();
    // up and to the left
    d.ok("ui.drag", json!({"from": {"id": id}, "to": {"id": id, "fx": -0.3, "fy": -3.0}}));
    d.frames(3);
    let style = d.app().session.active_sequence().unwrap().caption_tracks[0].style.clone();
    let p = style.position.expect("placed by hand");
    assert!(p[1] < 0.75, "moved up: {p:?}");
    assert_eq!(d.app().session.history.undo.len(), undo + 1, "one undo step: {}", d.app().ui.status);
    snapshot(&mut d, "caption-drag");
    // the panel puts it back (further down the panel)
    for _ in 0..40 {
        if d.visible("captionStyle.putBack") {
            break;
        }
        d.ok("ui.scroll", json!({"id": "captionStyle.track", "fy": 6.0, "dy": -200.0}));
        d.frames(2);
    }
    d.click("captionStyle.putBack");
    assert_eq!(d.app().session.active_sequence().unwrap().caption_tracks[0].style.position, None);
    // one phrase in capitals, the other as the track
    d.click("captionStyle.phrase");
    for _ in 0..40 {
        if d.visible("captionStyle.uppercase") {
            break;
        }
        d.ok("ui.scroll", json!({"id": "captionStyle.track", "fy": 6.0, "dy": 200.0}));
        d.frames(2);
    }
    d.click("captionStyle.uppercase");
    let t = d.app().session.active_sequence().unwrap().caption_tracks[0].clone();
    assert!(t.caption(first).unwrap().style.as_ref().is_some_and(|s| s.uppercase), "the selected phrase");
    assert!(t.caption(second).unwrap().style.is_none() && !t.style.uppercase, "the rest stay as the track");
    d.click("captionStyle.phraseReset");
    assert!(d.app().session.active_sequence().unwrap().caption_tracks[0].caption(first).unwrap().style.is_none());
}
