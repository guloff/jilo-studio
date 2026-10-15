//! AI ▸ Recipes…: run a saved series of editing steps on the selection and the open sequence, look
//! at and change a recipe's steps (turn one off, move it, remove it), and record a new recipe while
//! editing as usual. The window stays open beside the work (it is not modal); while a recipe is
//! being recorded and the window is closed, a small "● REC" badge brings it back. Every action is a
//! `recipes.*` command.
//!
//! Automation ids: `recipes.window`, `recipes.item.<name>`, `recipes.run`, `recipes.save`,
//! `recipes.delete`, `recipes.record`, `recipes.stopRecording`, `recipes.discardRecording`,
//! `recipes.recordName`, `recipes.step.<n>.enabled|up|down|remove`, `recipes.rec` (the badge).

use egui::{Color32, RichText, vec2};
use filmcraft_engine::recipes::Recipe;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The window's state.
#[derive(Clone, Debug, Default)]
pub struct RecipesUi {
    pub open: bool,
    /// The recipe shown, as edited (None: nothing chosen).
    pub draft: Option<Recipe>,
    /// The draft differs from the recipe it came from.
    pub dirty: bool,
    /// Name for the recipe being recorded.
    pub record_name: String,
    /// A running recipe's job, watched for its outcome.
    pub job: Option<u64>,
    pub message: String,
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    app.ui.recipes.open = true;
    if app.ui.recipes.draft.is_none() {
        app.ui.recipes.draft = filmcraft_engine::recipes::all().into_iter().next();
    }
    Ok(json!({"window": "recipes"}))
}

/// The label of a step's command, as the menus show it.
fn step_label(command: &str) -> String {
    filmcraft_engine::find_command(command).map_or_else(|| command.to_string(), |c| crate::i18n::t(c.label).trim_end_matches('…').to_string())
}

