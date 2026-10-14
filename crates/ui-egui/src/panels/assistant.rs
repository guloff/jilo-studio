//! AI Assistant panel (AI ▸ AI Assistant…, Window ▸ AI Assistant): ask in your own words —
//! "remove the pauses and add yellow karaoke captions" — and get a plan of editor steps
//! (`assistant.plan`, the text engine of Settings ▸ AI). The plan is shown before anything changes:
//! turn steps off, then Run it (as a recipe: one undo step) or Save it as a recipe. The request
//! and a summary of the project go to the cloud after the usual one-time consent per provider;
//! media and transcript text do not.
//!
//! Automation ids: `assistant.input`, `assistant.send`, `assistant.plan.<n>.run`,
//! `assistant.plan.<n>.save`, `assistant.plan.<n>.step.<k>`, `assistant.settings`.

use egui::{Align, Layout, Rect, RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// One message of the conversation.
#[derive(Clone, Debug)]
pub struct ChatEntry {
    pub user: bool,
    pub text: String,
    /// The plan of an answer: `{command, params, enabled, why}`.
    pub steps: Vec<Value>,
    pub rejected: Vec<String>,
    pub ran: bool,
}

/// The panel's state.
#[derive(Clone, Debug, Default)]
pub struct AssistantUi {
    pub input: String,
    pub history: Vec<ChatEntry>,
    /// A request is being planned.
    pub pending: bool,
}

/// Show the panel and, with `request`, send it.
pub fn ask(app: &mut FilmcraftApp, request: Option<String>) -> Result<Value, String> {
    app.show_panel(crate::dock::PanelKind::Assistant);
    if let Some(r) = request.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()) {
        send(app, r);
    }
    Ok(json!({"panel": "Assistant"}))
}

fn send(app: &mut FilmcraftApp, request: String) {
    if app.ui.ai_task.is_some() || app.ui.assistant.pending {
        app.ui.status = tl!("Another AI task is running; wait for it to finish.").into();
        return;
    }
    app.ui.assistant.history.push(ChatEntry { user: true, text: request.clone(), steps: Vec::new(), rejected: Vec::new(), ran: false });
    app.ui.assistant.pending = true;
    crate::panels::ai_tasks::start(app, "assistant.plan", json!({"request": request}), "Planning", "your request and a summary of the project");
}

