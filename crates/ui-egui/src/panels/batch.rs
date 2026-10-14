//! AI ▸ Batch Process… for the sequences and clips selected in the Project panel: which steps to
//! run on each (transcribe, remove fillers, remove pauses, captions in a style, auto zoom, a recipe,
//! a brand kit, highlights, YouTube texts, copies in other formats, export), the speech engine,
//! the language and the export preset. The batch is one job in the status bar and the Progress
//! panel (with Cancel); when it ends, a summary lists every item and the steps that failed
//! (`batch.start`, `batch.status`). The same steps can be kept for the inbox folder: new files
//! there are imported and processed (`inbox.set`).

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The steps as the dialog lists them: (engine step, name, whether it uses the text engine).
const STEPS: [(&str, &str, bool); 11] = [
    ("transcribe", "Transcribe the clips without a transcript", false),
    ("fillers", "Remove filler words (hesitations)", false),
    ("pauses", "Remove silent pauses (by sound)", false),
    ("captions", "Captions from the transcript", false),
    ("autoZoom", "Auto zoom on louder phrases", false),
    ("recipe", "Run a recipe", false),
    ("brandKit", "Apply a brand kit", false),
    ("highlights", "Find highlights", true),
    ("youtube", "YouTube title, description and chapters", true),
    ("formats", "Copies in other formats", false),
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
    if app.session.batch.as_ref().is_some_and(|b| b.to_json()["finished"] != true) {
        return Err("a batch is already running".into());
    }
    // with nothing selected the dialog sets up the inbox folder only
    let inbox = &app.session.prefs.inbox;
    app.ui.batch.inbox_folder = inbox.folder.clone();
    app.ui.batch.inbox_on = inbox.enabled;
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

/// The `batch.start` parameters the dialog's choices make (without `items`).
fn params(b: &crate::state::BatchUi) -> Value {
    let mut p = json!({"confirm": true, "speechEngine": b.speech, "language": b.language});
    for (k, (id, _, _)) in STEPS.iter().enumerate() {
        p[*id] = json!(b.steps[k]);
    }
    if !b.preset.is_empty() {
        p["preset"] = json!(b.preset);
    }
    if !b.caption_preset.is_empty() {
        p["captionPreset"] = json!(b.caption_preset);
    }
    if !b.recipe.is_empty() {
        p["recipeName"] = json!(b.recipe);
    }
    if !b.brand_kit.is_empty() {
        p["brandKitName"] = json!(b.brand_kit);
    }
    let formats: Vec<&str> = crate::panels::formats::ASPECTS.iter().zip(b.formats).filter(|(_, on)| *on).map(|(a, _)| a.0).collect();
    p["aspects"] = json!(formats);
    p
}

/// A step's own choice, as a combo box under it.
fn choice(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, value: &mut String, options: &[(String, String)], none: Option<&str>) {
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        let cur = options
            .iter()
            .find(|o| o.0 == *value)
            .map(|o| crate::i18n::t(&o.1).to_string())
            .or(none.map(|n| crate::i18n::t(n).to_string()))
            .unwrap_or_default();
        let r = egui::ComboBox::from_id_salt(id).width(300.0).height(320.0).selected_text(cur).show_ui(ui, |ui| {
            if let Some(n) = none {
                ui.selectable_value(value, String::new(), crate::i18n::t(n));
            }
            for (k, name) in options {
                ui.selectable_value(value, k.clone(), crate::i18n::t(name));
            }
        });
        app.auto.add(id, r.response.rect, id);
    });
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel, mut keep, mut browse, mut finder) = (false, false, false, false, false);
    let w = 500.0;
    let names: Vec<String> =
        app.ui.batch.items.iter().filter_map(|i| app.session.project.item(filmcraft_project::ItemId(*i)).map(|x| x.name.clone())).collect();
    let presets: Vec<String> = app
        .session
        .execute("export.presets.list", json!({}))
        .ok()
        .and_then(|v| v["presets"].as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default();
    let caption_presets: Vec<(String, String)> = filmcraft_engine::caption_presets::builtin().iter().map(|p| (p.id.to_string(), p.name.to_string())).collect();
    let recipes: Vec<(String, String)> = filmcraft_engine::recipes::all().into_iter().map(|r| (r.name.clone(), r.name)).collect();
    let kits: Vec<(String, String)> = filmcraft_engine::brand_kit::dir()
        .map(|d| filmcraft_engine::brand_kit::list_in(&d))
        .unwrap_or_default()
        .into_iter()
        .map(|k| (k.name.clone(), k.name))
        .collect();
    let text_provider = app.session.prefs.ai.text_provider();
    egui::Modal::new(egui::Id::new("batch-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Batch Process"), w);
        app.auto.add("batch.dialog", bar, "Batch Process");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            if names.is_empty() {
                ui.label(RichText::new(tl!("Nothing is selected in the Project panel: the steps can be kept for the inbox folder.")).color(t.text_dim));
            } else {
                let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
                let more = names.len().saturating_sub(shown.len());
                let list = if more > 0 { format!("{} {}", shown.join(", "), tlf!("and {n} more", n = more)) } else { shown.join(", ") };
                ui.label(RichText::new(tlf!("{n} items: {list}", n = names.len(), list)).color(t.text_dim));
            }
            ui.label(RichText::new(tl!("For each, in this order")).strong());
            egui::ScrollArea::vertical().id_salt("batch-steps").max_height(ctx.content_rect().height() * 0.55).show(ui, |ui| {
                for (k, (id, name, _)) in STEPS.iter().enumerate() {
                    let r = ui.checkbox(&mut app.ui.batch.steps[k], crate::i18n::t(name));
                    app.auto.add(&format!("batch.step.{id}"), r.rect, name);
                    if !app.ui.batch.steps[k] {
                        continue;
                    }
                    // the step's own choices, under it
                    let b = &mut app.ui.batch;
                    match *id {
                        "transcribe" => {
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
                        "captions" => {
                            let mut v = std::mem::take(&mut b.caption_preset);
                            choice(app, ui, "batch.captionPreset", &mut v, &caption_presets, Some("Default style"));
                            app.ui.batch.caption_preset = v;
                        }
                        "recipe" => {
                            let mut v = std::mem::take(&mut b.recipe);
                            if v.is_empty() {
                                v = recipes.first().map(|r| r.0.clone()).unwrap_or_default();
                            }
                            choice(app, ui, "batch.recipe", &mut v, &recipes, None);
                            app.ui.batch.recipe = v;
                        }
                        "brandKit" => {
                            let mut v = std::mem::take(&mut b.brand_kit);
                            if v.is_empty() {
                                v = kits.first().map(|k| k.0.clone()).unwrap_or_default();
                            }
                            if kits.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.add_space(24.0);
                                    ui.label(RichText::new(tl!("No brand kits yet: Graphics and Titles ▸ Brand Kit…")).color(t.hot_text).small());
                                });
                            } else {
                                choice(app, ui, "batch.brandKit", &mut v, &kits, None);
                            }
                            app.ui.batch.brand_kit = v;
                        }
                        "formats" => {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                for (n, (aid, aname, _)) in crate::panels::formats::ASPECTS.iter().enumerate() {
                                    let r = ui.checkbox(&mut b.formats[n], crate::i18n::t(aname));
                                    app.auto.add(&format!("batch.format.{aid}"), r.rect, aname);
                                }
                            });
                        }
                        "export" => {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                let cur =
                                    if b.preset.is_empty() { tl!("The current export settings").to_string() } else { crate::i18n::t(&b.preset).to_string() };
                                let r = egui::ComboBox::from_id_salt("batch.preset").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut b.preset, String::new(), tl!("The current export settings"));
                                    for p in &presets {
                                        ui.selectable_value(&mut b.preset, p.clone(), crate::i18n::t(p));
                                    }
                                });
                                app.auto.add("batch.preset", r.response.rect, "Export preset");
                            });
                        }
                        _ => {}
                    }
                }
            });
            ui.add_space(4.0);
            let b = &mut app.ui.batch;
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
            // the inbox folder
            ui.separator();
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut b.inbox_on, tl!("Inbox folder: new files there get these steps"));
                app.auto.add("batch.inbox", r.rect, "Inbox folder");
            });
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                let b = &app.ui.batch;
                let shown = if b.inbox_folder.is_empty() { tl!("No folder chosen").to_string() } else { b.inbox_folder.clone() };
                ui.add(egui::Label::new(RichText::new(shown).color(t.text_dim)).truncate());
                let r = ui.add_enabled(app.hooks.pick_folder.is_some(), egui::Button::new(tl!("Browse…")).small());
                app.auto.add("batch.inboxFolder", r.rect, "Choose inbox folder");
                browse = r.clicked();
            });
            // the same steps as a Finder Quick Action
            ui.horizontal(|ui| {
                ui.label(tl!("Finder Quick Action"));
                let b = &mut app.ui.batch;
                let r = ui.add(egui::TextEdit::singleline(&mut b.quick_action).hint_text(tl!("Name in Finder")).desired_width(180.0));
                app.auto.add("batch.quickActionName", r.rect, "Quick Action name");
                let ok = !b.quick_action.trim().is_empty() && b.steps.iter().any(|x| *x);
                let r = ui.add_enabled(ok, egui::Button::new(tl!("Add to Finder")).small());
                app.auto.add("batch.quickAction", r.rect, "Add to Finder");
                finder = r.clicked();
            });
            crate::dialog_style::actions(ui, |ui| {
                let b = &app.ui.batch;
                let any = b.steps.iter().any(|x| *x);
                let g = ui.add_enabled(any && !b.items.is_empty() && app.ui.ai_task.is_none(), crate::dialog_style::primary(tl!("Start")));
                app.auto.add("batch.go", g.rect, "Start");
                let can_keep = any && (!b.inbox_on || !b.inbox_folder.is_empty());
                let k = ui.add_enabled(can_keep, crate::dialog_style::secondary(tl!("Keep for Inbox")));
                app.auto.add("batch.keep", k.rect, "Keep for Inbox");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("batch.cancel", c.rect, "Cancel");
                go = g.clicked();
                keep = k.clicked();
                cancel = c.clicked();
            });
        });
    });
    if finder {
        let b = &app.ui.batch;
        let name = b.quick_action.trim().to_string();
        let mut q = json!({"name": name, "params": params(b)});
        if let Some(d) = &b.quick_action_dir {
            q["dir"] = json!(d);
        }
        app.ui.status = match app.session.execute("system.quickAction.install", q) {
            Ok(_) => tlf!("In Finder: right-click files ▸ Quick Actions ▸ {name}", name),
            Err(e) => e.to_string(),
        };
    }
    if browse && let Some(d) = app.hooks.pick_folder.as_mut().and_then(|f| f()) {
        app.ui.batch.inbox_folder = d;
    }
    if keep || go {
        // the inbox keeps what it has unless the switch or the folder changed
        let b = &app.ui.batch;
        let inbox = &app.session.prefs.inbox;
        if keep || b.inbox_on != inbox.enabled || b.inbox_folder != inbox.folder {
            let mut q = json!({"folder": b.inbox_folder, "enabled": b.inbox_on && !b.inbox_folder.is_empty()});
            if keep {
                q["batch"] = params(b);
            }
            match app.session.execute("inbox.set", q) {
                Ok(_) if keep => {
                    app.ui.status = if b.inbox_on {
                        tl!("The inbox folder is watched: new files get these steps.").to_string()
                    } else {
                        tl!("The steps are kept for the inbox folder.").to_string()
                    }
                }
                Ok(_) => {}
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        app.ui.batch.dialog = false;
    }
    if go {
        let b = &app.ui.batch;
        let mut p = params(b);
        p["items"] = json!(b.items);
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
