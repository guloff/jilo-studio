//! File ▸ Export ▸ Export in All Formats…: the open sequence and a copy of it in each other chosen
//! aspect (9:16, 1:1, 4:5, 16:9) whose clips fill the frame — centred or following the speaker's
//! face — go to the export queue with one preset (`export.allFormats`). Each copy is one undo step.

use egui::RichText;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The aspects offered, in order: (id, name, what it is for).
pub const ASPECTS: [(&str, &str, &str); 4] = [
    ("16:9", "16:9 Landscape", "YouTube, TV"),
    ("9:16", "9:16 Vertical", "Shorts, Reels, TikTok"),
    ("1:1", "1:1 Square", "Feeds"),
    ("4:5", "4:5 Portrait", "Instagram feed"),
];

/// The dialog's choices.
#[derive(Clone, Debug)]
pub struct FormatsUi {
    pub dialog: bool,
    /// The chosen aspects, in [`ASPECTS`] order.
    pub aspects: [bool; 4],
    /// Clips follow the speaker's face (else they are centred).
    pub follow_face: bool,
    /// Export preset ("" = the current export settings).
    pub preset: String,
    /// Folder for the files ("" = the export folder of the current settings).
    pub folder: String,
    /// Start the export queue at once (else the formats wait in it).
    pub start: bool,
}

impl Default for FormatsUi {
    fn default() -> Self {
        Self { dialog: false, aspects: [true, true, true, false], follow_face: true, preset: String::new(), folder: String::new(), start: true }
    }
}

/// Open the dialog (refused, with the reason, without an open sequence).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("export.allFormats").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    app.ui.formats.dialog = true;
    Ok(json!({"dialog": "allFormats"}))
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.ui.formats.dialog {
        dialog(app, ctx);
    }
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 440.0;
    let presets: Vec<String> = app
        .session
        .execute("export.presets.list", json!({}))
        .ok()
        .and_then(|v| v["presets"].as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default();
    let own = app.session.active_sequence().map(|q| filmcraft_engine::formats::aspect_of(q.settings.width, q.settings.height)).unwrap_or("16:9");
    let faces = app.session.face_detector.is_some();
    let browse_ok = app.hooks.pick_folder.is_some();
    let mut browse = false;
    egui::Modal::new(egui::Id::new("formats-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Export in All Formats"), w);
        app.auto.add("formats.dialog", bar, "Export in All Formats");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new(tl!("Formats")).strong());
            let f = &mut app.ui.formats;
            for (k, (id, name, about)) in ASPECTS.iter().enumerate() {
                ui.horizontal(|ui| {
                    let r = ui.checkbox(&mut f.aspects[k], crate::i18n::t(name));
                    app.auto.add(&format!("formats.aspect.{id}"), r.rect, name);
                    let note = if *id == own { format!("{} · {}", crate::i18n::t(about), tl!("this sequence")) } else { crate::i18n::t(about).to_string() };
                    ui.label(RichText::new(note).color(t.text_dim).small());
                });
            }
            ui.add_space(4.0);
            let r = ui.add_enabled(faces, egui::Checkbox::new(&mut f.follow_face, tl!("Follow the speaker's face")));
            app.auto.add("formats.followFace", r.rect, "Follow the speaker's face");
            ui.horizontal(|ui| {
                ui.label(tl!("Preset"));
                let cur = if f.preset.is_empty() { tl!("The current export settings").to_string() } else { crate::i18n::t(&f.preset).to_string() };
                let r = egui::ComboBox::from_id_salt("formats.preset").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.preset, String::new(), tl!("The current export settings"));
                    for p in &presets {
                        ui.selectable_value(&mut f.preset, p.clone(), crate::i18n::t(p));
                    }
                });
                app.auto.add("formats.preset", r.response.rect, "Export preset");
            });
            ui.horizontal(|ui| {
                ui.label(tl!("Folder"));
                let shown = if f.folder.is_empty() { tl!("The current export folder").to_string() } else { f.folder.clone() };
                ui.add(egui::Label::new(RichText::new(shown).color(t.text_dim)).truncate());
                let b = ui.add_enabled(browse_ok, egui::Button::new(tl!("Browse…")));
                app.auto.add("formats.folder", b.rect, "Choose folder");
                browse = b.clicked();
            });
            let r = ui.checkbox(&mut f.start, tl!("Start exporting now"));
            app.auto.add("formats.start", r.rect, "Start exporting now");
            ui.add(
                egui::Label::new(
                    RichText::new(tl!(
                        "A copy of the sequence is made for each other format; its clips fill the frame. Captions carry over; check titles and graphics in the copies. Each copy can be undone."
                    ))
                    .color(t.text_dim)
                    .small(),
                )
                .wrap(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add_enabled(f.aspects.iter().any(|x| *x), crate::dialog_style::primary(tl!("Export")));
                app.auto.add("formats.go", g.rect, "Export");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("formats.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let f = &app.ui.formats;
        let aspects: Vec<&str> = ASPECTS.iter().zip(f.aspects).filter(|(_, on)| *on).map(|(a, _)| a.0).collect();
        let mut p = json!({"aspects": aspects, "follow": if f.follow_face && faces { "face" } else { "center" }});
        if !f.preset.is_empty() {
            p["preset"] = json!(f.preset);
        }
        if !f.folder.is_empty() {
            p["path"] = json!(format!("{}/", f.folder.trim_end_matches('/')));
        }
        p["start"] = json!(f.start);
        app.ui.formats.dialog = false;
        app.ui.status = match app.session.execute("export.allFormats", p) {
            Ok(r) => tlf!("Exporting {n} formats: see the Export Queue", n = r["sequences"].as_array().map_or(0, Vec::len)),
            Err(e) => e.to_string(),
        };
    } else if browse {
        if let Some(d) = app.hooks.pick_folder.as_mut().and_then(|f| f()) {
            app.ui.formats.folder = d;
        }
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.formats.dialog = false;
    }
}
