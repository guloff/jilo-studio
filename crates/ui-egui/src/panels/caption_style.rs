//! Caption Style panel: how captions look, in one place. A gallery of presets (each tile is a
//! frame drawn by the same caption renderer as playback and export) applies a look with one click;
//! below it every field of the track's style — font, style, size, capitals, colour, outline, box,
//! shadow, the spoken word (karaoke highlight, words at once, entrance animation), position. Changes go to the caption track chosen at the top, or to every caption track
//! of the sequence when "All caption tracks" is on. Every change is `captions.setStyle` or
//! `captions.applyStylePreset`, so the panel, MCP and the control channel share one path.
//!
//! Presets in the caption font pack ([`filmcraft_engine::caption_fonts`]) carry a ↓ badge until the
//! pack is downloaded (the button above the gallery says its size, source and licence); until then
//! they draw in Inter.
//!
//! The user's own styles follow (My Styles: Save Style, Import…, right-click a tile for Export… and
//! Delete). Hovering any tile previews its look in the paused Program monitor.
//!
//! Automation ids: `captionStyle.track`, `captionStyle.all`, `captionStyle.preset.<id>` (own
//! styles: `user:<name>`), `captionStyle.saveName`, `captionStyle.save`, `captionStyle.import`,
//! `captionStyle.downloadFonts`,
//! `captionStyle.<field>` (font, fontStyle, size, uppercase, color, outline, outlineColor,
//! background, backgroundColor, boxRadius, shadow, shadowColor, shadowBlur, shadowX, shadowY,
//! align.<left|center|right>, anchor.<top|middle|bottom>, margin, lineSpacing), `captionStyle.reset`,
//! `captionStyle.newTrack`.

