//! The crash-recovery prompt and the Revert confirmation (Settings ▸ Auto Save lives in
//! `panels::settings`).
//!
//! Automation ids: `recovery.item.<n>`, `recovery.recover`, `recovery.discard`, `recovery.later`;
//! `revert.yes`, `revert.no`; `quit.save`, `quit.dontSave`, `quit.cancel`.

use egui::{Frame, Margin, RichText, Ui};
use serde_json::json;

use crate::theme::Tokens;
use crate::{Dialog, FilmcraftApp};

/// Dialog-local state.
#[derive(Default)]
pub struct FileDialogState {
    pub recovery_choice: usize,
}

fn modal_frame(t: &Tokens) -> Frame {
    crate::dialog_style::frame(t)
}

fn button(app: &mut FilmcraftApp, ui: &mut Ui, id: &str, label: &str, primary: bool) -> bool {
    let widget = if primary { crate::dialog_style::primary(label) } else { crate::dialog_style::secondary(label) };
    let r = ui.add(widget);
    app.auto.add(id, r.rect, label);
    r.clicked()
}

pub fn show_recovery(app: &mut FilmcraftApp, ctx: &egui::Context) -> bool {
    let t = app.tokens;
    let list = app.session.execute("file.recoveryList", json!({})).unwrap_or_default();
    let items = list.as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return false;
    }
    let mut choice = app.file_dialogs.recovery_choice.min(items.len() - 1);
    let mut open = true;
    egui::Modal::new(egui::Id::new("recovery-modal")).frame(modal_frame(&t)).show(ctx, |ui| {
        ui.set_width(520.0);
        crate::dialog_style::header(ui, tl!("Recover Unsaved Changes"), 520.0);
        Frame::new().inner_margin(Margin { left: 28, right: 28, top: 24, bottom: 20 }).show(ui, |ui| {
            ui.separator();
            ui.add_space(6.0);
            let c = &items[choice];
            let name = c["projectName"].as_str().unwrap_or(tl!("Untitled"));
            let when = c["savedAt"].as_str().unwrap_or("");
            let why = if c["cleanExit"].as_bool() == Some(true) {
                tlf!("FilmCraft was closed while “{name}” had unsaved changes.", name)
            } else {
                tlf!("FilmCraft quit unexpectedly while “{name}” had unsaved changes.", name)
            };
            ui.label(RichText::new(why).size(13.5).color(t.text));
            ui.add_space(4.0);
            ui.label(RichText::new(tlf!("Recover unsaved changes from {when}?", when)).size(13.5).color(t.text));
            if let Some(p) = c["projectPath"].as_str() {
                ui.label(RichText::new(p).size(11.5).color(t.text_dim));
            } else {
                ui.label(RichText::new(tl!("The project had not been saved yet.")).size(11.5).color(t.text_dim));
            }
            if items.len() > 1 {
                ui.add_space(8.0);
                ui.label(RichText::new(tlf!("{n} sessions have unsaved changes:", n = items.len())).size(12.0).color(t.text_dim));
                for (i, it) in items.iter().enumerate() {
                    let label = format!("{} — {}", it["projectName"].as_str().unwrap_or(tl!("Untitled")), it["savedAt"].as_str().unwrap_or(""));
                    let r = ui.radio(choice == i, RichText::new(&label).size(12.5));
                    app.auto.add(&format!("recovery.item.{i}"), r.rect, &label);
                    if r.clicked() {
                        choice = i;
                    }
                }
            }
            ui.add_space(18.0);
            let id = items[choice]["id"].as_str().unwrap_or("").to_string();
            crate::dialog_style::actions(ui, |ui| {
                if button(app, ui, "recovery.later", tl!("Not Now"), false) {
                    open = false;
                }
                if button(app, ui, "recovery.recover", tl!("Recover"), true) {
                    match app.session.execute("file.recover", json!({ "id": id })) {
                        Ok(_) => open = false,
                        Err(e) => app.ui.status = e.to_string(),
                    }
                }
                if button(app, ui, "recovery.discard", tl!("Discard"), false) {
                    if let Err(e) = app.session.execute("file.discardRecovery", json!({ "id": id })) {
                        app.ui.status = e.to_string();
                    }
                    choice = 0;
                }
            });
        });
    });
    app.file_dialogs.recovery_choice = choice;
    open && !app.session.recovery_candidates().is_empty()
}

