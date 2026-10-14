//! Headless UI test of stickers: the Library's Stickers tab puts an emoji at the playhead with a
//! click (a right-click keeps it among the favourites), and the sticker's box in the Program
//! monitor moves and scales it. `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `stickers.png`
//! (`FILMCRAFT_UI_LANG=ru`: Russian).

#![allow(dead_code)]

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ClipId;
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
fn a_sticker_from_the_library_is_placed_moved_and_scaled() {
    let dir = std::env::temp_dir().join(format!("jilo-stickers-ui-{}", std::process::id()));
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    // an orange square stands in for the system's emoji
    s.emoji_renderer = Some(Arc::new(|_: &str, size: u32| Ok((size, size, [255u8, 140, 0, 255].repeat((size * size) as usize)))));
    let _ = dir;
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Libraries"}));
    d.frames(2);
    d.click("library.tab.stickers");
    d.click("library.stickers.cat.hearts");
    d.frames(2);
    let undo = d.app().session.history.undo.len();
    d.click("library.sticker.emoji:🔥");
    d.frames(3);
    let app = d.app();
    assert_eq!(app.session.history.undo.len(), undo + 1, "{}", app.ui.status);
    let clip = app.session.state.selection[0];
    // favourite with a right-click
    d.ok("ui.click", json!({"id": "library.sticker.emoji:🔥", "button": "right"}));
    d.frames(2);
    assert!(d.app().session.prefs.stickers.favorites.contains(&"emoji:🔥".to_string()));
    // move it: past the pop-in, drag its box in the Program monitor
    let at = d.app().session.playhead();
    let later = at + filmcraft_time::Tick::from_seconds_f64(1.0);
    d.app().session.set_playhead(later);
    d.frames(3);
    snapshot(&mut d, "stickers");
    let pos = |d: &mut Driver| {
        let q = d.app().session.active_sequence().unwrap();
        let (_, it) = q.find_item(ClipId(clip.0)).unwrap();
        let m = it.effect("motion").unwrap();
        let t = it.effect_time_at(later);
        (m.vec2_at("position", t), m.f64_at("scale", t))
    };
    let (p0, s0) = pos(&mut d);
    let id = format!("program.clip.{}.box", clip.0);
    d.ok("ui.drag", json!({"from": {"id": id}, "to": {"id": id, "fx": 1.4, "fy": 1.2}}));
    d.frames(3);
    let (p1, _) = pos(&mut d);
    assert!(p1.x > p0.x + 5.0 && p1.y > p0.y + 2.0, "moved: {p0:?} → {p1:?}");
    let corner = format!("program.clip.{}.corner.2", clip.0);
    d.ok("ui.drag", json!({"from": {"id": corner}, "to": {"id": corner, "fx": 4.0, "fy": 4.0}}));
    d.frames(3);
    let (_, s1) = pos(&mut d);
    assert!(s1 > s0, "scaled up: {s0} → {s1}");
}

#[test]
fn the_selected_clip_is_cut_out_into_my_stickers() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    // the "subject" is the middle quarter of the picture
    s.cutout = Some(Arc::new(|rgba: &[u8], w: usize, h: usize| {
        let (cw, ch) = (w / 2, h / 2);
        let mut out = Vec::with_capacity(cw * ch * 4);
        for y in h / 4..h / 4 + ch {
            out.extend_from_slice(&rgba[(y * w + w / 4) * 4..(y * w + w / 4 + cw) * 4]);
        }
        Ok((cw as u32, ch as u32, out))
    }));
    let t = s.playhead();
    let clip = s.active_sequence().unwrap().video_tracks.iter().flat_map(|tr| tr.items.iter()).find(|i| i.range().contains(t)).unwrap().id;
    s.state.selection = vec![clip];
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"id": "window.panel.Libraries"}));
    d.frames(2);
    d.click("library.tab.stickers");
    d.click("library.stickers.cat.mine");
    d.frames(2);
    let undo = d.app().session.history.undo.len();
    d.click("library.stickers.cutOut");
    d.frames(3);
    let app = d.app();
    assert_eq!(app.session.history.undo.len(), undo + 1, "{}", app.ui.status);
    let mine = app.session.execute("stickers.list", json!({"category": "mine"})).unwrap();
    let id = mine["stickers"][0]["id"].as_str().unwrap().to_string();
    let path = mine["stickers"][0]["path"].as_str().unwrap().to_string();
    assert!(d.has(&format!("library.sticker.{id}")), "the tile shows");
    snapshot(&mut d, "my-stickers");
    let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap().parent().unwrap());
}