use egui::{Color32, Rect, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{Caption, CaptionAlign, CaptionAnchor, CaptionAnimation, CaptionHighlight, CaptionStyle, ClipId};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

const PAD: f32 = 12.0;
/// Preset tiles: a 16:9 frame in points, drawn from a 640×360 render.
const TILE_W: f32 = 148.0;
const TILE_H: f32 = TILE_W * 9.0 / 16.0;
const RENDER_W: usize = 640;
const RENDER_H: usize = 360;
const LABEL_W: f32 = 104.0;

/// Panel state kept between frames (egui memory).
#[derive(Clone, Copy, Debug, Default)]
struct PanelMemory {
    /// Index of the caption track shown.
    track: usize,
    /// Changes go to every caption track.
    all: bool,
    /// Changes go only to the selected captions (a style of their own).
    phrase: bool,
}

fn memory_id() -> egui::Id {
    egui::Id::new("caption-style-panel")
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    ui.painter().rect_filled(rect, 0.0, t.panel_bg);
    let mut actions: Vec<(String, Value)> = Vec::new();
    let mut mem: PanelMemory = ui.ctx().data(|d| d.get_temp(memory_id())).unwrap_or_default();
    let tracks: Vec<(u64, String, CaptionStyle)> =
        app.session.active_sequence().map(|q| q.caption_tracks.iter().map(|tr| (tr.id.0, tr.name.clone(), tr.style.clone())).collect()).unwrap_or_default();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(PAD, 8.0))).id_salt("caption-style"));
    let ui = &mut child;
    if app.session.active_sequence().is_none() {
        ui.label(RichText::new(tl!("Open a sequence to style its captions.")).color(t.text_dim));
        return;
    }
    if tracks.is_empty() {
        ui.label(
            RichText::new(tl!("This sequence has no captions yet. Create them from the transcript (AI ▸ Speech and Captions) or add a caption track."))
                .color(t.text_dim),
        );
        ui.add_space(8.0);
        let r = ui.button(tl!("Add Caption Track"));
        app.auto.add("captionStyle.newTrack", r.rect, "Add Caption Track");
        if r.clicked() {
            actions.push(("captions.newTrack".into(), json!({})));
        }
        run(app, ui, actions);
        return;
    }
    mem.track = mem.track.min(tracks.len() - 1);
    let (track_id, _, track_style) = tracks[mem.track].clone();
    let target = if mem.all { json!("all") } else { json!(track_id) };
    // the selected captions on this track: they may get a style of their own
    let phrases: Vec<(ClipId, Option<CaptionStyle>)> = app
        .session
        .active_sequence()
        .and_then(|q| q.caption_tracks.get(mem.track))
        .map(|tr| tr.captions.iter().filter(|c| app.session.state.caption_selection.contains(&c.id)).map(|c| (c.id, c.style.as_deref().cloned())).collect())
        .unwrap_or_default();
    let phrase_mode = mem.phrase && !phrases.is_empty();
    let style = if phrase_mode { phrases[0].1.clone().unwrap_or_else(|| track_style.clone()) } else { track_style.clone() };

    // ---- target: track and "all"
    ui.horizontal(|ui| {
        let label = |i: usize| format!("C{} · {}", i + 1, tracks[i].1);
        let r = egui::ComboBox::from_id_salt("caption-style-track").selected_text(label(mem.track)).width(170.0).show_ui(ui, |ui| {
            for i in 0..tracks.len() {
                ui.selectable_value(&mut mem.track, i, label(i));
            }
        });
        app.auto.add("captionStyle.track", r.response.rect, "Caption track");
        let r = ui.checkbox(&mut mem.all, tl!("All caption tracks"));
        app.auto.add("captionStyle.all", r.rect, "All caption tracks");
    });
    if !phrases.is_empty() {
        ui.horizontal(|ui| {
            let r = ui.checkbox(&mut mem.phrase, tlf!("Only the selected phrases ({n})", n = phrases.len()));
            app.auto.add("captionStyle.phrase", r.rect, "Only the selected phrases");
            if phrase_mode && phrases.iter().any(|p| p.1.is_some()) {
                let r = ui.small_button(tl!("Use Track Style"));
                app.auto.add("captionStyle.phraseReset", r.rect, "Use Track Style");
                if r.clicked() {
                    actions.push(("captions.setStyle".into(), json!({"captions": phrases.iter().map(|p| p.0.0).collect::<Vec<_>>(), "own": false})));
                }
            }
        });
    }
    ui.add_space(6.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("caption-style-scroll").show(ui, |ui| {
        ui.set_width(ui.available_width());
        // ---- presets
        heading(ui, tl!("Presets"), &t);
        let sample = tl!("Sample caption text");
        let pack = pack_installed(ui.ctx());
        if !pack {
            let busy = app.session.jobs.iter().any(|j| j.label == "Downloading caption fonts" && !j.progress.finished.load(std::sync::atomic::Ordering::Relaxed));
            ui.label(
                RichText::new(tlf!(
                    "Presets marked ↓ use the caption fonts: Montserrat, Oswald, Fira Sans Condensed and five more, with Russian and Tajik letters. {size} from Google Fonts, SIL Open Font License.",
                    size = format!("{:.1} MB", filmcraft_engine::caption_fonts::size() as f64 / 1e6)
                ))
                .size(11.5)
                .color(t.text_dim),
            );
            let r = ui.add_enabled(!busy, egui::Button::new(if busy { tl!("Downloading…") } else { tl!("Download Caption Fonts") }));
            app.auto.add("captionStyle.downloadFonts", r.rect, "Download Caption Fonts");
            if r.clicked() {
                actions.push(("fonts.downloadCaptionPack".into(), json!({})));
            }
            ui.add_space(6.0);
        }
        let cols = ((ui.available_width() + 8.0) / (TILE_W + 8.0)).floor().max(1.0) as usize;
        let presets = filmcraft_engine::caption_presets::builtin();
        for row in presets.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for p in row {
                    let (r, resp) = ui.allocate_exact_size(vec2(TILE_W, TILE_H + 18.0), Sense::click());
                    let frame = Rect::from_min_size(r.min, vec2(TILE_W, TILE_H));
                    let needs_pack = p.style.font != "Inter";
                    let tex = preview_texture(ui.ctx(), p.id, &p.style, sample, pack);
                    ui.painter().image(tex.id(), frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    if needs_pack && !pack {
                        let badge = Rect::from_min_size(pos2(frame.max.x - 20.0, frame.min.y + 4.0), vec2(16.0, 16.0));
                        ui.painter().rect_filled(badge, 8.0, Color32::from_black_alpha(170));
                        ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, "↓", Tokens::semibold(11.0), Color32::WHITE);
                    }
                    let current = p.style == style;
                    let stroke = if current {
                        Stroke::new(2.0, t.accent)
                    } else if resp.hovered() {
                        Stroke::new(1.5, t.text_dim)
                    } else {
                        Stroke::new(1.0, t.field_border)
                    };
                    ui.painter().rect_stroke(frame, 4.0, stroke, StrokeKind::Inside);
                    let name = crate::i18n::t(p.name);
                    ui.painter().text(
                        pos2(frame.center().x, frame.max.y + 9.0),
                        egui::Align2::CENTER_CENTER,
                        name,
                        Tokens::ui(11.5),
                        if current { t.text } else { t.text_dim },
                    );
                    app.auto.add(&format!("captionStyle.preset.{}", p.id), r, name);
                    if resp.hovered() {
                        app.ui.caption_hover = Some((p.id.to_string(), p.style.clone(), ui.ctx().cumulative_pass_nr()));
                    }
                    if resp.clicked() {
                        let q = if phrase_mode {
                            json!({"preset": p.id, "captions": phrases.iter().map(|x| x.0.0).collect::<Vec<_>>()})
                        } else {
                            json!({"preset": p.id, "track": target})
                        };
                        actions.push(("captions.applyStylePreset".into(), q));
                    }
                }
            });
            ui.add_space(6.0);
        }
        let phrase_target = phrase_mode.then(|| json!(phrases.iter().map(|x| x.0.0).collect::<Vec<_>>()));
        my_styles(app, ui, &style, &target, phrase_target.as_ref(), sample, pack, &mut actions);
        ui.add_space(6.0);
        let phrase_ids: Vec<u64> = phrases.iter().map(|p| p.0.0).collect();
        let mut set = |field: &str, v: Value| {
            let q = if phrase_mode { json!({"captions": phrase_ids, field: v}) } else { json!({"track": target, field: v}) };
            actions.push(("captions.setStyle".into(), q));
        };

        // ---- text
        heading(ui, tl!("Text"), &t);
        let fams = filmcraft_text::families();
        row(ui, tl!("Font"), |ui| {
            let r = egui::ComboBox::from_id_salt("caption-style-font").selected_text(&style.font).width(180.0).height(400.0).show_ui(ui, |ui| {
                if !filmcraft_text::fonts::system_scanned() {
                    filmcraft_text::fonts::scan_system();
                }
                for (f, _) in &fams {
                    if ui.selectable_label(f.eq_ignore_ascii_case(&style.font), f).clicked() {
                        set("font", json!(f));
                    }
                }
            });
            app.auto.add("captionStyle.font", r.response.rect, "Font");
        });
        let styles: Vec<String> = fams.iter().find(|(f, _)| f.eq_ignore_ascii_case(&style.font)).map(|(_, s)| s.clone()).unwrap_or_default();
        row(ui, tl!("Font Style"), |ui| {
            let r = egui::ComboBox::from_id_salt("caption-style-font-style").selected_text(&style.font_style).width(180.0).show_ui(ui, |ui| {
                let mut seen: Vec<&str> = Vec::new();
                for st in styles.iter().map(String::as_str).chain(["Bold", "Italic"]) {
                    if seen.contains(&st) {
                        continue;
                    }
                    seen.push(st);
                    if ui.selectable_label(st == style.font_style, st).clicked() {
                        set("fontStyle", json!(st));
                    }
                }
            });
            app.auto.add("captionStyle.fontStyle", r.response.rect, "Font Style");
        });
        row(ui, tl!("Size"), |ui| {
            let mut v = style.size;
            let r = ui.add(egui::DragValue::new(&mut v).range(8.0..=200.0).speed(0.5).suffix(" px"));
            app.auto.add("captionStyle.size", r.rect, "Size");
            if commit(ui, &r) {
                set("size", json!(v));
            }
            let mut up = style.uppercase;
            let r = ui.checkbox(&mut up, tl!("ALL CAPS"));
            app.auto.add("captionStyle.uppercase", r.rect, "ALL CAPS");
            if r.changed() {
                set("uppercase", json!(up));
            }
        });
        row(ui, tl!("Tracking"), |ui| {
            let mut v = style.tracking;
            let r = ui.add(egui::DragValue::new(&mut v).range(-200.0..=1000.0).speed(2.0));
            app.auto.add("captionStyle.tracking", r.rect, "Tracking");
            if commit(ui, &r) {
                set("tracking", json!(v));
            }
        });
        row(ui, tl!("Color"), |ui| {
            if let Some(c) = color(ui, style.color, false) {
                set("color", json!(c));
            }
            app.auto.add("captionStyle.color", ui.min_rect(), "Color");
        });

        // ---- outline
        heading(ui, tl!("Outline"), &t);
        row(ui, tl!("Width"), |ui| {
            let mut v = style.outline;
            let r = ui.add(egui::DragValue::new(&mut v).range(0.0..=40.0).speed(0.2).suffix(" px"));
            app.auto.add("captionStyle.outline", r.rect, "Outline width");
            if commit(ui, &r) {
                set("outline", json!(v));
            }
            if let Some(c) = color(ui, style.outline_color, false) {
                set("outlineColor", json!(c));
            }
        });

        // ---- background box
        heading(ui, tl!("Background"), &t);
        row(ui, tl!("Box"), |ui| {
            let mut on = style.background;
            let r = ui.checkbox(&mut on, "");
            app.auto.add("captionStyle.background", r.rect, "Background box");
            if r.changed() {
                set("background", json!(on));
            }
            if let Some(c) = color(ui, style.background_color, true) {
                set("backgroundColor", json!(c));
            }
        });
        row(ui, tl!("Corner Radius"), |ui| {
            let mut v = style.box_radius;
            let r = ui.add(egui::DragValue::new(&mut v).range(0.0..=100.0).speed(0.5).suffix(" px"));
            app.auto.add("captionStyle.boxRadius", r.rect, "Corner radius");
            if commit(ui, &r) {
                set("boxRadius", json!(v));
            }
        });

        // ---- shadow
        heading(ui, tl!("Shadow"), &t);
        row(ui, tl!("Shadow"), |ui| {
            let mut on = style.shadow;
            let r = ui.checkbox(&mut on, "");
            app.auto.add("captionStyle.shadow", r.rect, "Shadow");
            if r.changed() {
                set("shadow", json!(on));
            }
            if let Some(c) = color(ui, style.shadow_color, true) {
                set("shadowColor", json!(c));
            }
        });
        row(ui, tl!("Softness"), |ui| {
            let mut v = style.shadow_blur;
            let r = ui.add(egui::DragValue::new(&mut v).range(0.0..=60.0).speed(0.2).suffix(" px"));
            app.auto.add("captionStyle.shadowBlur", r.rect, "Shadow softness");
            if commit(ui, &r) {
                set("shadowBlur", json!(v));
            }
        });
        row(ui, tl!("Offset"), |ui| {
            let [mut x, mut y] = style.shadow_offset;
            let rx = ui.add(egui::DragValue::new(&mut x).range(-60.0..=60.0).speed(0.2).prefix("X "));
            app.auto.add("captionStyle.shadowX", rx.rect, "Shadow offset X");
            let ry = ui.add(egui::DragValue::new(&mut y).range(-60.0..=60.0).speed(0.2).prefix("Y "));
            app.auto.add("captionStyle.shadowY", ry.rect, "Shadow offset Y");
            if commit(ui, &rx) || commit(ui, &ry) {
                set("shadowOffset", json!([x, y]));
            }
        });

        // ---- the spoken word and how captions appear
        heading(ui, tl!("Animation"), &t);
        row(ui, tl!("Spoken Word"), |ui| {
            for (h, name, label) in [
                (CaptionHighlight::None, "none", tl!("Plain")),
                (CaptionHighlight::Color, "color", tl!("Color")),
                (CaptionHighlight::Box, "box", tl!("Box")),
            ] {
                let r = ui.selectable_label(style.highlight == h, label);
                app.auto.add(&format!("captionStyle.highlight.{name}"), r.rect, label);
                if r.clicked() {
                    set("highlight", json!(name));
                }
            }
            if let Some(c) = color(ui, style.highlight_color, false) {
                set("highlightColor", json!(c));
            }
        });
        row(ui, tl!("Spoken Word Size"), |ui| {
            let mut v = style.highlight_scale * 100.0;
            let r = ui.add(egui::DragValue::new(&mut v).range(100.0..=200.0).speed(0.5).suffix(" %"));
            app.auto.add("captionStyle.highlightScale", r.rect, "Spoken word size");
            if commit(ui, &r) {
                set("highlightScale", json!(v / 100.0));
            }
        });
        row(ui, tl!("Words at Once"), |ui| {
            for (n, label) in [(0u8, tl!("All")), (1, "1"), (2, "2"), (3, "3"), (4, "4")] {
                let r = ui.selectable_label(style.chunk_words == n, label);
                app.auto.add(&format!("captionStyle.chunkWords.{n}"), r.rect, label);
                if r.clicked() {
                    set("chunkWords", json!(n));
                }
            }
        });
        row(ui, tl!("Entrance"), |ui| {
            let names = [
                (CaptionAnimation::None, "none", tl!("None")),
                (CaptionAnimation::Fade, "fade", tl!("Fade")),
                (CaptionAnimation::Pop, "pop", tl!("Pop")),
                (CaptionAnimation::Slide, "slide", tl!("Slide Up")),
                (CaptionAnimation::Typewriter, "typewriter", tl!("Typewriter")),
            ];
            let cur = names.iter().find(|n| n.0 == style.animation).map_or("", |n| n.2);
            let r = egui::ComboBox::from_id_salt("caption-style-animation").selected_text(cur).width(160.0).show_ui(ui, |ui| {
                for (a, name, label) in names {
                    if ui.selectable_label(style.animation == a, label).clicked() {
                        set("animation", json!(name));
                    }
                }
            });
            app.auto.add("captionStyle.animation", r.response.rect, "Entrance");
        });
        ui.label(
            RichText::new(tl!("Captions made from a transcript know when each word is spoken; for other captions the timing is estimated from word lengths."))
                .size(11.0)
                .color(t.text_dim),
        );

        // ---- position
        heading(ui, tl!("Position"), &t);
        row(ui, tl!("Alignment"), |ui| {
            for (a, name, tip) in [
                (CaptionAlign::Left, "left", tl!("Align left")),
                (CaptionAlign::Center, "center", tl!("Align center")),
                (CaptionAlign::Right, "right", tl!("Align right")),
            ] {
                let r = ui.selectable_label(style.align == a, tip);
                app.auto.add(&format!("captionStyle.align.{name}"), r.rect, tip);
                if r.clicked() {
                    set("align", json!(name));
                }
            }
        });
        row(ui, tl!("Place"), |ui| {
            for (a, name, tip) in
                [(CaptionAnchor::Top, "top", tl!("Top")), (CaptionAnchor::Middle, "middle", tl!("Middle")), (CaptionAnchor::Bottom, "bottom", tl!("Bottom"))]
            {
                let r = ui.selectable_label(style.anchor == a, tip);
                app.auto.add(&format!("captionStyle.anchor.{name}"), r.rect, tip);
                if r.clicked() {
                    set("anchor", json!(name));
                }
            }
        });
        if style.position.is_some() {
            row(ui, tl!("Placed by Hand"), |ui| {
                let r = ui.small_button(tl!("Put Back"));
                app.auto.add("captionStyle.putBack", r.rect, "Put Back");
                if r.clicked() {
                    set("position", Value::Null);
                }
            });
        }
        ui.label(RichText::new(tl!("Select a caption and drag it in the Program monitor to place it anywhere.")).size(11.0).color(t.text_dim));
        row(ui, tl!("Edge Distance"), |ui| {
            let mut v = style.margin * 100.0;
            let r = ui.add(egui::DragValue::new(&mut v).range(0.0..=45.0).speed(0.2).suffix(" %"));
            app.auto.add("captionStyle.margin", r.rect, "Edge distance");
            if commit(ui, &r) {
                set("margin", json!(v / 100.0));
            }
        });
        row(ui, tl!("Line Spacing"), |ui| {
            let mut v = style.line_spacing;
            let r = ui.add(egui::DragValue::new(&mut v).range(0.8..=4.0).speed(0.01));
            app.auto.add("captionStyle.lineSpacing", r.rect, "Line spacing");
            if commit(ui, &r) {
                set("lineSpacing", json!(v));
            }
        });
        ui.add_space(10.0);
        let r = ui.button(tl!("Reset Style"));
        app.auto.add("captionStyle.reset", r.rect, "Reset Style");
        if r.clicked() {
            set("reset", json!(true));
        }
        ui.add_space(12.0);
    });
    ui.ctx().data_mut(|d| d.insert_temp(memory_id(), mem));
    run(app, ui, actions);
}

