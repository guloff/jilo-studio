//! AI ▸ Batch Process… for the sequences and clips selected in the Project panel: which steps to
//! run on each (transcribe, remove fillers, remove pauses, auto zoom, highlights, YouTube texts,
//! export), the speech engine, the language and the export preset. The batch is one job in the
//! status bar and the Progress panel (with Cancel); when it ends, a summary lists every item and
//! the steps that failed (`batch.start`, `batch.status`).

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The steps as the dialog lists them: (engine step, name, whether it uses the text engine).
const STEPS: [(&str, &str, bool); 7] = [
    ("transcribe", "Transcribe the clips without a transcript", false),
    ("fillers", "Remove filler words (hesitations)", false),
    ("pauses", "Remove silent pauses (by sound)", false),
    ("autoZoom", "Auto zoom on louder phrases", false),
    ("highlights", "Find highlights", true),
    ("youtube", "YouTube title, description and chapters", true),
    ("export", "Export each sequence", false),
];

const SPEECH: [(&str, &str); 4] = [
    ("whisper", "Whisper on this computer (offline)"),
    ("gemini", "Google Gemini (cloud)"),
    ("openai", "OpenAI (cloud)"),
    ("elevenlabs", "ElevenLabs Scribe (cloud)"),
];
const LANGS: [(&str, &str); 4] = [("auto", "Auto-detect"), ("ru", "Russian"), ("tg", "Tajik"), ("en", "English")];

