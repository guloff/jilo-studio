//! AI ▸ Automatic Editing ▸ Auto Multi-Camera by Speaker…: the angle of the multi-camera clips
//! (selected, else all in the sequence) follows whoever speaks — by the transcript's speakers or
//! by each camera's microphone — with a shortest shot, a wide camera for crosstalk and silences,
//! and how early the cut comes (`multicam.autoSwitch`). One undo step.
//!
//! Automation ids: `autoMulticam.dialog`, `autoMulticam.method.<auto|speakers|loudness>`,
//! `autoMulticam.minShot`, `autoMulticam.wide`, `autoMulticam.lead`, `autoMulticam.go`, `autoMulticam.cancel`.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Debug)]
pub struct AutoMulticamUi {
    pub dialog: bool,
    /// `auto`, `speakers` or `loudness`.
    pub method: String,
    pub min_shot: f32,
    /// Wide camera, 1-based (0 = none).
    pub wide: u32,
    pub lead: f32,
}

impl Default for AutoMulticamUi {
    fn default() -> Self {
        Self { dialog: false, method: "auto".into(), min_shot: 2.0, wide: 0, lead: 0.25 }
    }
}

/// The camera names of the first multi-camera clip in the sequence.
fn cameras(app: &FilmcraftApp) -> Vec<String> {
    let Some(q) = app.session.active_sequence() else { return Vec::new() };
    let Some(it) = q.video_tracks.iter().flat_map(|t| t.items.iter()).find(|i| i.multicam.is_some_and(|m| m.enabled)) else { return Vec::new() };
    let Some(src) = app.session.project.sequence(it.item) else { return Vec::new() };
    let c = src.cameras();
    c.shown_angles().into_iter().map(|a| c.cameras[a].name.clone()).collect()
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("multicam.autoSwitch").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    if cameras(app).len() < 2 {
        return Err("put a multi-camera clip in the sequence first (Clip ▸ Create Multi-Camera Source Sequence…)".into());
    }
    app.ui.auto_multicam.dialog = true;
    Ok(json!({"dialog": "autoMulticam"}))
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.auto_multicam.dialog {
        return;
    }
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 460.0;
    let cams = cameras(app);
    egui::Modal::new(egui::Id::new("auto-multicam-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Auto Multi-Camera by Speaker"), w);
        app.auto.add("autoMulticam.dialog", bar, "Auto Multi-Camera by Speaker");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(tl!("Who speaks is on screen")).strong());
            let u = &mut app.ui.auto_multicam;
            for (id, name, about) in [
                ("auto", tl!("Automatic"), tl!("By the transcript's speakers when it has them, else by loudness.")),
                ("speakers", tl!("By the transcript's speakers"), tl!("Speaker 1 on camera 1, speaker 2 on camera 2… Needs a transcript with speakers.")),
                ("loudness", tl!("By each camera's microphone"), tl!("The camera whose own sound is loudest. Works on this computer, no transcript needed.")),
            ] {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r = ui.radio_value(&mut u.method, id.to_string(), name);
                    app.auto.add(&format!("autoMulticam.method.{id}"), r.rect, name);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.add(egui::Label::new(RichText::new(about).color(t.text_dim).small()).wrap());
                    });
                });
            }
            let u = &mut app.ui.auto_multicam;
            let mut rects = Vec::new();
            egui::Grid::new("auto-multicam-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Shortest shot"));
                let r = ui.add(egui::Slider::new(&mut u.min_shot, 0.5..=10.0).suffix(tl!(" s")).max_decimals(1));
                rects.push(("autoMulticam.minShot", r.rect));
                ui.end_row();
                ui.label(tl!("Wide shot"));
                let cur = if u.wide == 0 { tl!("None").to_string() } else { cams.get(u.wide as usize - 1).cloned().unwrap_or_default() };
                let r = egui::ComboBox::from_id_salt("autoMulticam.wide").width(200.0).selected_text(cur).show_ui(ui, |ui| {
                    ui.selectable_value(&mut u.wide, 0, tl!("None"));
                    for (k, name) in cams.iter().enumerate() {
                        ui.selectable_value(&mut u.wide, k as u32 + 1, name);
                    }
                });
                rects.push(("autoMulticam.wide", r.response.rect));
                ui.end_row();
                ui.label(tl!("Cut before the speech"));
                let r = ui.add(egui::Slider::new(&mut u.lead, 0.0..=1.0).suffix(tl!(" s")).max_decimals(2));
                rects.push(("autoMulticam.lead", r.rect));
                ui.end_row();
            });
            for (id, r) in rects {
                app.auto.add(id, r, id);
            }
            ui.add(
                egui::Label::new(
                    RichText::new(tl!("The wide shot takes crosstalk and long silences. The selected multi-camera clips are cut, or all of them when none is selected; every cut can be changed afterwards. One Undo takes it back."))
                        .color(t.text_dim)
                        .small(),
                )
                .wrap(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add(crate::dialog_style::primary(tl!("Cut")));
                app.auto.add("autoMulticam.go", g.rect, "Cut");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("autoMulticam.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let u = &app.ui.auto_multicam;
        let mut p = json!({"minShot": (f64::from(u.min_shot) * 10.0).round() / 10.0, "lead": (f64::from(u.lead) * 100.0).round() / 100.0});
        if u.method != "auto" {
            p["method"] = json!(u.method);
        }
        if u.wide > 0 {
            p["wide"] = json!(u.wide);
        }
        app.ui.auto_multicam.dialog = false;
        app.ui.status = match app.session.execute("multicam.autoSwitch", p) {
            Ok(r) => tlf!("Multi-camera cuts: {n}", n = r["cuts"].as_u64().unwrap_or(0)),
            Err(e) => e.to_string(),
        };
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.auto_multicam.dialog = false;
    }
}