/// Whether the caption font pack is installed (checked on disk at most every 2 seconds).
/// The user's own styles: tiles like the presets (right-click: Export…, Delete), and Save /
/// Import. Hovering a tile previews it in the Program monitor, as for the presets.
#[allow(clippy::too_many_arguments)]
fn my_styles(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    style: &CaptionStyle,
    target: &Value,
    phrases: Option<&Value>,
    sample: &str,
    pack: bool,
    actions: &mut Vec<(String, Value)>,
) {
    let t = app.tokens;
    heading(ui, tl!("My Styles"), &t);
    let mine: Vec<filmcraft_engine::caption_presets::UserStyle> =
        filmcraft_engine::caption_presets::user_dir().map(|d| filmcraft_engine::caption_presets::user_styles_in(&d)).unwrap_or_default();
    if mine.is_empty() {
        ui.label(RichText::new(tl!("Save the current look to use it again in any project, or import one someone shared.")).size(11.5).color(t.text_dim));
    }
    let cols = ((ui.available_width() + 8.0) / (TILE_W + 8.0)).floor().max(1.0) as usize;
    for row in mine.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for u in row {
                let id = format!("user:{}", u.name);
                let (r, resp) = ui.allocate_exact_size(vec2(TILE_W, TILE_H + 18.0), Sense::click());
                let frame = Rect::from_min_size(r.min, vec2(TILE_W, TILE_H));
                let tex = preview_texture(ui.ctx(), &id, &u.style, sample, pack);
                ui.painter().image(tex.id(), frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                let current = u.style == *style;
                let stroke = if current {
                    Stroke::new(2.0, t.accent)
                } else if resp.hovered() {
                    Stroke::new(1.5, t.text_dim)
                } else {
                    Stroke::new(1.0, t.field_border)
                };
                ui.painter().rect_stroke(frame, 4.0, stroke, StrokeKind::Inside);
                ui.painter().text(
                    pos2(frame.center().x, frame.max.y + 9.0),
                    egui::Align2::CENTER_CENTER,
                    &u.name,
                    Tokens::ui(11.5),
                    if current { t.text } else { t.text_dim },
                );
                app.auto.add(&format!("captionStyle.preset.{id}"), r, &u.name);
                if resp.hovered() {
                    app.ui.caption_hover = Some((id.clone(), u.style.clone(), ui.ctx().cumulative_pass_nr()));
                }
                if resp.clicked() {
                    let q = match phrases {
                        Some(ids) => json!({"preset": id, "captions": ids}),
                        None => json!({"preset": id, "track": target}),
                    };
                    actions.push(("captions.applyStylePreset".into(), q));
                }
                resp.context_menu(|ui| {
                    if ui.button(tl!("Export…")).clicked() {
                        let file = format!("{}.json", u.name);
                        if let Some(path) = app.hooks.pick_save_as.as_mut().and_then(|f| f("Caption style", &["json"], &file)) {
                            actions.push(("captions.exportStylePreset".into(), json!({"preset": id, "path": path})));
                        }
                        ui.close();
                    }
                    if ui.button(tl!("Delete")).clicked() {
                        actions.push(("captions.deleteStylePreset".into(), json!({"name": u.name})));
                        ui.close();
                    }
                });
            }
        });
        ui.add_space(6.0);
    }
    let name_id = egui::Id::new("caption-style-save-name");
    let mut name: String = ui.ctx().data(|d| d.get_temp(name_id)).unwrap_or_default();
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(tl!("Name for this look")).desired_width(170.0));
        app.auto.add("captionStyle.saveName", r.rect, "Name for this look");
        let r = ui.add_enabled(!name.trim().is_empty(), egui::Button::new(tl!("Save Style")));
        app.auto.add("captionStyle.save", r.rect, "Save Style");
        if r.clicked() {
            let track = if target == "all" { json!("C1") } else { target.clone() };
            actions.push(("captions.saveStylePreset".into(), json!({"name": name.trim(), "track": track})));
            name.clear();
        }
        let r = ui.add_enabled(app.hooks.pick_open_file.is_some(), egui::Button::new(tl!("Import…")));
        app.auto.add("captionStyle.import", r.rect, "Import style");
        if r.clicked()
            && let Some(path) = app.hooks.pick_open_file.as_mut().and_then(|f| f("Caption style", &["json"]))
        {
            actions.push(("captions.importStylePreset".into(), json!({"path": path})));
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(name_id, name));
}

