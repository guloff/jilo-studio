//! AI ▸ Automatic Editing ▸ Auto Zoom… (also the Text panel's Transcript toolbar): how the stressed phrases are found
//! — by loudness on this computer, or by meaning with the text engine — how far and how the picture
//! zooms, and how far apart. Loudness applies at once (`clip.autoZoom`); meaning runs as a cloud
//! task (consent, progress and Cancel in the status bar and the Progress panel, see
//! `panels::ai_tasks`). Either way the zooms are Scale keyframes in Effect Controls, one undo step.

use egui::RichText;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// Open the dialog (refused, with the reason, when the sequence has no transcript).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("clip.autoZoom").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    app.ui.auto_zoom.dialog = true;
    Ok(json!({"dialog": "autoZoom"}))
}

/// Which clips the zooms go on, as `clip.autoZoom` chooses them: (selected, count).
fn targets(app: &FilmcraftApp) -> (bool, usize) {
    let Some(q) = app.session.active_sequence() else { return (false, 0) };
    let video = |c: &filmcraft_project::ClipId| q.find_item(*c).is_some_and(|(tid, _)| q.video_tracks.iter().any(|t| t.id == tid));
    let selected = app.session.state.selection.iter().filter(|c| video(c)).count();
    if selected > 0 {
        return (true, selected);
    }
    (false, q.video_tracks.first().map_or(0, |t| t.items.iter().filter(|i| i.enabled).count()))
}

fn done_message(r: &Value) -> String {
    tlf!("Auto zoom: {n} zooms on {clips} clips", n = r["moments"].as_array().map_or(0, Vec::len), clips = r["clips"].as_u64().unwrap_or(0))
}

/// The dialog, and the message when a zoom by meaning has been applied.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "clip.autoZoom")
        && let Some((_, r)) = app.ui.ai_task_result.take()
    {
        app.ui.status = done_message(&r);
    }
    if app.ui.auto_zoom.dialog {
        dialog(app, ctx);
    }
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 440.0;
    let (selected, n) = targets(app);
    let provider = app.session.prefs.ai.text_provider();
    egui::Modal::new(egui::Id::new("auto-zoom-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Auto Zoom"), w);
        app.auto.add("autoZoom.dialog", bar, "Auto Zoom");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let on = if selected { tlf!("Zooms go on the selected clips: {n}", n) } else { tlf!("Zooms go on the clips of V1: {n}", n) };
            ui.label(RichText::new(on).color(t.text_dim));
            let z = &mut app.ui.auto_zoom;
            ui.label(RichText::new(tl!("Find stressed phrases")).strong());
            let choices = [
                ("loudness", tl!("By loudness"), tl!("The louder phrases of the sequence mix, on this computer.")),
                ("meaning", tl!("By meaning"), tl!("The text engine picks the stressed phrases; the zoom lands on the quoted words.")),
            ];
            for (id, name, about) in choices {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r = ui.radio_value(&mut z.method, id.to_string(), name);
                    app.auto.add(&format!("autoZoom.method.{id}"), r.rect, name);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.add(egui::Label::new(RichText::new(about).color(t.text_dim).small()).wrap());
                    });
                });
            }
            ui.add_space(4.0);
            egui::Grid::new("auto-zoom-options").num_columns(2).spacing(egui::vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Zoom amount"));
                let r = ui.add(egui::Slider::new(&mut z.percent, 102.0..=160.0).suffix(" %").max_decimals(0));
                app.auto.add("autoZoom.percent", r.rect, "Zoom amount");
                ui.end_row();
                ui.label(tl!("Style"));
                ui.horizontal(|ui| {
                    let c = ui.radio_value(&mut z.style, "cut".to_string(), tl!("Jump"));
                    let s = ui.radio_value(&mut z.style, "smooth".to_string(), tl!("Smooth"));
                    app.auto.add("autoZoom.style.cut", c.rect, "Cut");
                    app.auto.add("autoZoom.style.smooth", s.rect, "Smooth");
                });
                ui.end_row();
                ui.label(tl!("At least, seconds apart"));
                let r = ui.add(egui::DragValue::new(&mut z.min_gap).range(2.0..=60.0).speed(0.5).max_decimals(0));
                app.auto.add("autoZoom.minGap", r.rect, "At least, seconds apart");
                ui.end_row();
            });
            let note = if z.method == "meaning" {
                tlf!("The transcript goes to {provider}. The zooms are Scale keyframes in Effect Controls.", provider = provider.name())
            } else {
                tl!("Nothing is sent. The zooms are Scale keyframes in Effect Controls.").to_string()
            };
            ui.label(RichText::new(note).color(t.text_dim).small());
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add_enabled(n > 0 && app.ui.ai_task.is_none(), crate::dialog_style::primary(tl!("Add Zooms")));
                app.auto.add("autoZoom.go", g.rect, "Add Zooms");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("autoZoom.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let z = &mut app.ui.auto_zoom;
        z.dialog = false;
        let params = json!({
            "method": z.method,
            "intensity": f64::from(z.percent.round()) / 100.0,
            "style": z.style,
            "minGap": f64::from(z.min_gap.round()),
        });
        if z.method == "meaning" {
            crate::panels::ai_tasks::start(app, "clip.autoZoom", params, "Finding zoom moments", "the transcript");
        } else {
            app.ui.status = match app.session.execute("clip.autoZoom", params) {
                Ok(r) => done_message(&r),
                Err(e) => e.to_string(),
            };
        }
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.auto_zoom.dialog = false;
    }
}
