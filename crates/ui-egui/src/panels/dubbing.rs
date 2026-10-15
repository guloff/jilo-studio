//! AI ▸ Dub…: the speech in another language. The captions are translated by the text engine
//! (`captions.translate`, a cloud task with the usual consent) unless a caption track in that
//! language is there already, then spoken by the chosen voice of Text to Speech in a job
//! (`audio.dub`, progress and Stop), each phrase at its caption's time on a new "Dub" track, the
//! original voice turned down or muted.
//!
//! Automation ids: `dub.dialog`, `dub.language`, `dub.voice`, `dub.original.<quiet|mute|keep>`,
//! `dub.go`, `dub.cancel`.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Debug)]
pub struct DubUi {
    pub dialog: bool,
    pub language: String,
    pub voice: String,
    /// `quiet` (−20 dB), `mute` or `keep`.
    pub original: String,
    /// Waiting for the translation (true) and then for the speech job: (job, `audio.dub` parameters).
    pub translating: bool,
    pub job: Option<(u64, Value)>,
}

impl Default for DubUi {
    fn default() -> Self {
        Self { dialog: false, language: "ru".into(), voice: String::new(), original: "quiet".into(), translating: false, job: None }
    }
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    let has = app.session.active_sequence().is_some_and(|q| q.caption_tracks.iter().any(|t| !t.captions.is_empty()));
    if !has {
        return Err(tl!("Make captions first: AI ▸ Captions from the Transcript.").to_string());
    }
    app.ui.dub.dialog = true;
    Ok(json!({"dialog": "dub"}))
}

/// The caption track already in `lang` (C1, C2… index from 1).
fn track_in(app: &FilmcraftApp, lang: &str) -> Option<usize> {
    app.session.active_sequence()?.caption_tracks.iter().position(|t| t.language == lang && !t.captions.is_empty()).map(|i| i + 1)
}

/// Speak caption track `index` (from 1): in a job, then put in.
fn speak(app: &mut FilmcraftApp, index: usize) {
    let u = &app.ui.dub;
    let db = match u.original.as_str() {
        "mute" => -96.0,
        "keep" => 0.0,
        _ => -20.0,
    };
    let mut p = json!({"captions": format!("C{index}"), "language": u.language, "originalDb": db, "confirm": true});
    if !u.voice.is_empty() {
        p["voice"] = json!(u.voice);
    }
    let mut bg = p.clone();
    bg["background"] = json!(true);
    match app.session.execute("audio.dub", bg) {
        Ok(r) if r["background"] == true => {
            app.ui.dub.job = r["job"].as_u64().map(|j| (j, p));
            app.ui.status = tl!("Dubbing: speaking the phrases…").to_string();
        }
        Ok(_) => put_in(app, p),
        Err(e) => app.ui.status = e.to_string(),
    }
}

