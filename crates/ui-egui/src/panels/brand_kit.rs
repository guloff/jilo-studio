//! Graphics and Titles ▸ Brand Kit…: a logo, colours, a font, a caption style, a lower third and
//! an intro and outro kept as a named kit, and put on the open sequence with one click (one undo
//! step). The window stays open beside the work. Every action is a `brandKit.*` command.
//!
//! Automation ids: `brandKit.window`, `brandKit.kit`, `brandKit.name`, `brandKit.logo`,
//! `brandKit.logoClear`, `brandKit.corner`, `brandKit.size`, `brandKit.opacity`,
//! `brandKit.color.<n>`, `brandKit.font`, `brandKit.captions`, `brandKit.lowerThird`,
//! `brandKit.intro`, `brandKit.outro`, `brandKit.save`, `brandKit.delete`,
//! `brandKit.part.<part>`, `brandKit.apply`, `brandKit.ltText`, `brandKit.ltRole`, `brandKit.addLowerThird`.

use egui::{Color32, RichText, vec2};
use filmcraft_engine::brand_kit::{BrandKit, PARTS};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The window's state.
#[derive(Clone, Debug)]
pub struct BrandKitUi {
    pub open: bool,
    /// The kit shown, as edited.
    pub draft: BrandKit,
    /// The parts Apply puts on, in [`PARTS`] order.
    pub parts: [bool; 5],
    /// Lower third text and role.
    pub lt_text: String,
    pub lt_role: String,
    pub message: String,
}

impl Default for BrandKitUi {
    fn default() -> Self {
        Self { open: false, draft: BrandKit::default(), parts: [true; 5], lt_text: String::new(), lt_role: String::new(), message: String::new() }
    }
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    app.ui.brand_kit.open = true;
    if app.ui.brand_kit.draft.name.is_empty()
        && let Some(k) = saved().into_iter().next()
    {
        app.ui.brand_kit.draft = k;
    }
    Ok(json!({"window": "brandKit"}))
}

fn saved() -> Vec<BrandKit> {
    filmcraft_engine::brand_kit::dir().map(|d| filmcraft_engine::brand_kit::list_in(&d)).unwrap_or_default()
}

const CORNERS: [(&str, &str); 4] = [("topLeft", "Top Left"), ("topRight", "Top Right"), ("bottomLeft", "Bottom Left"), ("bottomRight", "Bottom Right")];
const COLORS: [&str; 3] = ["Main color", "Second", "Accent"];
const PART_NAMES: [&str; 5] = ["Caption style", "Titles and lower thirds", "Intro", "Outro", "Logo"];