/// A short form of a step's parameters (empty for none).
fn params_text(p: &Value) -> String {
    match p.as_object() {
        Some(o) if !o.is_empty() => {
            let s = o.iter().map(|(k, v)| format!("{k}: {}", v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect::<Vec<_>>().join(", ");
            if s.chars().count() > 60 { format!("{}…", s.chars().take(59).collect::<String>()) } else { s }
        }
        _ => String::new(),
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // the outcome of a run, once it ends
    if let Some(id) = app.ui.recipes.job
        && app.session.jobs.iter().find(|j| j.id == id).is_none_or(|j| j.to_json()["finished"] == true)
    {
        app.ui.recipes.job = None;
        let st = app.session.execute("recipes.status", json!({})).unwrap_or_default();
        app.ui.recipes.message = match st["error"].as_str() {
            Some(e) => tlf!("The recipe stopped: {error}", error = e),
            None => tlf!("{name} done. One Undo takes it all back.", name = st["name"].as_str().unwrap_or_default()),
        };
        app.ui.status = app.ui.recipes.message.clone();
    }
    let recording = app.session.recording.as_ref().map(Vec::len);
    if !app.ui.recipes.open {
        if recording.is_some() {
            badge(app, ctx);
        }
        return;
    }
    let t = app.tokens;
    let mut open = true;
    let mut actions: Vec<(String, Value)> = Vec::new();
    let all = filmcraft_engine::recipes::all();
    crate::dialog_style::Window::new(tl!("Recipes"))
        .id(egui::Id::new("recipes-window"))
        .open(&mut open)
        .default_size(vec2(640.0, 420.0))
        .collapsible(true)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            app.auto.add("recipes.window", ui.max_rect(), "Recipes");
            // recording
            match recording {
                Some(n) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tlf!("● Recording: {n} steps", n)).color(Color32::from_rgb(235, 64, 64)).strong());
                        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.recipes.record_name).hint_text(tl!("Recipe name")).desired_width(180.0));
                        app.auto.add("recipes.recordName", r.rect, "Recipe name");
                        let named = !app.ui.recipes.record_name.trim().is_empty();
                        let r = ui.add_enabled(named && n > 0, egui::Button::new(tl!("Stop and Save")));
                        app.auto.add("recipes.stopRecording", r.rect, "Stop and Save");
                        if r.clicked() {
                            actions.push(("recipes.recordStop".into(), json!({"name": app.ui.recipes.record_name.trim()})));
                        }
                        let r = ui.button(tl!("Discard"));
                        app.auto.add("recipes.discardRecording", r.rect, "Discard");
                        if r.clicked() {
                            actions.push(("recipes.recordStop".into(), json!({})));
                        }
                    });
                    ui.label(
                        RichText::new(tl!("Edit as usual: every change is a step. Moving around and selecting are not recorded; the recipe works on whatever is selected when it runs."))
                            .size(11.5)
                            .color(t.text_dim),
                    );
                }
                None => {
                    ui.horizontal(|ui| {
                        let r = ui.button(tl!("● Record New Recipe"));
                        app.auto.add("recipes.record", r.rect, "Record New Recipe");
                        if r.clicked() {
                            app.ui.recipes.record_name.clear();
                            actions.push(("recipes.recordStart".into(), json!({})));
                        }
                    });
                }
            }
            ui.separator();
            ui.horizontal_top(|ui| {
                // the recipes
                ui.vertical(|ui| {
                    ui.set_width(190.0);
                    egui::ScrollArea::vertical().id_salt("recipes-list").max_height(300.0).show(ui, |ui| {
                        for r in &all {
                            let sel = app.ui.recipes.draft.as_ref().is_some_and(|d| d.name == r.name);
                            let label = if r.builtin { format!("{}  ·  {}", crate::i18n::t(&r.name), tl!("built-in")) } else { r.name.clone() };
                            let resp = ui.selectable_label(sel, label);
                            app.auto.add(&format!("recipes.item.{}", r.name), resp.rect, &r.name);
                            if resp.clicked() {
                                app.ui.recipes.draft = Some(r.clone());
                                app.ui.recipes.dirty = false;
                            }
                        }
                    });
                });
                ui.add_space(12.0);
                // the chosen recipe's steps
                ui.vertical(|ui| {
                    let Some(d) = app.ui.recipes.draft.as_mut() else {
                        ui.label(RichText::new(tl!("Choose a recipe.")).color(t.text_dim));
                        return;
                    };
                    ui.label(RichText::new(crate::i18n::t(&d.name)).strong().size(14.0));
                    if !d.description.is_empty() {
                        ui.label(RichText::new(crate::i18n::t(&d.description)).color(t.text_dim).size(11.5));
                    }
                    ui.add_space(4.0);
                    let n = d.steps.len();
                    let mut change: Option<(usize, &str)> = None;
                    egui::ScrollArea::vertical().id_salt("recipes-steps").max_height(250.0).show(ui, |ui| {
                        for (k, st) in d.steps.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                let r = ui.checkbox(&mut st.enabled, format!("{}. {}", k + 1, step_label(&st.command)));
                                app.auto.add(&format!("recipes.step.{k}.enabled"), r.rect, &st.command);
                                if r.changed() {
                                    change = Some((k, "toggled"));
                                }
                                let p = params_text(&st.params);
                                if !p.is_empty() {
                                    ui.label(RichText::new(p).color(t.text_dim).size(11.0));
                                }
                                for (what, glyph, ok) in [("up", "↑", k > 0), ("down", "↓", k + 1 < n), ("remove", "✕", true)] {
                                    let r = ui.add_enabled(ok, egui::Button::new(glyph).small());
                                    app.auto.add(&format!("recipes.step.{k}.{what}"), r.rect, what);
                                    if r.clicked() {
                                        change = Some((k, what));
                                    }
                                }
                            });
                        }
                    });
                    match change {
                        Some((k, "up")) => d.steps.swap(k, k - 1),
                        Some((k, "down")) => d.steps.swap(k, k + 1),
                        Some((k, "remove")) => {
                            d.steps.remove(k);
                        }
                        _ => {}
                    }
                    if change.is_some() {
                        app.ui.recipes.dirty = true;
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                let running = app.session.recipe.as_ref().is_some_and(|r| !r.finished);
                let d = app.ui.recipes.draft.clone();
                let has = d.as_ref().is_some_and(|d| !d.steps.is_empty());
                let r = ui.add_enabled(has && !running && recording.is_none(), crate::dialog_style::primary(tl!("Run")));
                app.auto.add("recipes.run", r.rect, "Run");
                if r.clicked()
                    && let Some(d) = &d
                {
                    actions.push(("recipes.run".into(), json!({"name": d.name, "steps": d.steps})));
                }
                let r = ui.add_enabled(app.ui.recipes.dirty && has, egui::Button::new(tl!("Save Changes")));
                app.auto.add("recipes.save", r.rect, "Save Changes");
                if r.clicked()
                    && let Some(d) = &d
                {
                    actions.push(("recipes.save".into(), json!({"name": d.name, "description": d.description, "steps": d.steps})));
                }
                let user = d.as_ref().is_some_and(|d| !d.builtin);
                let r = ui.add_enabled(user, egui::Button::new(tl!("Delete")));
                app.auto.add("recipes.delete", r.rect, "Delete");
                if r.clicked()
                    && let Some(d) = &d
                {
                    actions.push(("recipes.delete".into(), json!({"name": d.name})));
                }
                if running {
                    ui.spinner();
                }
            });
            if !app.ui.recipes.message.is_empty() {
                ui.label(RichText::new(&app.ui.recipes.message).color(t.text_dim).size(11.5));
            }
        });
    if !open {
        app.ui.recipes.open = false;
    }
    for (cmd, p) in actions {
        match app.session.execute(&cmd, p) {
            Ok(v) => match cmd.as_str() {
                "recipes.run" => {
                    app.ui.recipes.job = v["job"].as_u64();
                    app.ui.recipes.message = tl!("Running…").into();
                }
                "recipes.save" => {
                    app.ui.recipes.dirty = false;
                    if let Some(d) = app.ui.recipes.draft.as_mut() {
                        d.builtin = false;
                    }
                    app.ui.recipes.message = tl!("Saved.").into();
                }
                "recipes.delete" => {
                    app.ui.recipes.draft = filmcraft_engine::recipes::all().into_iter().next();
                    app.ui.recipes.dirty = false;
                    app.ui.recipes.message = tl!("Deleted.").into();
                }
                "recipes.recordStop" if v["name"].is_string() => {
                    let name = v["name"].as_str().unwrap_or_default().to_string();
                    app.ui.recipes.draft = filmcraft_engine::recipes::find(&name);
                    app.ui.recipes.dirty = false;
                    app.ui.recipes.message = tlf!("Recorded {name}.", name);
                }
                _ => {}
            },
            Err(e) => app.ui.recipes.message = e.to_string(),
        }
    }
}

/// "● REC" at the top right while a recipe is being recorded with the window closed.
fn badge(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let n = app.session.recording.as_ref().map_or(0, Vec::len);
    egui::Area::new(egui::Id::new("recipes-rec-badge")).anchor(egui::Align2::RIGHT_TOP, vec2(-16.0, 56.0)).show(ctx, |ui| {
        let r = ui.add(egui::Button::new(RichText::new(tlf!("● REC {n}", n)).color(Color32::WHITE).strong()).fill(Color32::from_rgb(200, 40, 40)));
        app.auto.add("recipes.rec", r.rect, "Recording");
        if r.clicked() {
            app.ui.recipes.open = true;
        }
    });
}
