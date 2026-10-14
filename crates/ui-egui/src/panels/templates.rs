//! File ▸ New ▸ Project from Template…: a new project from a built-in template (Shorts 9:16,
//! Podcast, Interview, Vlog) or one saved with File ▸ Save as Template… (`file.templates`,
//! `file.newProjectFromTemplate`). With unsaved changes in the open project the dialog says so and
//! the button discards them.
//!
//! Automation ids: `templates.dialog`, `templates.item.<id or name>`, `templates.name`,
//! `templates.create`, `templates.cancel`.

use egui::{RichText, Sense, StrokeKind, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Debug, Default)]
pub struct TemplatesUi {
    pub dialog: bool,
    /// The chosen template: its id (`builtin:…`) or path.
    pub chosen: String,
    pub name: String,
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    let u = &mut app.ui.templates;
    u.dialog = true;
    if u.chosen.is_empty() {
        u.chosen = "builtin:shorts".into();
    }
    if u.name.is_empty() {
        u.name = tl!("Untitled").to_string();
    }
    Ok(json!({"dialog": "templates"}))
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.templates.dialog {
        return;
    }
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 560.0;
    let list: Vec<Value> = app.session.execute("file.templates", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let dirty = app.session.is_dirty();
    egui::Modal::new(egui::Id::new("templates-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("New Project from Template"), w);
        app.auto.add("templates.dialog", bar, "New Project from Template");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let tile = vec2((w - 36.0 - 12.0) / 2.0, 90.0);
            for row in list.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for v in row {
                        let builtin = v["builtin"] == true;
                        let key = if builtin { v["id"].as_str() } else { v["path"].as_str() }.unwrap_or_default().to_string();
                        let name = v["name"].as_str().unwrap_or_default();
                        let about = v["description"].as_str().map_or_else(|| tl!("Saved template").to_string(), |d| crate::i18n::t(d).to_string());
                        let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
                        let on = app.ui.templates.chosen == key;
                        let p = ui.painter();
                        p.rect_filled(
                            rect,
                            6.0,
                            if on {
                                t.row_selected
                            } else if resp.hovered() {
                                t.hover
                            } else {
                                t.field_bg
                            },
                        );
                        p.rect_stroke(rect, 6.0, egui::Stroke::new(if on { 1.5 } else { 1.0 }, if on { t.accent } else { t.field_border }), StrokeKind::Inside);
                        // the frame's shape
                        let vertical = v["id"] == "builtin:shorts";
                        let fr = if vertical { vec2(22.0, 38.0) } else { vec2(40.0, 23.0) };
                        let icon = egui::Rect::from_center_size(rect.left_center() + vec2(30.0, 0.0), fr);
                        p.rect_stroke(icon, 2.0, egui::Stroke::new(1.5, if on { t.accent } else { t.text_dim }), StrokeKind::Inside);
                        let text_x = rect.left() + 62.0;
                        let tw = rect.right() - text_x - 10.0;
                        p.text(egui::pos2(text_x, rect.top() + 10.0), egui::Align2::LEFT_TOP, crate::i18n::t(name), egui::FontId::proportional(14.0), t.text);
                        let mut job = egui::text::LayoutJob::simple(about, egui::FontId::proportional(11.0), t.text_dim, tw);
                        job.wrap.max_rows = 4;
                        let g = ui.painter().layout_job(job);
                        ui.painter().galley(egui::pos2(text_x, rect.top() + 30.0), g, t.text_dim);
                        app.auto.add(&format!("templates.item.{}", if builtin { key.trim_start_matches("builtin:") } else { name }), rect, name);
                        if resp.clicked() {
                            app.ui.templates.chosen = key;
                        }
                        if resp.double_clicked() {
                            go = true;
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label(tl!("Project name"));
                let r = ui.add(egui::TextEdit::singleline(&mut app.ui.templates.name).desired_width(260.0));
                app.auto.add("templates.name", r.rect, "Project name");
            });
            if dirty {
                ui.label(RichText::new(tl!("The open project has unsaved changes. Save it first, or they are discarded.")).color(t.hot_text).small());
            }
            crate::dialog_style::actions(ui, |ui| {
                let label = if dirty { tl!("Discard Changes and Create") } else { tl!("Create") };
                let g = ui.add_enabled(!app.ui.templates.chosen.is_empty(), crate::dialog_style::primary(label));
                app.auto.add("templates.create", g.rect, "Create");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("templates.cancel", c.rect, "Cancel");
                go |= g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let u = &app.ui.templates;
        let name = if u.name.trim().is_empty() { tl!("Untitled").to_string() } else { u.name.trim().to_string() };
        let p = json!({"template": u.chosen, "name": name, "force": dirty});
        app.ui.templates.dialog = false;
        match app.session.execute("file.newProjectFromTemplate", p) {
            Ok(_) => {
                app.stop();
                app.ui.mode = crate::state::Mode::Edit;
                app.ui.status = tlf!("New project “{name}”", name);
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.templates.dialog = false;
    }
}
