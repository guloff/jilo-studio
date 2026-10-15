//! AI ▸ Cut into Shorts…: a long recording into vertical shorts in one go. The dialog asks how
//! many, how long, what to look for, the caption look, whether to follow the face and whether to
//! export; the highlights are found by the text engine (`highlights.find`, a cloud task with the
//! usual consent and progress), then `shorts.fromHighlights` makes a sequence per highlight with
//! captions and a vertical copy following the speaker, and queues them for export if asked.
//!
//! Automation ids: `shorts.dialog`, `shorts.count`, `shorts.minSeconds`, `shorts.maxSeconds`,
//! `shorts.mode.<viral|meaningful|facts>`, `shorts.look`, `shorts.face`, `shorts.export`,
//! `shorts.go`, `shorts.cancel`.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The looks offered for the shorts' captions (preset ids; none: no captions).
const LOOKS: [&str; 6] = ["karaoke-yellow", "yellow-pop", "bold-outline", "clean-shadow", "dark-pill", ""];

#[derive(Clone, Debug)]
pub struct ShortsUi {
    pub dialog: bool,
    pub count: u32,
    pub min_seconds: f32,
    pub max_seconds: f32,
    pub mode: String,
    pub look: String,
    pub face: bool,
    pub export: bool,
    /// The highlights are being found; the shorts are made when they arrive.
    pub waiting: bool,
}

impl Default for ShortsUi {
    fn default() -> Self {
        Self {
            dialog: false,
            count: 3,
            min_seconds: 20.0,
            max_seconds: 60.0,
            mode: "viral".into(),
            look: "karaoke-yellow".into(),
            face: true,
            export: false,
            waiting: false,
        }
    }
}

/// Open the dialog (refused, with the reason, when the sequence has no transcript).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("highlights.find").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    app.ui.shorts.dialog = true;
    Ok(json!({"dialog": "shorts"}))
}

/// The shorts for highlights found: `found` are `highlights.find` result items.
pub fn make(app: &mut FilmcraftApp, found: &[Value]) {
    let markers: Vec<u64> = found.iter().filter_map(|f| f["marker"].as_u64()).collect();
    if markers.is_empty() {
        app.ui.status = tl!("No highlights were found: nothing to cut.").to_string();
        return;
    }
    let u = app.ui.shorts.clone();
    let look = if u.look.is_empty() { Value::Null } else { json!(u.look) };
    let p = json!({"markers": markers, "captions": look, "follow": if u.face { "face" } else { "center" }, "export": u.export});
    app.ui.status = match app.session.execute("shorts.fromHighlights", p) {
        Ok(r) => tlf!("Shorts made: {n}. The first one is open.", n = r["shorts"].as_array().map_or(0, Vec::len)),
        Err(e) => e.to_string(),
    };
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    // the highlights of our search: the shorts, instead of the results window
    if app.ui.shorts.waiting {
        if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "highlights.find")
            && let Some((_, r)) = app.ui.ai_task_result.take()
        {
            app.ui.shorts.waiting = false;
            make(app, r["highlights"].as_array().map(Vec::as_slice).unwrap_or_default());
        } else if app.ui.ai_task.is_none() {
            // stopped or failed: the status bar says why
            app.ui.shorts.waiting = false;
        }
    }
    if !app.ui.shorts.dialog {
        return;
    }
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 460.0;
    let provider = app.session.prefs.ai.text_provider();
    let can_face = app.session.face_detector.is_some();
    egui::Modal::new(egui::Id::new("shorts-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Cut into Shorts"), w);
        app.auto.add("shorts.dialog", bar, "Cut into Shorts");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(
                RichText::new(tl!(
                    "The best moments of the recording become vertical shorts: each its own sequence, with captions, the speaker kept in frame."
                ))
                .color(t.text_dim),
            );
            let u = &mut app.ui.shorts;
            egui::Grid::new("shorts-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("How many"));
                let r = ui.add(egui::DragValue::new(&mut u.count).range(1..=20));
                app.auto.add("shorts.count", r.rect, "How many");
                ui.end_row();
                ui.label(tl!("Length, seconds"));
                ui.horizontal(|ui| {
                    let a = ui.add(egui::DragValue::new(&mut u.min_seconds).range(5.0..=120.0).speed(1.0).max_decimals(0));
                    ui.label("–");
                    let lo = u.min_seconds + 5.0;
                    let b = ui.add(egui::DragValue::new(&mut u.max_seconds).range(lo..=180.0).speed(1.0).max_decimals(0));
                    app.auto.add("shorts.minSeconds", a.rect, "Shortest");
                    app.auto.add("shorts.maxSeconds", b.rect, "Longest");
                });
                ui.end_row();
                ui.label(tl!("Look for"));
                ui.horizontal(|ui| {
                    for (id, name) in [("viral", tl!("Viral moments")), ("meaningful", tl!("Meaningful ideas")), ("facts", tl!("Facts"))] {
                        let r = ui.radio_value(&mut u.mode, id.to_string(), name);
                        app.auto.add(&format!("shorts.mode.{id}"), r.rect, name);
                    }
                });
                ui.end_row();
                ui.label(tl!("Captions"));
                let presets = filmcraft_engine::caption_presets::builtin();
                let name = |id: &str| {
                    if id.is_empty() {
                        tl!("No captions").to_string()
                    } else {
                        presets.iter().find(|p| p.id == id).map_or(id.to_string(), |p| crate::i18n::t(p.name).to_string())
                    }
                };
                let r = egui::ComboBox::from_id_salt("shorts-look").selected_text(name(&u.look)).width(200.0).show_ui(ui, |ui| {
                    for id in LOOKS {
                        ui.selectable_value(&mut u.look, id.to_string(), name(id));
                    }
                });
                app.auto.add("shorts.look", r.response.rect, "Captions");
                ui.end_row();
            });
            ui.add_enabled_ui(can_face, |ui| {
                let r = ui.checkbox(&mut u.face, tl!("Keep the speaker in frame (Apple Vision)"));
                app.auto.add("shorts.face", r.rect, "Keep the speaker in frame");
            });
            let r = ui.checkbox(&mut u.export, tl!("Add the shorts to the export queue"));
            app.auto.add("shorts.export", r.rect, "Add to the export queue");
            u.max_seconds = u.max_seconds.max(u.min_seconds + 5.0);
            ui.label(
                RichText::new(tlf!(
                    "The transcript goes to {provider} to find the moments; everything else is done on this computer and is one undo step.",
                    provider = provider.name()
                ))
                .color(t.text_dim)
                .small(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add_enabled(app.ui.ai_task.is_none(), crate::dialog_style::primary(tl!("Cut")));
                app.auto.add("shorts.go", g.rect, "Cut");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("shorts.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let u = &mut app.ui.shorts;
        u.dialog = false;
        u.waiting = true;
        let params = json!({"mode": u.mode, "count": u.count, "minSeconds": u.min_seconds.round(), "maxSeconds": u.max_seconds.round()});
        crate::panels::ai_tasks::start(app, "highlights.find", params, "Finding highlights", "the transcript");
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.shorts.dialog = false;
    }
}