fn put_in(app: &mut FilmcraftApp, p: Value) {
    app.ui.status = match app.session.execute("audio.dub", p) {
        Ok(r) => tlf!("Dubbed: {n} phrases on a new track.", n = r["clips"].as_array().map_or(0, Vec::len)),
        Err(e) => e.to_string(),
    };
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // the translation arrived: speak it
    if app.ui.dub.translating {
        if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "captions.translate")
            && let Some((_, r)) = app.ui.ai_task_result.take()
        {
            app.ui.dub.translating = false;
            let id = r["track"].as_u64();
            let index = app.session.active_sequence().and_then(|q| q.caption_tracks.iter().position(|t| Some(t.id.0) == id)).map(|i| i + 1);
            match index {
                Some(i) => speak(app, i),
                None => app.ui.status = tl!("The translation did not make a caption track.").to_string(),
            }
        } else if app.ui.ai_task.is_none() {
            app.ui.dub.translating = false;
        }
    }
    if let Some((id, p)) = app.ui.dub.job.clone() {
        match app.session.jobs.iter().find(|j| j.id == id).map(|j| j.to_json()) {
            Some(j) if j["finished"] != true => {}
            Some(j) if j["result"]["error"].is_string() => {
                app.ui.dub.job = None;
                app.ui.status = j["result"]["error"].as_str().unwrap_or_default().to_string();
            }
            _ => {
                app.ui.dub.job = None;
                put_in(app, p);
            }
        }
    }
    if !app.ui.dub.dialog {
        return;
    }
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 460.0;
    let voices = app.session.execute("tts.voices", json!({})).unwrap_or_default();
    let mut list: Vec<(String, String)> = voices["voices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["installed"] != false)
        .filter_map(|v| Some((v["id"].as_str()?.to_string(), format!("{} · {}", v["name"].as_str()?, tl!("this computer")))))
        .collect();
    list.extend(
        voices["cloud"]["voices"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["id"].as_str()?.to_string(), format!("{} · ElevenLabs", v["name"].as_str()?)))),
    );
    let text_provider = app.session.prefs.ai.text_provider();
    egui::Modal::new(egui::Id::new("dub-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Dub"), w);
        app.auto.add("dub.dialog", bar, "Dub");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(tl!("The captions translated and spoken by a voice, each phrase at its time, on a new audio track.")).color(t.text_dim));
            let u = &mut app.ui.dub;
            egui::Grid::new("dub-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Into"));
                let cur = crate::panels::text::TRANSLATE_LANGUAGES.iter().find(|l| l.0 == u.language).map_or(u.language.clone(), |l| crate::i18n::t(l.1).to_string());
                let r = egui::ComboBox::from_id_salt("dub-language").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                    for (code, name) in crate::panels::text::TRANSLATE_LANGUAGES {
                        ui.selectable_value(&mut u.language, code.to_string(), crate::i18n::t(name));
                    }
                });
                app.auto.add("dub.language", r.response.rect, "Into");
                ui.end_row();
                ui.label(tl!("Voice"));
                let cur = list.iter().find(|v| v.0 == u.voice).map_or_else(|| tl!("Default voice").to_string(), |v| v.1.clone());
                let r = egui::ComboBox::from_id_salt("dub-voice").selected_text(cur).width(260.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut u.voice, String::new(), tl!("Default voice"));
                    for (id, name) in &list {
                        ui.selectable_value(&mut u.voice, id.clone(), name);
                    }
                });
                app.auto.add("dub.voice", r.response.rect, "Voice");
                ui.end_row();
                ui.label(tl!("Original voice"));
                ui.horizontal(|ui| {
                    for (id, name) in [("quiet", tl!("Quieter")), ("mute", tl!("Muted")), ("keep", tl!("As is"))] {
                        let r = ui.radio_value(&mut u.original, id.to_string(), name);
                        app.auto.add(&format!("dub.original.{id}"), r.rect, name);
                    }
                });
                ui.end_row();
            });
            ui.label(
                RichText::new(tlf!(
                    "The caption text goes to {provider} to be translated; an ElevenLabs voice speaks in the cloud, the natural voices (English) on this computer.",
                    provider = text_provider.name()
                ))
                .color(t.text_dim)
                .small(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add_enabled(app.ui.ai_task.is_none() && app.ui.dub.job.is_none(), crate::dialog_style::primary(tl!("Dub")));
                app.auto.add("dub.go", g.rect, "Dub");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("dub.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        app.ui.dub.dialog = false;
        let lang = app.ui.dub.language.clone();
        match track_in(app, &lang) {
            Some(i) => speak(app, i),
            None => {
                let source = app.session.active_sequence().and_then(|q| q.caption_tracks.iter().find(|t| !t.captions.is_empty()).map(|t| t.id.0));
                app.ui.dub.translating = true;
                crate::panels::ai_tasks::start(app, "captions.translate", json!({"track": source, "to": lang}), "Translating captions", "the caption text");
            }
        }
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.dub.dialog = false;
    }
}