fn pack_installed(ctx: &egui::Context) -> bool {
    let id = egui::Id::new("caption-style-pack");
    let now = ctx.input(|i| i.time);
    if let Some((at, v)) = ctx.data(|d| d.get_temp::<(f64, bool)>(id))
        && now - at < 2.0
    {
        return v;
    }
    let v = filmcraft_engine::caption_fonts::installed();
    ctx.data_mut(|d| d.insert_temp(id, (now, v)));
    v
}

fn heading(ui: &mut egui::Ui, title: &str, t: &Tokens) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).font(Tokens::semibold(12.5)).color(t.text));
    ui.add_space(2.0);
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(LABEL_W, 20.0), egui::Label::new(RichText::new(label).size(12.0)).truncate());
        add(ui);
    });
}

/// A value field's change is applied when typing ends or a drag stops (one undo step per drag); a
/// drag ended by Escape keeps the old value (#580).
fn commit(ui: &egui::Ui, r: &egui::Response) -> bool {
    let escaped = r.drag_stopped() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    !escaped && (r.drag_stopped() || (r.changed() && !r.dragged()))
}

/// A colour button; the new colour when it changed.
fn color(ui: &mut egui::Ui, c: [u8; 4], alpha: bool) -> Option<[u8; 4]> {
    let mut col = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    let mode = if alpha { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
    let r = egui::color_picker::color_edit_button_srgba(ui, &mut col, mode);
    r.changed().then(|| col.to_srgba_unmultiplied())
}

/// A preset tile: a dark 16:9 frame with the sample caption drawn by the caption renderer, cached
/// per preset and sample text.
fn preview_texture(ctx: &egui::Context, id: &str, style: &CaptionStyle, sample: &str, pack: bool) -> egui::TextureHandle {
    // drawn again once the caption fonts are installed
    let key = egui::Id::new(("caption-style-tile", id, sample, pack));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(key)) {
        return t;
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([RENDER_W, RENDER_H], &preview_rgba(style, sample));
    let t = ctx.load_texture(format!("caption-style-{id}"), img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, t.clone()));
    t
}

