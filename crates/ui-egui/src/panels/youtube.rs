//! AI ▸ YouTube Title, Description and Chapters… (also the Text panel's Transcript toolbar):
//! a window with the texts kept with the sequence — title options, the description (editable), the
//! chapters as YouTube will list them (from the sequence's chapter markers), tags and hashtags —
//! with Copy for each field and for everything. Without texts yet (or Write Again) it shows the form
//! that writes them: `youtube.generate` as a cloud task (consent, status bar, Progress panel).

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// Languages offered for the texts ("" = the transcript's).
const LANGUAGES: [(&str, &str); 7] =
    [("", "Same as the transcript"), ("ru", "Russian"), ("tg", "Tajik"), ("en", "English"), ("uz", "Uzbek"), ("fa", "Persian"), ("es", "Spanish")];

/// Open the window: the kept texts, or the form (refused, with the reason, without a sequence).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    if app.session.active_sequence().is_none() {
        return Err("no sequence is open".into());
    }
    let data = app.session.execute("youtube.texts", json!({})).ok();
    let y = &mut app.ui.youtube;
    load(y, data);
    y.open = true;
    y.rewrite = false;
    Ok(json!({"window": "youtube"}))
}

fn load(y: &mut crate::state::YouTubeUi, data: Option<Value>) {
    if let Some(d) = &data {
        y.text = d["text"].as_str().unwrap_or_default().to_string();
        y.tags = d["tagsText"].as_str().unwrap_or_default().to_string();
        y.hashtags = d["hashtags"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        y.title = 0;
    }
    y.data = data;
}

/// The window, and taking over the texts a finished cloud task wrote.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "youtube.generate")
        && let Some((_, r)) = app.ui.ai_task_result.take()
    {
        let y = &mut app.ui.youtube;
        load(y, Some(r));
        y.open = true;
        y.rewrite = false;
    }
    if !app.ui.youtube.open {
        return;
    }
    let mut open = true;
    crate::dialog_style::Window::new(tl!("YouTube"))
        .id(egui::Id::new("youtube-window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .default_height(560.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            if app.ui.youtube.data.is_none() || app.ui.youtube.rewrite {
                form(app, ui);
            } else {
                texts(app, ui);
            }
        });
    if !open || crate::widgets::escape_closes(ctx) && !ctx.egui_wants_keyboard_input() {
        save(app);
        app.ui.youtube.open = false;
    }
}

/// Save the edited fields when they differ from the kept texts.
fn save(app: &mut FilmcraftApp) {
    let y = &app.ui.youtube;
    let Some(d) = &y.data else { return };
    let hashtags = d["hashtags"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
    if y.text == d["text"].as_str().unwrap_or_default() && y.tags == d["tagsText"].as_str().unwrap_or_default() && y.hashtags == hashtags {
        return;
    }
    let params = json!({"text": y.text, "tags": y.tags, "hashtags": y.hashtags});
    match app.session.execute("youtube.set", params) {
        Ok(r) => load(&mut app.ui.youtube, Some(r)),
        Err(e) => app.ui.status = e.to_string(),
    }
}

fn form(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    let provider = app.session.prefs.ai.text_provider();
    let chapters = app.session.active_sequence().map_or(0, |q| q.markers.iter().filter(|m| m.kind == filmcraft_project::MarkerKind::Chapter).count());
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.label(RichText::new(tl!("Three title options, a description, chapters, tags and hashtags, written from the transcript.")).color(t.text_dim));
    let y = &mut app.ui.youtube;
    ui.horizontal(|ui| {
        ui.label(tl!("Language"));
        let cur = LANGUAGES.iter().find(|l| l.0 == y.language).map_or("", |l| l.1);
        let mut items: Vec<(String, egui::Rect, &str)> = Vec::new();
        let r = egui::ComboBox::from_id_salt("youtube.language").width(200.0).selected_text(crate::i18n::t(cur)).show_ui(ui, |ui| {
            for (code, name) in LANGUAGES {
                let r = ui.selectable_label(y.language == code, crate::i18n::t(name));
                items.push((format!("youtube.language.{}", if code.is_empty() { "auto" } else { code }), r.rect, name));
                if r.clicked() {
                    y.language = code.to_string();
                }
            }
        });
        app.auto.add("youtube.language", r.response.rect, "Language");
        for (id, rect, l) in items {
            app.auto.add(&id, rect, l);
        }
    });
    if chapters > 0 {
        let mut replace = !y.keep_chapters;
        let r = ui.checkbox(&mut replace, tlf!("Replace the chapter markers ({n})", n = chapters));
        y.keep_chapters = !replace;
        app.auto.add("youtube.replaceChapters", r.rect, "Replace the chapter markers");
    }
    ui.label(
        RichText::new(tlf!("The transcript goes to {provider}. The chapters become chapter markers: move them to move a chapter.", provider = provider.name()))
            .color(t.text_dim)
            .small(),
    );
    let (mut go, mut back) = (false, false);
    crate::dialog_style::actions(ui, |ui| {
        let g = ui.add_enabled(app.ui.ai_task.is_none(), crate::dialog_style::primary(tl!("Write Texts")));
        app.auto.add("youtube.generate", g.rect, "Write Texts");
        go = g.clicked();
        if app.ui.youtube.data.is_some() {
            let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
            app.auto.add("youtube.cancel", c.rect, "Cancel");
            back = c.clicked();
        }
    });
    ui.add_space(4.0);
    if back {
        app.ui.youtube.rewrite = false;
    }
    if go {
        let y = &app.ui.youtube;
        let mut params = json!({"replaceChapters": !y.keep_chapters});
        if !y.language.is_empty() {
            params["language"] = json!(y.language);
        }
        app.ui.youtube.open = false;
        crate::panels::ai_tasks::start(app, "youtube.generate", params, "Writing YouTube texts", "the transcript");
    }
}

/// A field heading with a Copy button on the right.
fn heading(app: &mut FilmcraftApp, ui: &mut egui::Ui, label: &str, id: &str, copy: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.small_button(tl!("Copy"));
            app.auto.add(&format!("youtube.copy.{id}"), r.rect, "Copy");
            if r.clicked() {
                ui.ctx().copy_text(copy.to_string());
                app.ui.status = tlf!("Copied: {what}", what = label);
            }
        });
    });
}

