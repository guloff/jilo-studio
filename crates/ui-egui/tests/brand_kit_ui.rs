//! Headless UI test of Graphics and Titles ▸ Brand Kit…: the window opens from the menu, Apply to
//! Sequence puts the kit's logo and caption style on the open sequence as one undo step, and a
//! lower third in the kit's colours goes in at the playhead.
//!
//! Set `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` to write `brand-kit-*.png` there (`FILMCRAFT_UI_LANG=ru`
//! for the Russian interface).

#![allow(dead_code)]

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_engine::brand_kit::{BrandKit, LOGO_TRACK};
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
fn a_brand_kit_goes_on_the_sequence_in_one_step() {
    let dir = std::env::temp_dir().join(format!("jilo-brand-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let logo = dir.join("logo.png");
    image::RgbaImage::from_pixel(300, 120, image::Rgba([250, 200, 30, 255])).save(&logo).unwrap();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("captions.add", json!({"seconds": 4.0, "durationSeconds": 3.0, "text": "Brand kit"})).unwrap();
    let mut d = Driver::new(s);
    if std::env::var("FILMCRAFT_UI_LANG").as_deref() == Ok("ru") {
        d.ok("ui.menu.invoke", json!({"id": "app.language.russian"}));
    }
    d.ok("ui.menu.invoke", json!({"command": "graphics.brandKit"}));
    d.frames(3);
    assert!(d.has("brandKit.window"));
    d.app().ui.brand_kit.draft = BrandKit {
        name: "Jilo".into(),
        logo: Some(logo.to_string_lossy().to_string()),
        colors: vec!["#7c3aed".into(), "#111827".into(), "#facc15".into()],
        caption_preset: Some("karaoke-yellow".into()),
        ..Default::default()
    };
    d.frames(3);
    snapshot(&mut d, "brand-kit-window");
    let undo = d.app().session.history.undo.len();
    d.click("brandKit.apply");
    d.frames(2);
    let app = d.app();
    assert_eq!(app.session.history.undo.len(), undo + 1, "one undo step: {}", app.ui.status);
    let q = app.session.active_sequence().unwrap();
    let logo_track = q.video_tracks.iter().find(|t| t.name == LOGO_TRACK).expect("the logo track");
    assert_eq!(logo_track.items.len(), 1);
    let preset = filmcraft_engine::caption_presets::find("karaoke-yellow").unwrap();
    assert_eq!(q.caption_tracks[0].style.highlight, preset.style.highlight, "the caption style is the kit's");
    snapshot(&mut d, "brand-kit-applied");
    // a lower third at the playhead, in the kit's main colour
    d.app().ui.brand_kit.lt_text = "Rustam Gulov".into();
    d.click("brandKit.addLowerThird");
    d.frames(2);
    let app = d.app();
    let clip = app
        .session
        .active_sequence()
        .unwrap()
        .video_tracks
        .iter()
        .flat_map(|t| t.items.iter())
        .find(|i| i.graphic.as_ref().is_some_and(|g| g.template.is_some()))
        .map(|i| i.id.0);
    let c = app.session.execute("graphics.template.controls", json!({"clip": clip.expect("a lower third")})).unwrap();
    let val = |id: &str| c["controls"].as_array().unwrap().iter().find(|x| x["id"] == id).map(|x| x["value"].clone()).unwrap();
    assert_eq!(val("name"), "Rustam Gulov");
    assert_eq!(val("slab_color").as_str().map(str::to_ascii_lowercase).as_deref(), Some("#7c3aed"));
    let _ = std::fs::remove_dir_all(&dir);
}
