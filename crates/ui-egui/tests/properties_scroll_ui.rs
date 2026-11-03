//! The Properties panel scrolls: under a tall Timeline it is short, and its last settings (the
//! Audio section of a clip with sound) are reached with the wheel instead of being cut off.

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
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(r) = reply.try_recv() {
                assert_eq!(r["ok"], true, "{method} {params}: {r}");
                self.frames(2);
                return r["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn rect(&mut self, id: &str) -> Option<[f32; 4]> {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id)?.clone();
        let r: Vec<f32> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
        Some([r[0], r[1], r[2], r[3]])
    }
}

#[test]
fn a_short_properties_panel_scrolls_to_the_audio_settings() {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    // a clip with its linked sound: Properties shows Transform, Opacity… and Audio under them
    let q = s.active_sequence().unwrap();
    let clip = q.video_tracks.iter().flat_map(|t| &t.items).find(|i| i.link.is_some()).map(|i| i.id).expect("a linked clip");
    s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
    let (tx, rx) = channel();
    let app = FilmcraftApp::new(s).with_control(rx);
    // a short window: the top row (Source | Program | Properties) gets little height
    let harness = Harness::builder().with_size(egui::vec2(1500.0, 640.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
    let mut d = Driver { harness, tx };
    d.frames(4);
    d.ok("ui.set", json!({"workspace": "Editing"}));
    d.frames(4);
    let panel = d.rect("panel.Properties").expect("the Properties panel is open");
    let bottom = panel[1] + panel[3];
    let audio = d.rect("properties.section.audio").expect("an Audio section");
    assert!(audio[1] > bottom, "the panel is short enough that Audio starts below it: {audio:?} vs panel {panel:?}");
    let (x, y) = (panel[0] + panel[2] / 2.0, panel[1] + panel[3] / 2.0);
    for _ in 0..12 {
        d.ok("ui.scroll", json!({"x": x, "y": y, "dy": -200.0}));
    }
    d.frames(4);
    let audio = d.rect("properties.section.audio").unwrap();
    assert!(audio[1] >= panel[1] && audio[1] + audio[3] <= bottom + 1.0, "scrolled into view: {audio:?} in panel {panel:?}");
}
