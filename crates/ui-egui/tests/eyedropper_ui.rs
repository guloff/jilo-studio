//! The eyedropper of a colour parameter (#311), driven headless through the control channel: arm it
//! in Effect Controls, click the Program picture, and the parameter takes the picture's colour.

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
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
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
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], true, "{method} {params}: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn center(&mut self, id: &str) -> (f64, f64) {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id}: {v}")).clone();
        let r: Vec<f64> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
    }
    /// The first clip of V1 and its effect list as JSON.
    fn first_clip(&self) -> (u64, Value) {
        let s = &self.harness.state().session;
        let seq = s.active_sequence().expect("a sequence");
        let it = seq.video_tracks[0].items.first().expect("a clip on V1");
        (it.id.0, serde_json::to_value(&it.effects).unwrap())
    }
}

#[test]
fn eyedropper_fills_a_colour_parameter_from_the_program_picture() {
    let mut d = Driver::demo();
    let (clip, _) = d.first_clip();
    d.exec("effects.apply", json!({"clips": [clip], "effect": "Ultra Key"}));
    let (_, effects) = d.first_clip();
    let fx = effects.as_array().unwrap().iter().position(|e| e["effect"] == "ultra_key").expect("Ultra Key applied");
    d.exec("effects.setParam", json!({"clip": clip, "effect": fx, "param": "key_color", "value": "#ffffff"}));
    d.exec("timeline.select", json!({"clips": [clip]}));
    let before = d.first_clip().1[fx]["params"]["key_color"].clone();
    d.ok("ui.panel.show", json!({"panel": "EffectControls"}));
    d.frames(4);
    d.ok("ui.click", json!({"id": "effectControls.ultra_key.key_color.eyedropper"}));
    d.frames(2);
    assert!(d.harness.state().ui.eyedropper.is_some(), "the eyedropper is armed");
    let (x, y) = d.center("program.picture");
    d.ok("ui.click", json!({"x": x, "y": y}));
    for _ in 0..400 {
        d.frames(1);
        if d.harness.state().ui.eyedropper.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(d.harness.state().ui.eyedropper.is_none(), "the pick finished");
    let after = d.first_clip().1[fx]["params"]["key_color"].clone();
    assert_ne!(after, before, "the colour came from the picture");
    assert!(d.harness.state().ui.status.is_empty() || !d.harness.state().ui.status.contains("Eyedropper"), "{}", d.harness.state().ui.status);
}