fn texts(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    let Some(d) = app.ui.youtube.data.clone() else { return };
    let titles: Vec<String> = d["titles"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    let footer = 40.0;
    egui::ScrollArea::vertical().max_height((ui.available_height() - footer).max(160.0)).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        // titles: choose one (it leads Copy All), copy any
        let chosen = titles.get(app.ui.youtube.title).cloned().unwrap_or_default();
        heading(app, ui, tl!("Title"), "title", &chosen);
        for (i, title) in titles.iter().enumerate() {
            ui.horizontal(|ui| {
                let r = ui.radio_value(&mut app.ui.youtube.title, i, title);
                app.auto.add(&format!("youtube.title.{i}"), r.rect, title);
                ui.label(RichText::new(format!("{}/100", title.chars().count())).color(t.text_dim).small());
            });
        }
        ui.add_space(4.0);
        let description = d["description"].as_str().unwrap_or_default().to_string();
        heading(app, ui, tl!("Description"), "description", &description);
        let r = ui.add(egui::TextEdit::multiline(&mut app.ui.youtube.text).desired_rows(6).desired_width(f32::INFINITY));
        app.auto.add("youtube.text", r.rect, "Description");
        if r.lost_focus() {
            save(app);
        }
        ui.label(RichText::new(tl!("Copy adds the chapters and hashtags below to the text.")).color(t.text_dim).small());
        // chapters as YouTube will list them
        ui.add_space(4.0);
        ui.label(RichText::new(tl!("Chapters")).strong());
        let problems: Vec<&str> = d["problems"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        egui::Grid::new("youtube-chapters").num_columns(2).spacing(vec2(12.0, 2.0)).show(ui, |ui| {
            for c in d["chapters"].as_array().into_iter().flatten() {
                ui.label(RichText::new(c["label"].as_str().unwrap_or_default()).monospace().color(t.text_dim));
                ui.label(c["title"].as_str().unwrap_or_default());
                ui.end_row();
            }
        });
        if let Some(p) = problems.first() {
            let why = if *p == "videoTooShort" {
                tl!("YouTube shows chapters only in videos of at least 30 seconds.")
            } else {
                tl!("YouTube shows chapters from three, each at least 10 seconds long: add chapter markers (Markers ▸ Add Chapter Marker).")
            };
            ui.label(RichText::new(why).color(t.hot_text).small());
        } else {
            ui.label(RichText::new(tl!("From the sequence's chapter markers: move one to move its chapter.")).color(t.text_dim).small());
        }
        ui.add_space(4.0);
        let tags = app.ui.youtube.tags.clone();
        heading(app, ui, tl!("Tags"), "tags", &tags);
        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.youtube.tags).desired_width(f32::INFINITY));
        app.auto.add("youtube.tags", r.rect, "Tags");
        if r.lost_focus() {
            save(app);
        }
        let n = d["tagsText"].as_str().map_or(0, |x| x.chars().count());
        ui.label(RichText::new(tlf!("Separated by commas · {n}/500", n)).color(t.text_dim).small());
        ui.add_space(4.0);
        let hashtags = app.ui.youtube.hashtags.clone();
        heading(app, ui, tl!("Hashtags"), "hashtags", &hashtags);
        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.youtube.hashtags).desired_width(f32::INFINITY));
        app.auto.add("youtube.hashtags", r.rect, "Hashtags");
        if r.lost_focus() {
            save(app);
        }
    });
    ui.add_space(6.0);
    let (mut all, mut again) = (false, false);
    crate::dialog_style::actions(ui, |ui| {
        let a = ui.add(crate::dialog_style::primary(tl!("Copy All")));
        app.auto.add("youtube.copyAll", a.rect, "Copy All");
        let w = ui.add_enabled(app.ui.ai_task.is_none(), crate::dialog_style::secondary(tl!("Write Again…")));
        app.auto.add("youtube.rewrite", w.rect, "Write Again…");
        all = a.clicked();
        again = w.clicked();
    });
    ui.add_space(4.0);
    if all {
        save(app);
        let d = app.ui.youtube.data.clone().unwrap_or_default();
        let title = d["titles"].get(app.ui.youtube.title).and_then(Value::as_str).unwrap_or_default().to_string();
        let text = [title, d["description"].as_str().unwrap_or_default().to_string(), d["tagsText"].as_str().unwrap_or_default().to_string()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        ui.ctx().copy_text(text);
        app.ui.status = tl!("Copied: the title, the description with chapters and the tags").to_string();
    }
    if again {
        save(app);
        app.ui.youtube.rewrite = true;
        app.ui.youtube.language = d["language"].as_str().unwrap_or_default().to_string();
    }
}