/// Open the dialog for the Project panel's selection (sequences and clips).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    let p = &app.session.project;
    let items: Vec<u64> = app
        .session
        .state
        .project_selection
        .iter()
        .filter(|i| {
            matches!(
                p.item(**i).map(|x| &x.kind),
                Some(filmcraft_project::ItemKind::Sequence(_) | filmcraft_project::ItemKind::Media(_) | filmcraft_project::ItemKind::Subclip { .. })
            )
        })
        .map(|i| i.0)
        .collect();
    if items.is_empty() {
        return Err("select sequences or clips in the Project panel first".into());
    }
    if app.session.batch.as_ref().is_some_and(|b| b.to_json()["finished"] != true) {
        return Err("a batch is already running".into());
    }
    app.ui.batch.items = items;
    app.ui.batch.dialog = true;
    Ok(json!({"dialog": "batch"}))
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // the summary when the watched batch ends
    if let Some(id) = app.ui.batch.job
        && app.session.jobs.iter().find(|j| j.id == id).is_some_and(|j| j.to_json()["finished"] == true)
    {
        app.ui.batch.job = None;
        app.ui.batch.summary = app.session.execute("batch.status", json!({})).ok();
    }
    if app.ui.batch.dialog {
        dialog(app, ctx);
    }
    if app.ui.batch.summary.is_some() {
        summary(app, ctx);
    }
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 480.0;
    let names: Vec<String> =
        app.ui.batch.items.iter().filter_map(|i| app.session.project.item(filmcraft_project::ItemId(*i)).map(|x| x.name.clone())).collect();
    let presets: Vec<String> = app
        .session
        .execute("export.presets.list", json!({}))
        .ok()
        .and_then(|v| v["presets"].as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default();
    let text_provider = app.session.prefs.ai.text_provider();
    egui::Modal::new(egui::Id::new("batch-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Batch Process"), w);
        app.auto.add("batch.dialog", bar, "Batch Process");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
            let more = names.len().saturating_sub(shown.len());
            let list = if more > 0 { format!("{} {}", shown.join(", "), tlf!("and {n} more", n = more)) } else { shown.join(", ") };
            ui.label(RichText::new(tlf!("{n} items: {list}", n = names.len(), list)).color(t.text_dim));
            ui.label(RichText::new(tl!("For each, in this order")).strong());
            let b = &mut app.ui.batch;
            for (k, (id, name, _)) in STEPS.iter().enumerate() {
                let r = ui.checkbox(&mut b.steps[k], crate::i18n::t(name));
                app.auto.add(&format!("batch.step.{id}"), r.rect, name);
                // the step's own choices, under it
                if *id == "transcribe" && b.steps[k] {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        let cur = SPEECH.iter().find(|s| s.0 == b.speech).map_or("", |s| s.1);
                        let r = egui::ComboBox::from_id_salt("batch.speech").width(230.0).selected_text(crate::i18n::t(cur)).show_ui(ui, |ui| {
                            for (sid, sname) in SPEECH {
                                ui.selectable_value(&mut b.speech, sid.to_string(), crate::i18n::t(sname));
                            }
                        });
                        app.auto.add("batch.speech", r.response.rect, "Speech engine");
                        let cur = LANGS.iter().find(|l| l.0 == b.language).map_or("", |l| l.1);
                        let r = egui::ComboBox::from_id_salt("batch.language").width(120.0).selected_text(crate::i18n::t(cur)).show_ui(ui, |ui| {
                            for (lid, lname) in LANGS {
                                ui.selectable_value(&mut b.language, lid.to_string(), crate::i18n::t(lname));
                            }
                        });
                        app.auto.add("batch.language", r.response.rect, "Language");
                    });
                }
                if *id == "export" && b.steps[k] {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        let cur = if b.preset.is_empty() { tl!("The current export settings").to_string() } else { crate::i18n::t(&b.preset).to_string() };
                        let r = egui::ComboBox::from_id_salt("batch.preset").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                            ui.selectable_value(&mut b.preset, String::new(), tl!("The current export settings"));
                            for p in &presets {
                                ui.selectable_value(&mut b.preset, p.clone(), crate::i18n::t(p));
                            }
                        });
                        app.auto.add("batch.preset", r.response.rect, "Export preset");
                    });
                }
            }
            ui.add_space(4.0);
            // what goes to the cloud, said before the explicit Start
            let speech_cloud = b.steps[0] && b.speech != "whisper";
            let text_cloud = STEPS.iter().enumerate().any(|(k, s)| s.2 && b.steps[k]);
            let mut notes: Vec<String> = Vec::new();
            if speech_cloud {
                let name = filmcraft_ai::Provider::from_id(&b.speech).map_or("", |p| p.name());
                notes.push(tlf!("the clips' audio goes to {provider}", provider = name));
            }
            if text_cloud {
                notes.push(tlf!("the transcripts go to {provider}", provider = text_provider.name()));
            }
            let note = if notes.is_empty() {
                tl!("Nothing is sent: every chosen step runs on this computer. Each change can be undone.").to_string()
            } else {
                tlf!("Starting the batch agrees that {what}. Each change can be undone.", what = notes.join("; "))
            };
            ui.label(RichText::new(note).color(t.text_dim).small());
            crate::dialog_style::actions(ui, |ui| {
                let any = b.steps.iter().any(|x| *x);
                let g = ui.add_enabled(any && app.ui.ai_task.is_none(), crate::dialog_style::primary(tl!("Start")));
                app.auto.add("batch.go", g.rect, "Start");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("batch.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let b = &app.ui.batch;
        let mut p = json!({"items": b.items, "confirm": true, "speechEngine": b.speech, "language": b.language});
        for (k, (id, _, _)) in STEPS.iter().enumerate() {
            p[*id] = json!(b.steps[k]);
        }
        if !b.preset.is_empty() {
            p["preset"] = json!(b.preset);
        }
        app.ui.batch.dialog = false;
        match app.session.execute("batch.start", p) {
            Ok(r) => {
                app.ui.batch.job = r["job"].as_u64();
                app.ui.status = tlf!("Batch started: {n} items", n = app.ui.batch.items.len());
            }
            Err(e) => app.ui.status = e.to_string(),
        }
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.batch.dialog = false;
    }
}

fn summary(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let Some(v) = app.ui.batch.summary.clone() else { return };
    let mut open = true;
    let mut close = false;
    let steps: std::collections::HashMap<&str, &str> = STEPS.iter().map(|(id, name, _)| (*id, *name)).collect();
    crate::dialog_style::Window::new(tl!("Batch Results"))
        .id(egui::Id::new("batch-summary"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(460.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let items = v["items"].as_array().cloned().unwrap_or_default();
            let done = items.iter().filter(|i| i["status"] == "done").count();
            ui.label(RichText::new(tlf!("Done: {done} of {n}", done, n = items.len())).strong());
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                for (i, it) in items.iter().enumerate() {
                    let status = it["status"].as_str().unwrap_or_default();
                    let mark = match status {
                        "done" => tl!("done"),
                        "cancelled" => tl!("cancelled"),
                        _ => tl!("with errors"),
                    };
                    let r = ui.horizontal(|ui| {
                        ui.label(RichText::new(it["name"].as_str().unwrap_or_default()).strong());
                        ui.label(RichText::new(mark).color(if status == "done" { t.text_dim } else { t.hot_text }).small());
                    });
                    app.auto.add(&format!("batch.result.{i}"), r.response.rect, it["name"].as_str().unwrap_or_default());
                    for e in it["errors"].as_array().into_iter().flatten() {
                        let step = e["step"].as_str().map_or("", |s| steps.get(s).copied().unwrap_or(s));
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{}: {}", crate::i18n::t(step), e["error"].as_str().unwrap_or_default())).color(t.text_dim).small(),
                                )
                                .wrap(),
                            );
                        });
                    }
                    ui.add_space(2.0);
                }
            });
            ui.add_space(4.0);
            crate::dialog_style::actions(ui, |ui| {
                let c = ui.add(crate::dialog_style::primary(tl!("Close")));
                app.auto.add("batch.summary.close", c.rect, "Close");
                close = c.clicked();
            });
            ui.add_space(4.0);
            ui.allocate_space(vec2(0.0, 0.0));
        });
    if !open || close || crate::widgets::escape_closes(ctx) {
        app.ui.batch.summary = None;
    }
}
