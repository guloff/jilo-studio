//! "What do you want to make?" — the start from a video. Files dropped on the home screen, or
//! chosen with its "Start from a Video…" card, are imported and offered four ways to go on: a
//! vertical short with captions, a cleaned-up talking head, captions only, or just editing. The
//! first three run as a batch in the background (`batch.start`: transcription on this computer,
//! filler words and pauses out, captions in a style, a 9:16 copy) and open the result when it is
//! done; "Just edit" makes a sequence from the clips and opens it at once.
//!
//! Automation ids: `start.dialog`, `start.choice.<id>`, `start.cancel`.

use egui::{RichText, Sense, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The ways to go on: (id, name, what happens).
pub const CHOICES: [(&str, &str, &str); 4] = [
    ("short", "Vertical Short with Captions", "9:16, the speaker in frame, pauses out, bold captions word by word."),
    ("talk", "Clean Talking Head", "Filler words and pauses out, captions added; the frame stays as shot."),
    ("captions", "Just Captions", "The speech transcribed and captioned, nothing cut."),
    ("edit", "Just Edit", "A sequence from the clips, open on the timeline."),
];

#[derive(Clone, Debug, Default)]
pub struct StartUi {
    /// The imported clips the dialog is for (empty: closed).
    pub items: Vec<u64>,
    /// The batch running for the choice, and whether to open its 9:16 copy.
    pub job: Option<(u64, bool)>,
    /// Whether Whisper is ready, when known (tests); None: looked up.
    pub speech_ready: Option<bool>,
}

/// Show the dialog for imported `items` (media only).
pub fn offer(app: &mut FilmcraftApp, items: Vec<u64>) {
    let media: Vec<u64> = items
        .into_iter()
        .filter(|i| {
            app.session
                .project
                .item(filmcraft_project::ItemId(*i))
                .is_some_and(|x| matches!(x.kind, filmcraft_project::ItemKind::Media(_)) && (x.has_video() || x.has_audio()))
        })
        .collect();
    if !media.is_empty() {
        app.ui.start.items = media;
    }
}

/// The "Start from a Video…" card: pick files, import them, offer the choices.
pub fn pick(app: &mut FilmcraftApp) {
    let exts: Vec<&str> = filmcraft_media::VIDEO_EXTENSIONS.iter().chain(filmcraft_media::AUDIO_EXTENSIONS).copied().collect();
    let paths = app.hooks.pick_files.as_mut().map(|f| f(&exts)).unwrap_or_default();
    if paths.is_empty() {
        return;
    }
    let bin = app.import_bin().0;
    match app.session.execute("file.import", json!({"paths": paths, "bin": bin})) {
        Ok(v) => {
            let mut items: Vec<u64> = v["items"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
            items.extend(v["duplicates"].as_array().into_iter().flatten().filter_map(|d| d["item"].as_u64()));
            offer(app, items);
        }
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// The batch parameters of a choice (None: just edit).
pub fn steps(choice: &str) -> Option<Value> {
    let base = json!({"transcribe": true, "speechEngine": "whisper", "language": "auto", "captions": true});
    let mut p = base;
    match choice {
        "short" => {
            p["pauses"] = json!(true);
            p["captionPreset"] = json!("karaoke-yellow");
            p["formats"] = json!(true);
            p["aspects"] = json!(["9:16"]);
            p["follow"] = json!("face");
        }
        "talk" => {
            p["fillers"] = json!(true);
            p["pauses"] = json!(true);
            p["captionPreset"] = json!("clean-shadow");
        }
        "captions" => p["captionPreset"] = json!("clean-shadow"),
        _ => return None,
    }
    Some(p)
}

fn choose(app: &mut FilmcraftApp, choice: &str) {
    let items = std::mem::take(&mut app.ui.start.items);
    match steps(choice) {
        None => match app.session.execute("file.newSequenceFromClip", json!({"items": items})) {
            Ok(r) => {
                if let Some(q) = r["sequence"].as_u64() {
                    let _ = app.session.execute("sequence.open", json!({"item": q}));
                }
                app.ui.mode = crate::state::Mode::Edit;
            }
            Err(e) => app.ui.status = e.to_string(),
        },
        Some(mut p) => {
            p["items"] = json!(items);
            match app.session.execute("batch.start", p) {
                Ok(r) => {
                    app.ui.start.job = r["job"].as_u64().map(|j| (j, choice == "short"));
                    app.ui.mode = crate::state::Mode::Edit;
                    app.ui.status = tl!("Working on it in the background: the result opens when it is ready.").to_string();
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // the batch of a choice: open what it made
    if let Some((job, vertical)) = app.ui.start.job
        && app.session.jobs.iter().find(|j| j.id == job).is_none_or(|j| j.to_json()["finished"] == true)
    {
        app.ui.start.job = None;
        if let Ok(st) = app.session.execute("batch.status", json!({})) {
            let item = &st["items"][0];
            let seq = if vertical { item["formats"][0].as_u64().or(item["sequence"].as_u64()) } else { item["sequence"].as_u64() };
            if let Some(q) = seq {
                let _ = app.session.execute("sequence.open", json!({"item": q}));
            }
            let errors: Vec<String> = item["errors"].as_array().into_iter().flatten().filter_map(|e| e["error"].as_str().map(str::to_string)).collect();
            app.ui.status = if errors.is_empty() {
                tl!("Ready: the result is open on the timeline.").to_string()
            } else {
                tlf!("Done, with problems: {errors}", errors = errors.join("; "))
            };
        }
    }
    if app.ui.start.items.is_empty() {
        return;
    }
    let t = app.tokens;
    let w = 520.0;
    let ready = app.ui.start.speech_ready.unwrap_or_else(filmcraft_engine::transcript::whisper_ready);
    let mut chosen: Option<&str> = None;
    let mut cancel = false;
    let n = app.ui.start.items.len();
    egui::Modal::new(egui::Id::new("start-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("What Do You Want to Make?"), w);
        app.auto.add("start.dialog", bar, "What Do You Want to Make?");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(tlf!("{n} clips imported.", n)).color(t.text_dim));
            for (id, name, about) in CHOICES {
                let needs_speech = id != "edit";
                let enabled = ready || !needs_speech;
                let (rect, resp) = ui.allocate_exact_size(vec2(w - 36.0, 58.0), if enabled { Sense::click() } else { Sense::hover() });
                let p = ui.painter();
                p.rect_filled(rect, 6.0, if resp.hovered() && enabled { t.hover } else { t.field_bg });
                p.text(
                    rect.min + vec2(12.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    crate::i18n::t(name),
                    egui::FontId::proportional(14.0),
                    if enabled { t.text } else { t.text_dim },
                );
                let sub = if enabled { crate::i18n::t(about).to_string() } else { tl!("Needs a Whisper model: download one in AI Settings.").to_string() };
                p.text(rect.min + vec2(12.0, 32.0), egui::Align2::LEFT_TOP, sub, egui::FontId::proportional(11.5), t.text_dim);
                app.auto.add(&format!("start.choice.{id}"), rect, name);
                if resp.clicked() && enabled {
                    chosen = Some(id);
                }
            }
            ui.label(
                RichText::new(tl!("Everything runs on this computer and can be undone; the clips stay in the Project panel either way."))
                    .color(t.text_dim)
                    .small(),
            );
            crate::dialog_style::actions(ui, |ui| {
                if !ready {
                    let s = ui.add(crate::dialog_style::secondary(tl!("AI Settings…")));
                    if s.clicked() {
                        let _ = crate::panels::settings::open(app, "ai");
                    }
                }
                let c = ui.add(crate::dialog_style::secondary(tl!("Not Now")));
                app.auto.add("start.cancel", c.rect, "Not Now");
                cancel = c.clicked();
            });
        });
    });
    if let Some(c) = chosen {
        choose(app, c);
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.start.items.clear();
    }
}
