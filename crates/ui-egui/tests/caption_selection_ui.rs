//! Captions in the Timeline: Shift-click selects every caption from the last one clicked, Cmd-click
//! adds or takes one out, a marquee takes captions and clips together, a drag of either moves both
//! by the same time (they stay in sync), and a right-click opens the caption menu.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ClipId;
use filmcraft_time::Tick;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    /// The demo project with a caption track: captions at 1, 3, 5 and 7 s, 1 s each. The Timeline
    /// maximized and zoomed to the sequence.
    fn new() -> (Self, Vec<ClipId>) {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.execute("captions.newTrack", json!({})).unwrap();
        for t in [1.0, 3.0, 5.0, 7.0] {
            s.execute("captions.add", json!({"seconds": t, "durationSeconds": 1.0, "text": format!("cap {t}"), "split": false})).unwrap();
        }
        s.state.caption_selection.clear();
        s.set_playhead(Tick::from_seconds_f64(20.0));
        let caps: Vec<ClipId> = s.active_sequence().unwrap().caption_tracks[0].captions.iter().map(|c| c.id).collect();
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(s).with_control(rx);
        let mut b = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000);
        // `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>`: render offscreen with wgpu and write snapshots there
        if std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").is_some() {
            b = b.wgpu();
        }
        let harness = b.build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx };
        d.frames(4);
        d.ok("ui.set", json!({"focused": "Timeline"}));
        d.ok("ui.menu.invoke", json!({"id": "window.maximizeFrame"}));
        d.ok("ui.menu.invoke", json!({"id": "view.zoomToSequence"}));
        d.frames(6);
        (d, caps)
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
            if let Ok(r) = reply.try_recv() {
                assert_eq!(r["ok"], true, "{method} {params}: {r}");
                self.frames(2);
                return r["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn rect(&mut self, id: &str) -> [f32; 4] {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no element {id}")).clone();
        let r: Vec<f32> = e["rect"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
        [r[0], r[1], r[2], r[3]]
    }

    fn has(&mut self, id: &str) -> bool {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        v.as_array().unwrap().iter().any(|e| e["id"] == id)
    }

    fn send(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    /// Press, move in steps, release, with keyboard modifiers held throughout (a click when `from`
    /// and `to` are the same point).
    fn gesture(&mut self, from: egui::Pos2, to: egui::Pos2, modifiers: egui::Modifiers) {
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
        self.send(egui::Event::ModifiersChanged(modifiers));
        self.send(egui::Event::PointerMoved(from));
        self.send(button(from, true));
        if from != to {
            for k in 1..=12 {
                self.send(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
            }
        }
        self.send(button(to, false));
        self.frames(3);
        self.send(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
    }

    fn click_caption(&mut self, c: ClipId, modifiers: egui::Modifiers) {
        let p = centre(self.rect(&format!("timeline.caption.{}", c.0)));
        self.gesture(p, p, modifiers);
    }

    fn snapshot(&mut self, name: &str) {
        let Some(dir) = std::env::var_os("FILMCRAFT_UI_SNAPSHOT_DIR").map(std::path::PathBuf::from) else { return };
        self.frames(2);
        if let Ok(img) = self.harness.render() {
            std::fs::create_dir_all(&dir).unwrap();
            img.save(dir.join(format!("{name}.png"))).unwrap();
        }
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn captions(&mut self) -> Vec<ClipId> {
        let mut v = self.app().session.state.caption_selection.clone();
        v.sort_by_key(|c| c.0);
        v
    }

    fn caption_start(&mut self, c: ClipId) -> Tick {
        self.app().session.active_sequence().unwrap().find_caption(c).unwrap().1.start
    }

    fn pps(&mut self) -> f32 {
        self.app().ui.timeline.pps as f32
    }
}

fn centre(r: [f32; 4]) -> egui::Pos2 {
    egui::pos2(r[0] + r[2] / 2.0, r[1] + r[3] / 2.0)
}

fn sorted(mut v: Vec<ClipId>) -> Vec<ClipId> {
    v.sort_by_key(|c| c.0);
    v
}

#[test]
fn shift_click_selects_a_range_and_cmd_click_toggles() {
    let (mut d, caps) = Driver::new();
    d.click_caption(caps[0], egui::Modifiers::NONE);
    assert_eq!(d.captions(), vec![caps[0]]);
    // Shift-click on the last: the first, the last and every caption between
    d.click_caption(caps[3], egui::Modifiers::SHIFT);
    assert_eq!(d.captions(), sorted(caps.clone()));
    // Cmd-click takes one out, and puts it back
    d.click_caption(caps[2], egui::Modifiers::COMMAND);
    assert_eq!(d.captions(), sorted(vec![caps[0], caps[1], caps[3]]));
    d.click_caption(caps[2], egui::Modifiers::COMMAND);
    assert_eq!(d.captions().len(), 4);
    // a plain click selects one again
    d.click_caption(caps[1], egui::Modifiers::NONE);
    assert_eq!(d.captions(), vec![caps[1]]);
}

#[test]
fn a_marquee_takes_captions_and_clips_together() {
    let (mut d, caps) = Driver::new();
    let c0 = d.rect(&format!("timeline.caption.{}", caps[0].0));
    let c1 = d.rect(&format!("timeline.caption.{}", caps[1].0));
    // from empty caption lane before the first caption (well clear of its edge) down into V1,
    // across the first two captions
    let v1 = d.rect("timeline.track.V1.target");
    let from = egui::pos2(c0[0] - 0.4 * d.pps(), c0[1] + c0[3] / 2.0);
    let to = egui::pos2(c1[0] + c1[2] / 2.0, v1[1] + v1[3] / 2.0);
    d.gesture(from, to, egui::Modifiers::NONE);
    assert_eq!(d.captions(), sorted(vec![caps[0], caps[1]]), "the captions under the marquee");
    let first_shot = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].id;
    assert!(d.app().session.state.selection.contains(&first_shot), "and the clips: {:?}", d.app().session.state.selection);
}

#[test]
fn dragging_a_clip_moves_the_captions_selected_with_it_and_back() {
    let (mut d, caps) = Driver::new();
    // the V2 title and the 7 s caption, selected together
    let clip = d.app().session.active_sequence().unwrap().video_tracks[1].items[0].clone();
    d.app().session.execute("timeline.select", json!({"clips": [clip.id.0]})).unwrap();
    d.app().session.execute("captions.select", json!({"captions": [caps[3].0], "add": true})).unwrap();
    let cap_was = d.caption_start(caps[3]);
    d.frames(2);
    // the clip half a second later, by its body
    let r = d.rect(&format!("timeline.clip.{}", clip.id.0));
    let from = centre(r);
    let dx = 0.5 * d.pps();
    d.gesture(from, from + egui::vec2(dx, 0.0), egui::Modifiers::NONE);
    let moved = d.app().session.active_sequence().unwrap().find_item(clip.id).unwrap().1.start - clip.start;
    assert!(moved > Tick::ZERO, "the clip moved ({})", d.app().ui.status);
    assert_eq!(d.caption_start(caps[3]) - cap_was, moved, "the caption moved exactly as far");
    // now the caption, by its body, back: the clip comes along
    let (cap_now, clip_now) = (d.caption_start(caps[3]), d.app().session.active_sequence().unwrap().find_item(clip.id).unwrap().1.start);
    let r = d.rect(&format!("timeline.caption.{}", caps[3].0));
    let from = centre(r);
    d.gesture(from, from - egui::vec2(dx, 0.0), egui::Modifiers::NONE);
    let back = d.caption_start(caps[3]) - cap_now;
    assert!(back < Tick::ZERO, "the caption moved back ({})", d.app().ui.status);
    let clip_back = d.app().session.active_sequence().unwrap().find_item(clip.id).unwrap().1.start - clip_now;
    assert_eq!(clip_back, back, "the clip moved exactly as far");
}

#[test]
fn the_caption_menu_selects_a_track_splits_and_deletes() {
    let (mut d, caps) = Driver::new();
    // right-click a caption: it is selected and the menu opens
    d.ok("ui.click", json!({"id": format!("timeline.caption.{}", caps[1].0), "button": "right"}));
    d.frames(3);
    assert_eq!(d.captions(), vec![caps[1]]);
    // with the clips of the same time selected too, as a marquee would
    let first_shot = d.app().session.active_sequence().unwrap().video_tracks[0].items[0].id;
    d.app().session.execute("timeline.select", json!({"clips": [first_shot.0], "add": true})).unwrap();
    d.snapshot("caption-menu");
    for id in [
        "edit.copy",
        "caption.editText",
        "captions.split",
        "caption.selectTrack",
        "caption.selectForward",
        "caption.style",
        "edit.clear",
        "caption.rippleDelete",
    ] {
        assert!(d.has(&format!("timeline.captionMenu.{id}")), "{id} missing");
    }
    // Select All on Track
    d.ok("ui.click", json!({"id": "timeline.captionMenu.caption.selectTrack"}));
    d.frames(3);
    assert_eq!(d.captions(), sorted(caps.clone()));
    // Split at Playhead: with the playhead inside the 5 s caption
    let mid = d.caption_start(caps[2]) + Tick::from_seconds_f64(0.5);
    d.app().session.set_playhead(mid);
    d.ok("ui.click", json!({"id": format!("timeline.caption.{}", caps[2].0), "button": "right"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "timeline.captionMenu.captions.split"}));
    d.frames(3);
    let n = d.app().session.active_sequence().unwrap().caption_tracks[0].captions.len();
    assert_eq!(n, 5, "split in two ({})", d.app().ui.status);
    // Delete removes what is selected
    d.click_caption(caps[0], egui::Modifiers::NONE);
    d.ok("ui.click", json!({"id": format!("timeline.caption.{}", caps[0].0), "button": "right"}));
    d.frames(3);
    d.ok("ui.click", json!({"id": "timeline.captionMenu.edit.clear"}));
    d.frames(3);
    assert!(d.app().session.active_sequence().unwrap().find_caption(caps[0]).is_none());
}
