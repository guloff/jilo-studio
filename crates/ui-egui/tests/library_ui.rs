//! Headless UI test of the Library panel: its tabs list the project's media, text templates,
//! effects and caption styles, and a double-click puts one in. `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>`
//! writes `library-media.png` and `library-text.png` (`FILMCRAFT_UI_LANG=ru`: Russian).

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

/// The "+" of a tile (a double-click does the same).
fn double(d: &mut Driver, id: &str) {
    let plus = id.replacen("library.item.", "library.add.", 1);
    d.ok("ui.move", json!({"id": id}));
    d.frames(1);
    d.ok("ui.click", json!({"id": plus}));
    d.frames(3);
}

#[test]
fn the_library_puts_media_text_effects_and_styles_in() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    let aurora = s.project.items.iter().find(|(_, i)| i.name.starts_with("Aurora")).map(|(id, _)| id.0).unwrap();
    s.execute("captions.add", json!({"seconds": 1.0, "durationSeconds": 2.0, "text": "Hi"})).unwrap();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Libraries"}));
    d.frames(4);
    assert!(d.has(&format!("library.item.media.{aurora}")));
    snapshot(&mut d, "library-media");
    // media at the playhead on a free track
    let undo = d.app().session.history.undo.len();
    let at = d.app().session.playhead();
    double(&mut d, &format!("library.item.media.{aurora}"));
    let app = d.app();
    assert_eq!(app.session.history.undo.len(), undo + 1, "{}", app.ui.status);
    let q = app.session.active_sequence().unwrap();
    let sel = app.session.state.selection.clone();
    assert!(sel.iter().any(|c| q.find_item(*c).is_some_and(|(_, it)| it.item.0 == aurora && it.start == at)));
    // an effect on the selected clip
    d.click("library.tab.effects");
    double(&mut d, "library.item.effects.gaussian_blur");
    let app = d.app();
    let q = app.session.active_sequence().unwrap();
    assert!(sel.iter().any(|c| q.find_item(*c).is_some_and(|(_, it)| it.effect("gaussian_blur").is_some())), "{}", app.ui.status);
    // a caption style on every caption (one at the top of the list)
    d.click("library.tab.captions");
    let preset = &filmcraft_engine::caption_presets::builtin()[2];
    double(&mut d, &format!("library.item.captions.{}", preset.id));
    let style = d.app().session.active_sequence().unwrap().caption_tracks[0].style.clone();
    assert_eq!(
        (style.font.clone(), style.outline, style.uppercase),
        (preset.style.font.clone(), preset.style.outline, preset.style.uppercase),
        "{}",
        preset.id
    );
    // a title at the playhead
    d.click("library.tab.text");
    d.frames(2);
    snapshot(&mut d, "library-text");
    let graphics =
        |d: &mut Driver| d.app().session.active_sequence().unwrap().video_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.graphic.is_some()).count();
    let before = graphics(&mut d);
    double(&mut d, "library.item.text.builtin:lower-third-slab");
    assert_eq!(graphics(&mut d), before + 1, "{}", d.app().ui.status);
}