pub fn show_revert(app: &mut FilmcraftApp, ctx: &egui::Context) -> bool {
    let t = app.tokens;
    let file = app.session.path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut open = true;
    let r = egui::Modal::new(egui::Id::new("revert-modal")).frame(modal_frame(&t)).show(ctx, |ui| {
        ui.set_width(460.0);
        crate::dialog_style::header(ui, tl!("Revert"), 460.0);
        Frame::new().inner_margin(Margin { left: 28, right: 28, top: 24, bottom: 20 }).show(ui, |ui| {
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(tlf!("Are you sure you want to discard your changes to '{file}'?", file)).size(13.5).color(t.text));
            ui.add_space(18.0);
            crate::dialog_style::actions(ui, |ui| {
                if button(app, ui, "revert.yes", tl!("Yes"), true) {
                    if let Err(e) = app.session.execute("file.revert", json!({})) {
                        app.ui.status = e.to_string();
                    }
                    open = false;
                }
                if button(app, ui, "revert.no", tl!("No"), false) {
                    open = false;
                }
            });
        });
    });
    open && !r.should_close()
}

/// "Save changes to '<project>' before quitting?" Save (Save As when the project has no file yet),
/// Don't Save (quit and drop the recovery journal) or Cancel (stay), like Premiere (#266).
pub fn show_quit(app: &mut FilmcraftApp, ctx: &egui::Context) -> bool {
    let t = app.tokens;
    // the file, else the project's name as the title bar shows it (a project not saved yet)
    let name = app.session.path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned());
    let project = app.session.project.name.trim();
    let name = name.or_else(|| (!project.is_empty()).then(|| project.to_string())).unwrap_or_else(|| tl!("Untitled").to_string());
    let mut open = true;
    let mut quit = false;
    let r = egui::Modal::new(egui::Id::new("quit-modal")).frame(modal_frame(&t)).show(ctx, |ui| {
        ui.set_width(480.0);
        Frame::new().inner_margin(Margin { left: 28, right: 28, top: 24, bottom: 20 }).show(ui, |ui| {
            ui.label(RichText::new(tl!("Save Project")).size(18.0).strong().color(t.text));
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(tlf!("Save changes to '{name}' before quitting?", name)).size(13.5).color(t.text));
            ui.add_space(18.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(app, ui, "quit.save", tl!("Save"), true) {
                    // `file.save` on an untitled project opens the Save As dialog.
                    match crate::menus::invoke(app, ctx, "file.save", json!({})) {
                        Ok(_) if !app.session.is_dirty() => quit = true,
                        // the Save As dialog was cancelled: stay open
                        Ok(_) => open = false,
                        Err(e) => {
                            app.ui.status = e;
                            open = false;
                        }
                    }
                }
                if button(app, ui, "quit.dontSave", tl!("Don't Save"), false) {
                    app.discard_on_exit = true;
                    quit = true;
                }
                if button(app, ui, "quit.cancel", tl!("Cancel"), false) {
                    open = false;
                }
            });
        });
    });
    if quit {
        app.quit_confirmed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return false;
    }
    open && !r.should_close()
}

/// Draw the dialog if it is one of ours; returns Some(still open).
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context, d: Dialog) -> Option<bool> {
    match d {
        Dialog::Preferences => Some(crate::panels::settings::show(app, ctx)),
        Dialog::Recovery => Some(show_recovery(app, ctx)),
        Dialog::RevertConfirm => Some(show_revert(app, ctx)),
        Dialog::QuitConfirm => Some(show_quit(app, ctx)),
        _ => None,
    }
}
