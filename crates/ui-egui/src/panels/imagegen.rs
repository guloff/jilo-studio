//! AI ▸ Generate Image…: a description, the image engine (OpenAI or Google Gemini), the
//! shape (the sequence's by default) and whether the picture goes on the timeline at the playhead.
//! It runs as a cloud task (consent, progress and Cancel in the status bar and the Progress panel,
//! see `panels::ai_tasks`); the picture is saved with the project's media and imported as a still
//! (`ai.generateImage`).

use egui::RichText;
use serde_json::{Value, json};

use crate::FilmcraftApp;

const ENGINES: [(&str, &str); 2] = [("openai", "OpenAI (cloud)"), ("gemini", "Google Gemini (cloud)")];
const SHAPES: [(&str, &str); 3] = [("landscape", "Landscape"), ("portrait", "Portrait"), ("square", "Square")];

/// Open the dialog.
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    let g = &mut app.ui.imagegen;
    if g.engine.is_empty() {
        g.engine = app.session.prefs.ai.image_provider().id().to_string();
    }
    // the sequence's shape
    g.shape = app
        .session
        .active_sequence()
        .map(|q| match filmcraft_ai::images::Shape::for_frame(q.settings.width, q.settings.height) {
            filmcraft_ai::images::Shape::Landscape => "landscape",
            filmcraft_ai::images::Shape::Portrait => "portrait",
            filmcraft_ai::images::Shape::Square => "square",
        })
        .unwrap_or("landscape")
        .to_string();
    g.place = app.session.active_sequence().is_some();
    g.dialog = true;
    Ok(json!({"dialog": "generateImage"}))
}

/// The dialog, and the message when a picture has been added.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "ai.generateImage")
        && let Some((_, r)) = app.ui.ai_task_result.take()
    {
        let name = r["path"].as_str().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        app.ui.status = tlf!("Image added: {name}", name);
    }
    if app.ui.imagegen.dialog {
        dialog(app, ctx);
    }
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 480.0;
    let has_sequence = app.session.active_sequence().is_some();
    egui::Modal::new(egui::Id::new("generate-image-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Generate Image"), w);
        app.auto.add("imagegen.dialog", bar, "Generate Image");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let g = &mut app.ui.imagegen;
            ui.label(RichText::new(tl!("Describe the picture")).strong());
            let r = ui.add(
                egui::TextEdit::multiline(&mut g.prompt)
                    .desired_rows(4)
                    .desired_width(w - 36.0)
                    .hint_text(tl!("A lighthouse on a cliff at dawn, soft mist, wide shot")),
            );
            app.auto.add("imagegen.prompt", r.rect, "Describe the picture");
            egui::Grid::new("imagegen-options").num_columns(2).spacing(egui::vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Engine"));
                let cur = ENGINES.iter().find(|e| e.0 == g.engine).map_or("", |e| e.1);
                let r = egui::ComboBox::from_id_salt("imagegen.engine").width(240.0).selected_text(crate::i18n::t(cur)).show_ui(ui, |ui| {
                    for (id, name) in ENGINES {
                        ui.selectable_value(&mut g.engine, id.to_string(), crate::i18n::t(name));
                    }
                });
                app.auto.add("imagegen.engine", r.response.rect, "Engine");
                ui.end_row();
                ui.label(tl!("Picture shape"));
                ui.horizontal(|ui| {
                    for (id, name) in SHAPES {
                        let r = ui.radio_value(&mut g.shape, id.to_string(), crate::i18n::t(name));
                        app.auto.add(&format!("imagegen.shape.{id}"), r.rect, name);
                    }
                });
                ui.end_row();
            });
            if has_sequence {
                let r = ui.checkbox(&mut g.place, tl!("Put it on the timeline at the playhead"));
                app.auto.add("imagegen.place", r.rect, "Put it on the timeline at the playhead");
            }
            let provider = filmcraft_ai::Provider::from_id(&g.engine).map_or("", |p| p.name());
            ui.add(
                egui::Label::new(
                    RichText::new(tlf!(
                        "The description goes to {provider}. The picture is saved with the project's media and imported as a still; undo removes it.",
                        provider
                    ))
                    .color(t.text_dim)
                    .small(),
                )
                .wrap(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let ready = !g.prompt.trim().is_empty() && app.ui.ai_task.is_none();
                let b = ui.add_enabled(ready, crate::dialog_style::primary(tl!("Generate")));
                app.auto.add("imagegen.go", b.rect, "Generate");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("imagegen.cancel", c.rect, "Cancel");
                go = b.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let g = &mut app.ui.imagegen;
        g.dialog = false;
        let params = json!({"prompt": g.prompt.trim(), "engine": g.engine, "shape": g.shape, "place": g.place && has_sequence});
        crate::panels::ai_tasks::start(app, "ai.generateImage", params, "Generating an image", "the picture's description");
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.imagegen.dialog = false;
    }
}