/// Take a finished plan, or note that the request stopped (refused consent, an error).
fn collect(app: &mut FilmcraftApp) {
    if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "assistant.plan")
        && let Some((_, r)) = app.ui.ai_task_result.take()
    {
        app.ui.assistant.pending = false;
        let steps = r["steps"].as_array().cloned().unwrap_or_default();
        let rejected = r["rejected"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let text = r["reply"].as_str().unwrap_or_default().to_string();
        app.ui.assistant.history.push(ChatEntry { user: false, text, steps, rejected, ran: false });
    } else if app.ui.assistant.pending && app.ui.ai_task.is_none() {
        app.ui.assistant.pending = false;
        let why = if app.ui.status.is_empty() { tl!("The request was not sent.").to_string() } else { app.ui.status.clone() };
        app.ui.assistant.history.push(ChatEntry { user: false, text: why, steps: Vec::new(), rejected: Vec::new(), ran: false });
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    collect(app);
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 8.0))).id_salt("assistant"));
    let ui = &mut child;
    let has_key = app.session.execute("ai.status", json!({})).ok().is_some_and(|s| {
        let p = app.session.prefs.ai.text_provider().id();
        s["providers"].as_array().is_some_and(|a| a.iter().any(|x| x["id"] == p && !x["key"].is_null()))
    });
    let input_h = 74.0;
    let mut run: Option<(usize, bool)> = None;
    let list_h = (ui.available_height() - input_h - 8.0).max(60.0);
    egui::ScrollArea::vertical().id_salt("assistant-chat").max_height(list_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if app.ui.assistant.history.is_empty() {
            ui.label(RichText::new(tl!("Ask in your own words, for example:")).color(t.text_dim));
            for ex in [
                tl!("Remove the pauses and filler words"),
                tl!("Add captions in the Karaoke Yellow style"),
                tl!("Find the best moments for a short"),
                tl!("Zoom in on the loud moments"),
            ] {
                if ui.link(ex).clicked() {
                    app.ui.assistant.input = ex.to_string();
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new(tl!("You see the plan before anything changes; it runs as one undo step.")).size(11.0).color(t.text_faint));
        }
        let n = app.ui.assistant.history.len();
        for i in 0..n {
            let e = app.ui.assistant.history[i].clone();
            if e.user {
                ui.with_layout(Layout::top_down(Align::Max), |ui| {
                    egui::Frame::new().fill(t.pill_active_bg).corner_radius(10.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.set_max_width(ui.available_width() * 0.85);
                        ui.label(RichText::new(&e.text).color(t.text));
                    });
                });
                ui.add_space(6.0);
                continue;
            }
            egui::Frame::new().fill(t.field_bg).corner_radius(10.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !e.text.is_empty() {
                    ui.label(RichText::new(&e.text).color(t.text));
                }
                if !e.steps.is_empty() {
                    ui.add_space(4.0);
                    for k in 0..e.steps.len() {
                        let st = &mut app.ui.assistant.history[i].steps[k];
                        let cmd = st["command"].as_str().unwrap_or_default().to_string();
                        let label = filmcraft_engine::find_command(&cmd).map_or(cmd.clone(), |c| crate::i18n::t(c.label).trim_end_matches('…').to_string());
                        let mut on = st["enabled"].as_bool().unwrap_or(true);
                        let r = ui.add_enabled(!e.ran, egui::Checkbox::new(&mut on, format!("{}. {label}", k + 1)));
                        app.auto.add(&format!("assistant.plan.{i}.step.{k}"), r.rect, &cmd);
                        if r.changed() {
                            st["enabled"] = json!(on);
                        }
                        if let Some(why) = st["why"].as_str().filter(|w| !w.is_empty()) {
                            ui.label(RichText::new(format!("    {why}")).size(11.0).color(t.text_dim));
                        }
                    }
                    if !e.rejected.is_empty() {
                        ui.label(RichText::new(tlf!("Left out (not allowed): {list}", list = e.rejected.join(", "))).size(11.0).color(t.text_faint));
                    }
                    ui.horizontal(|ui| {
                        let r = ui.add_enabled(!e.ran, crate::dialog_style::primary(if e.ran { tl!("Done") } else { tl!("Run") }));
                        app.auto.add(&format!("assistant.plan.{i}.run"), r.rect, "Run");
                        if r.clicked() {
                            run = Some((i, false));
                        }
                        let r = ui.button(tl!("Save as Recipe"));
                        app.auto.add(&format!("assistant.plan.{i}.save"), r.rect, "Save as Recipe");
                        if r.clicked() {
                            run = Some((i, true));
                        }
                    });
                }
            });
            ui.add_space(6.0);
        }
        if app.ui.assistant.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(tl!("Planning…")).color(t.text_dim));
            });
        }
    });
    // the input
    ui.add_space(6.0);
    if !has_key {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tl!("The assistant needs an API key for the text engine.")).size(11.5).color(t.text_dim));
            let r = ui.link(tl!("AI Settings…"));
            app.auto.add("assistant.settings", r.rect, "AI Settings");
            if r.clicked() {
                let _ = crate::panels::settings::open(app, "ai");
            }
        });
    }
    let mut go = false;
    ui.horizontal(|ui| {
        let w = ui.available_width() - 80.0;
        let r = ui.add(egui::TextEdit::multiline(&mut app.ui.assistant.input).desired_rows(2).desired_width(w).hint_text(tl!("What should I do?")));
        app.auto.add("assistant.input", r.rect, "Ask the assistant");
        // Return sends, Shift+Return is a new line
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
            let trimmed = app.ui.assistant.input.trim_end_matches('\n').to_string();
            app.ui.assistant.input = trimmed;
            go = true;
        }
        let b = ui.add_enabled(!app.ui.assistant.pending && !app.ui.assistant.input.trim().is_empty(), crate::dialog_style::primary(tl!("Send")));
        app.auto.add("assistant.send", b.rect, "Send");
        go |= b.clicked();
    });
    if go && !app.ui.assistant.input.trim().is_empty() && !app.ui.assistant.pending {
        let text = std::mem::take(&mut app.ui.assistant.input);
        send(app, text.trim().to_string());
    }
    if let Some((i, save)) = run {
        let e = app.ui.assistant.history[i].clone();
        let request = app.ui.assistant.history[..i].iter().rev().find(|x| x.user).map(|x| x.text.clone()).unwrap_or_default();
        let name: String = request.chars().take(48).collect();
        let steps: Vec<Value> = e.steps.iter().map(|s| json!({"command": s["command"], "params": s["params"], "enabled": s["enabled"]})).collect();
        let (cmd, p) = if save {
            ("recipes.save", json!({"name": format!("{}: {name}", tl!("Assistant")), "description": request, "steps": steps}))
        } else {
            ("recipes.run", json!({"name": format!("{}: {name}", tl!("Assistant")), "steps": steps}))
        };
        match app.session.execute(cmd, p) {
            Ok(v) => {
                if save {
                    app.ui.status = tlf!("Saved as the recipe {name}", name = v["name"].as_str().unwrap_or_default());
                } else {
                    app.ui.assistant.history[i].ran = true;
                    app.ui.recipes.job = v["job"].as_u64();
                }
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    }
}