/// How much larger than in a real frame a tile draws its caption (a close-up, so the style reads at
/// tile size).
const PREVIEW_ZOOM: f32 = 2.4;

/// The style with every size (text, outline, box corners, shadow) `k` times larger.
fn zoomed(style: &CaptionStyle, k: f32) -> CaptionStyle {
    CaptionStyle {
        size: style.size * k,
        outline: style.outline * k,
        box_radius: style.box_radius * k,
        shadow_offset: [style.shadow_offset[0] * k, style.shadow_offset[1] * k],
        shadow_blur: style.shadow_blur * k,
        ..style.clone()
    }
}

/// sRGB RGBA bytes of a preview frame: a dusk-coloured gradient (so light and dark styles both
/// read) with the caption composited over it.
pub fn preview_rgba(style: &CaptionStyle, sample: &str) -> Vec<u8> {
    use filmcraft_color::{linear_to_srgb, srgb_to_linear};
    let (w, h) = (RENDER_W, RENDER_H);
    let mut canvas = vec![0.0f32; w * h * 4];
    for y in 0..h {
        let f = y as f32 / h as f32;
        // top #3a4a6b → bottom #8a5a6a, opaque
        let rgb = [0.227 + (0.541 - 0.227) * f, 0.290 + (0.353 - 0.290) * f, 0.420 + (0.416 - 0.420) * f];
        for x in 0..w {
            let i = (y * w + x) * 4;
            for k in 0..3 {
                canvas[i + k] = srgb_to_linear(rgb[k]);
            }
            canvas[i + 3] = 1.0;
        }
    }
    // a second per word, shown during the second word: the highlight, chunks and typewriter show
    let sec = filmcraft_time::TICKS_PER_SECOND;
    let n = sample.split_whitespace().count() as i64;
    let cap = Caption {
        id: ClipId(0),
        start: Tick(0),
        duration: Tick(n.max(1) * sec),
        text: sample.into(),
        speaker: None,
        cue_id: None,
        settings: String::new(),
        style: None,
        words: (0..n).map(|i| Tick(i * sec)).collect(),
    };
    if let Some(o) = filmcraft_captions::burn::render_caption_at(&cap, &zoomed(style, PREVIEW_ZOOM), w, h, Some(Tick(sec + sec / 2))) {
        o.composite_onto(&mut canvas, w, h);
    }
    canvas.chunks(4).flat_map(|p| [0, 1, 2].map(|k| (linear_to_srgb(p[k].clamp(0.0, 1.0)) * 255.0).round() as u8).into_iter().chain([255])).collect()
}

fn run(app: &mut FilmcraftApp, ui: &egui::Ui, actions: Vec<(String, Value)>) {
    let ctx = ui.ctx().clone();
    for (cmd, p) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &cmd, p) {
            app.ui.status = e;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_frames_show_the_preset() {
        let presets = filmcraft_engine::caption_presets::builtin();
        let classic = preview_rgba(&presets[0].style, "Sample caption text");
        let yellow = preview_rgba(&filmcraft_engine::caption_presets::find("yellow-pop").unwrap().style, "Sample caption text");
        assert_eq!(classic.len(), RENDER_W * RENDER_H * 4);
        assert_ne!(classic, yellow);
        // yellow text pixels: red and green high, blue low
        assert!(yellow.chunks(4).filter(|p| p[0] > 230 && p[1] > 200 && p[2] < 120).count() > 200);
    }
}