fn hex_to_color(h: &str) -> Color32 {
    let v = u32::from_str_radix(h.trim_start_matches('#').get(..6).unwrap_or("ffffff"), 16).unwrap_or(0xffffff);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn color_to_hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// A file path row: the name, Browse… and a clear button.
fn path_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, value: &mut Option<String>, kinds: (&str, &[&str]), browse: &mut Option<String>) {
    let t = app.tokens;
    let shown = value.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map_or_else(|| tl!("None").to_string(), |n| n.to_string_lossy().to_string());
    ui.add(egui::Label::new(RichText::new(shown).color(if value.is_some() { t.text } else { t.text_dim })).truncate());
    let r = ui.add_enabled(app.hooks.pick_open_file.is_some(), egui::Button::new(tl!("Browse…")).small());
    app.auto.add(&format!("brandKit.{id}"), r.rect, id);
    if r.clicked()
        && let Some(f) = app.hooks.pick_open_file.as_mut()
    {
        *browse = f(kinds.0, kinds.1);
    }
    let r = ui.add_enabled(value.is_some(), egui::Button::new(tl!("Clear")).small());
    app.auto.add(&format!("brandKit.{id}Clear"), r.rect, "Clear");
    if r.clicked() {
        *value = None;
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.brand_kit.open {
        return;
    }
    let t = app.tokens;
    let mut open = true;
    let mut actions: Vec<(&str, Value)> = Vec::new();
    let kits = saved();
    let presets = filmcraft_engine::caption_presets::builtin();
    let thirds: Vec<(String, String)> = app
        .session
        .execute("graphics.template.list", json!({"category": "Lower Thirds"}))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| Some((x["id"].as_str()?.to_string(), x["name"].as_str()?.to_string())))
        .collect();
    let has_seq = app.session.active_sequence().is_some();
    let mut browse: [Option<String>; 3] = [None, None, None];
    crate::dialog_style::Window::new(tl!("Brand Kit"))
        .id(egui::Id::new("brand-kit-window"))
        .open(&mut open)
        .default_size(vec2(520.0, 560.0))
        .collapsible(true)
        .show(ctx, |ui| {
            app.auto.add("brandKit.window", ui.max_rect(), "Brand Kit");
            let b = &mut app.ui.brand_kit;
            ui.horizontal(|ui| {
                ui.label(tl!("Kit"));
                let cur = if b.draft.name.is_empty() { tl!("New Kit").to_string() } else { b.draft.name.clone() };
                let r = egui::ComboBox::from_id_salt("brandKit.kit").width(200.0).selected_text(cur).show_ui(ui, |ui| {
                    for k in &kits {
                        if ui.selectable_label(k.name == b.draft.name, &k.name).clicked() {
                            b.draft = k.clone();
                        }
                    }
                    if ui.selectable_label(false, tl!("New Kit")).clicked() {
                        b.draft = BrandKit::default();
                    }
                });
                app.auto.add("brandKit.kit", r.response.rect, "Kit");
            });
            ui.separator();
            let b = &mut app.ui.brand_kit;
            let mut grid_rects: Vec<(String, egui::Rect)> = Vec::new();
            egui::Grid::new("brand-kit-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Name"));
                let r = ui.add(egui::TextEdit::singleline(&mut b.draft.name).hint_text(tl!("My Brand")).desired_width(240.0));
                grid_rects.push(("brandKit.name".into(), r.rect));
                ui.end_row();

                ui.label(tl!("Colors"));
                ui.horizontal(|ui| {
                    for (i, name) in COLORS.iter().enumerate() {
                        let mut c = b.draft.colors.get(i).map_or(Color32::GRAY, |h| hex_to_color(h));
                        let r = egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque);
                        grid_rects.push((format!("brandKit.color.{i}"), r.rect));
                        if r.changed() {
                            while b.draft.colors.len() <= i {
                                b.draft.colors.push("#808080".into());
                            }
                            b.draft.colors[i] = color_to_hex(c);
                        }
                        ui.label(RichText::new(crate::i18n::t(name)).color(t.text_dim).small());
                    }
                });
                ui.end_row();

                ui.label(tl!("Font"));
                let fams = filmcraft_text::families();
                let cur = b.draft.font.clone().unwrap_or_else(|| tl!("Keep each style's font").to_string());
                let r = egui::ComboBox::from_id_salt("brandKit.font").width(240.0).height(360.0).selected_text(cur).show_ui(ui, |ui| {
                    if !filmcraft_text::fonts::system_scanned() {
                        filmcraft_text::fonts::scan_system();
                    }
                    if ui.selectable_label(b.draft.font.is_none(), tl!("Keep each style's font")).clicked() {
                        b.draft.font = None;
                    }
                    for (f, _) in &fams {
                        if ui.selectable_label(b.draft.font.as_deref() == Some(f.as_str()), f).clicked() {
                            b.draft.font = Some(f.clone());
                        }
                    }
                });
                grid_rects.push(("brandKit.font".into(), r.response.rect));
                ui.end_row();

                ui.label(tl!("Caption style"));
                let cur = b
                    .draft
                    .caption_preset
                    .as_deref()
                    .and_then(|id| presets.iter().find(|p| p.id == id))
                    .map_or_else(|| tl!("None").to_string(), |p| crate::i18n::t(p.name).to_string());
                let r = egui::ComboBox::from_id_salt("brandKit.captions").width(240.0).height(360.0).selected_text(cur).show_ui(ui, |ui| {
                    if ui.selectable_label(b.draft.caption_preset.is_none(), tl!("None")).clicked() {
                        b.draft.caption_preset = None;
                    }
                    for p in &presets {
                        if ui.selectable_label(b.draft.caption_preset.as_deref() == Some(p.id), crate::i18n::t(p.name)).clicked() {
                            b.draft.caption_preset = Some(p.id.to_string());
                        }
                    }
                });
                grid_rects.push(("brandKit.captions".into(), r.response.rect));
                ui.end_row();

                ui.label(tl!("Lower third"));
                let cur = b
                    .draft
                    .lower_third
                    .as_deref()
                    .and_then(|id| thirds.iter().find(|x| x.0 == id))
                    .map_or_else(|| tl!("Default").to_string(), |x| crate::i18n::t(&x.1).to_string());
                let r = egui::ComboBox::from_id_salt("brandKit.lowerThird").width(240.0).selected_text(cur).show_ui(ui, |ui| {
                    if ui.selectable_label(b.draft.lower_third.is_none(), tl!("Default")).clicked() {
                        b.draft.lower_third = None;
                    }
                    for (id, name) in &thirds {
                        if ui.selectable_label(b.draft.lower_third.as_deref() == Some(id.as_str()), crate::i18n::t(name)).clicked() {
                            b.draft.lower_third = Some(id.clone());
                        }
                    }
                });
                grid_rects.push(("brandKit.lowerThird".into(), r.response.rect));
                ui.end_row();
            });
            for (id, r) in grid_rects {
                app.auto.add(&id, r, &id);
            }
            // files: logo, intro, outro
            egui::Grid::new("brand-kit-files").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                let images: (&str, &[&str]) = ("Images", &["png", "jpg", "jpeg", "webp", "gif", "tif", "tiff"]);
                let videos: (&str, &[&str]) = ("Video", &["mov", "mp4", "m4v", "mkv", "webm", "png", "jpg"]);
                ui.label(tl!("Logo"));
                ui.horizontal(|ui| {
                    let mut v = app.ui.brand_kit.draft.logo.clone();
                    path_row(app, ui, "logo", &mut v, images, &mut browse[0]);
                    app.ui.brand_kit.draft.logo = v;
                });
                ui.end_row();
                if app.ui.brand_kit.draft.logo.is_some() {
                    ui.label("");
                    ui.horizontal(|ui| {
                        let d = &mut app.ui.brand_kit.draft;
                        let cur = CORNERS.iter().find(|c| c.0 == d.logo_corner).map_or("Top Right", |c| c.1);
                        let r = egui::ComboBox::from_id_salt("brandKit.corner").width(120.0).selected_text(crate::i18n::t(cur)).show_ui(ui, |ui| {
                            for (id, name) in CORNERS {
                                ui.selectable_value(&mut d.logo_corner, id.to_string(), crate::i18n::t(name));
                            }
                        });
                        app.auto.add("brandKit.corner", r.response.rect, "Corner");
                        let d = &mut app.ui.brand_kit.draft;
                        let r = ui.add(egui::Slider::new(&mut d.logo_size, 4.0..=40.0).suffix(" %").max_decimals(0).text(tl!("width")));
                        app.auto.add("brandKit.size", r.rect, "Logo width");
                        let d = &mut app.ui.brand_kit.draft;
                        let r = ui.add(egui::Slider::new(&mut d.logo_opacity, 10.0..=100.0).suffix(" %").max_decimals(0).text(tl!("opacity")));
                        app.auto.add("brandKit.opacity", r.rect, "Logo opacity");
                    });
                    ui.end_row();
                }
                ui.label(tl!("Intro"));
                ui.horizontal(|ui| {
                    let mut v = app.ui.brand_kit.draft.intro.clone();
                    path_row(app, ui, "intro", &mut v, videos, &mut browse[1]);
                    app.ui.brand_kit.draft.intro = v;
                });
                ui.end_row();
                ui.label(tl!("Outro"));
                ui.horizontal(|ui| {
                    let mut v = app.ui.brand_kit.draft.outro.clone();
                    path_row(app, ui, "outro", &mut v, videos, &mut browse[2]);
                    app.ui.brand_kit.draft.outro = v;
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let named = !app.ui.brand_kit.draft.name.trim().is_empty();
                let r = ui.add_enabled(named, egui::Button::new(tl!("Save")));
                app.auto.add("brandKit.save", r.rect, "Save");
                if r.clicked() {
                    let kit = serde_json::to_value(&app.ui.brand_kit.draft).unwrap_or_default();
                    actions.push(("brandKit.save", json!({"kit": kit})));
                }
                let exists = kits.iter().any(|k| k.name == app.ui.brand_kit.draft.name);
                let r = ui.add_enabled(exists, egui::Button::new(tl!("Delete")));
                app.auto.add("brandKit.delete", r.rect, "Delete");
                if r.clicked() {
                    actions.push(("brandKit.delete", json!({"name": app.ui.brand_kit.draft.name})));
                }
            });
            ui.separator();
            // apply
            ui.label(RichText::new(tl!("Apply to the open sequence")).strong());
            let has = app.ui.brand_kit.draft.parts();
            ui.horizontal_wrapped(|ui| {
                for (k, part) in PARTS.iter().enumerate() {
                    let b = &mut app.ui.brand_kit;
                    let r = ui.add_enabled(has.contains(part), egui::Checkbox::new(&mut b.parts[k], crate::i18n::t(PART_NAMES[k])));
                    app.auto.add(&format!("brandKit.part.{part}"), r.rect, part);
                }
            });
            let chosen: Vec<&str> = PARTS.iter().enumerate().filter(|(k, p)| app.ui.brand_kit.parts[*k] && has.contains(p)).map(|(_, p)| *p).collect();
            let r = ui.add_enabled(has_seq && !chosen.is_empty(), crate::dialog_style::primary(tl!("Apply to Sequence")));
            app.auto.add("brandKit.apply", r.rect, "Apply to Sequence");
            if r.clicked() {
                let kit = serde_json::to_value(&app.ui.brand_kit.draft).unwrap_or_default();
                actions.push(("brandKit.apply", json!({"kit": kit, "parts": chosen})));
            }
            ui.label(
                RichText::new(tl!("One Undo takes the whole kit back. Applying again replaces the logo and does not add the intro or outro twice."))
                    .color(t.text_dim)
                    .small(),
            );
            ui.separator();
            // lower third at the playhead
            ui.label(RichText::new(tl!("Lower third at the playhead")).strong());
            ui.horizontal(|ui| {
                let b = &mut app.ui.brand_kit;
                let r = ui.add(egui::TextEdit::singleline(&mut b.lt_text).hint_text(tl!("Name")).desired_width(150.0));
                app.auto.add("brandKit.ltText", r.rect, "Name");
                let b = &mut app.ui.brand_kit;
                let r = ui.add(egui::TextEdit::singleline(&mut b.lt_role).hint_text(tl!("Role")).desired_width(150.0));
                app.auto.add("brandKit.ltRole", r.rect, "Role");
                let r = ui.add_enabled(has_seq, egui::Button::new(tl!("Add")));
                app.auto.add("brandKit.addLowerThird", r.rect, "Add Lower Third");
                if r.clicked() {
                    let b = &app.ui.brand_kit;
                    let kit = serde_json::to_value(&b.draft).unwrap_or_default();
                    let mut p = json!({"kit": kit});
                    if !b.lt_text.trim().is_empty() {
                        p["text"] = json!(b.lt_text.trim());
                    }
                    if !b.lt_role.trim().is_empty() {
                        p["role"] = json!(b.lt_role.trim());
                    }
                    actions.push(("brandKit.addLowerThird", p));
                }
            });
            if !app.ui.brand_kit.message.is_empty() {
                ui.label(RichText::new(&app.ui.brand_kit.message).color(t.text_dim).small());
            }
        });
    let d = &mut app.ui.brand_kit.draft;
    for (slot, v) in [&mut d.logo, &mut d.intro, &mut d.outro].into_iter().zip(browse) {
        if v.is_some() {
            *slot = v;
        }
    }
    for (cmd, p) in actions {
        let r = app.session.execute(cmd, p);
        let b = &mut app.ui.brand_kit;
        b.message = match (cmd, r) {
            ("brandKit.save", Ok(_)) => tlf!("Saved “{name}”.", name = b.draft.name),
            ("brandKit.delete", Ok(_)) => {
                b.draft = BrandKit::default();
                tl!("Deleted.").to_string()
            }
            ("brandKit.apply", Ok(_)) => tlf!("“{name}” is on the sequence. One Undo takes it back.", name = b.draft.name),
            ("brandKit.addLowerThird", Ok(_)) => tl!("Lower third added at the playhead.").to_string(),
            (_, Ok(_)) => String::new(),
            (_, Err(e)) => e.to_string(),
        };
        app.ui.status = app.ui.brand_kit.message.clone();
    }
    if !open {
        app.ui.brand_kit.open = false;
    }
}
