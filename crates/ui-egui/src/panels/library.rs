//! The Library panel (Window ▸ Libraries): everything that goes into an edit in one place, by kind
//! — Media, Audio, Text, Stickers, Effects, Transitions, Captions, Filters — with a search. A
//! double-click (or the "+" on a tile) puts it in: media and text at the playhead on a free track, effects and filters
//! on the selected clips, transitions on the selected cuts, caption styles on every caption.
//! Media, text and effects can be dragged onto the timeline as from the Project and Effects panels.
//!
//! Automation ids: `library.tab.<kind>`, `library.search`, `library.item.<kind>.<key>`,
//! `library.add.<kind>.<key>` (the "+").

use egui::{Align2, Rect, RichText, Sense, pos2, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The kinds, in tab order: (id, name).
pub const KINDS: [(&str, &str); 8] = [
    ("media", "Media"),
    ("audio", "Audio"),
    ("text", "Text"),
    ("stickers", "Stickers"),
    ("effects", "Effects"),
    ("transitions", "Transitions"),
    ("captions", "Captions"),
    ("filters", "Filters"),
];

#[derive(Clone, Debug, Default)]
pub struct LibraryUi {
    /// The tab shown (an id of [`KINDS`]; "" = media).
    pub kind: String,
    pub search: String,
    /// Stickers: the category shown ("" = recent, else favorites or a catalogue category) and the
    /// entrance animation new stickers get ("" = pop).
    pub sticker_category: String,
    pub sticker_animation: String,
    /// What a sticker drawn by the image engine shows.
    pub sticker_prompt: String,
}

/// The sticker categories offered first, before the catalogue's own.
const STICKER_FIRST: [(&str, &str); 2] = [("recent", "Recent"), ("favorites", "Favorites")];
const ANIMATIONS: [(&str, &str); 4] = [("pop", "Pop"), ("fade", "Fade"), ("slide", "Slide Up"), ("none", "No Animation")];

/// A sticker's picture as a texture, made once: an emoji drawn by the system font, or a shape
/// sticker's template thumbnail.
fn sticker_texture(app: &FilmcraftApp, ctx: &egui::Context, id: &str) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("sticker-tile", id));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(key)) {
        return t;
    }
    let tex = if let Some(e) = id.strip_prefix("emoji:") {
        app.session.emoji_renderer.as_ref().and_then(|r| r(e, 96).ok()).map(|(w, h, rgba)| {
            ctx.load_texture(format!("sticker-{id}"), egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), egui::TextureOptions::LINEAR)
        })
    } else if let Some(f) = id.strip_prefix("file:") {
        std::fs::read(f).ok().and_then(|b| image::load_from_memory_with_format(&b, image::ImageFormat::Png).ok()).map(|img| {
            let img = img.thumbnail(192, 192).to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            ctx.load_texture(format!("sticker-{id}"), egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR)
        })
    } else {
        filmcraft_engine::graphic_templates::find_template(&app.session, id).ok().map(|e| {
            let (w, h, rgba) = filmcraft_engine::graphic_templates::thumbnail(&e.template, 192);
            ctx.load_texture(format!("sticker-{id}"), egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba), egui::TextureOptions::LINEAR)
        })
    };
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

/// The Stickers tab: categories, the entrance animation, and the tiles; a click puts one at the
/// playhead, a right-click makes it a favourite.
fn stickers(app: &mut FilmcraftApp, ui: &mut egui::Ui, body: Rect, query: &str) {
    let t = app.tokens;
    let cat = if app.ui.library.sticker_category.is_empty() { "recent".to_string() } else { app.ui.library.sticker_category.clone() };
    let mut p = json!({"query": query});
    if query.is_empty() {
        p["category"] = json!(cat);
    }
    let list = app.session.execute("stickers.list", p).unwrap_or_default();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let mut add: Option<String> = None;
    let mut fav: Option<String> = None;
    child.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        let cats: Vec<(String, String)> = STICKER_FIRST
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .chain(list["categories"].as_array().into_iter().flatten().filter_map(|c| Some((c["id"].as_str()?.to_string(), c["name"].as_str()?.to_string()))))
            .collect();
        for (id, name) in cats {
            let r = ui.selectable_label(cat == id && query.is_empty(), crate::i18n::t(&name));
            app.auto.add(&format!("library.stickers.cat.{id}"), r.rect, &name);
            if r.clicked() {
                app.ui.library.sticker_category = id;
                app.ui.library.search.clear();
            }
        }
    });
    child.horizontal(|ui| {
        ui.label(RichText::new(tl!("Appears with")).color(t.text_dim).small());
        let a = &mut app.ui.library.sticker_animation;
        let cur = ANIMATIONS.iter().find(|x| x.0 == a.as_str()).map_or(ANIMATIONS[0].1, |x| x.1);
        let r = egui::ComboBox::from_id_salt("library.stickers.animation").width(120.0).selected_text(crate::i18n::t(cur)).show_ui(ui, |ui| {
            for (id, name) in ANIMATIONS {
                ui.selectable_value(a, id.to_string(), crate::i18n::t(name));
            }
        });
        app.auto.add("library.stickers.animation", r.response.rect, "Animation");
    });
    let mut cut = false;
    let mut draw = false;
    if cat == "mine" && query.is_empty() {
        let can = list["cutout"] == true;
        child.horizontal(|ui| {
            let b = ui.add_enabled(can, egui::Button::new(tl!("Cut Out of the Selected Clip")).small());
            app.auto.add("library.stickers.cutOut", b.rect, "Cut Out of the Selected Clip");
            cut = b.clicked();
            if !can {
                b.on_disabled_hover_text(tl!("Needs Apple Vision (macOS 14 or later)."));
            }
        });
        child.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut app.ui.library.sticker_prompt).hint_text(tl!("A sticker of… (a happy cat)")).desired_width(170.0));
            app.auto.add("library.stickers.prompt", r.rect, "Sticker description");
            let ok = can && !app.ui.library.sticker_prompt.trim().is_empty() && app.ui.ai_task.is_none();
            let b = ui.add_enabled(ok, egui::Button::new(tl!("Draw")).small());
            app.auto.add("library.stickers.draw", b.rect, "Draw");
            draw = b.clicked() || (ok && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            b.on_hover_text(tl!("The image engine of AI Settings draws it; the subject is cut out and kept in My Stickers."));
        });
    }
    // the selected sticker over a face: follow it
    let mut attach = false;
    let can_attach = app.session.face_detector.is_some()
        && app
            .session
            .active_sequence()
            .is_some_and(|q| q.video_tracks.iter().skip(1).any(|t| app.session.state.selection.iter().any(|c| t.item(*c).is_some())));
    if can_attach {
        child.horizontal(|ui| {
            let b = ui.add(egui::Button::new(tl!("Attach Selected Sticker to Face")).small());
            app.auto.add("library.stickers.followFace", b.rect, "Attach Selected Sticker to Face");
            attach = b.clicked();
        });
    }
    let items: Vec<Value> = list["stickers"].as_array().cloned().unwrap_or_default();
    egui::ScrollArea::vertical().id_salt(("library-stickers", cat.as_str())).auto_shrink([false, false]).show(&mut child, |ui| {
        if items.is_empty() {
            let text = match cat.as_str() {
                "recent" if query.is_empty() => tl!("Stickers you add show here. Pick a category above."),
                "favorites" if query.is_empty() => tl!("Right-click a sticker to keep it here."),
                "mine" if query.is_empty() => {
                    tl!("Select a photo or a video clip under the playhead and cut its subject out: it becomes a sticker of your own.")
                }
                _ => tl!("Nothing matches the search."),
            };
            ui.add_space(10.0);
            ui.add(egui::Label::new(RichText::new(text).color(t.text_dim)).wrap());
            return;
        }
        let cell = vec2(72.0, 72.0);
        let per_row = (ui.available_width() / (cell.x + 6.0)).floor().max(1.0) as usize;
        for row in items.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for v in row {
                    let id = v["id"].as_str().unwrap_or_default().to_string();
                    let name = v["name"].as_str().unwrap_or_default().to_string();
                    let (r, resp) = ui.allocate_exact_size(cell, Sense::click());
                    ui.painter().rect_filled(r, 6.0, if resp.hovered() { t.hover } else { t.field_bg });
                    match sticker_texture(app, ui.ctx(), &id) {
                        Some(tex) => {
                            let s = tex.size_vec2();
                            let k = ((cell.x - 14.0) / s.x).min((cell.y - 14.0) / s.y);
                            ui.painter().image(
                                tex.id(),
                                Rect::from_center_size(r.center(), s * k),
                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                        None => {
                            let label = v["emoji"].as_str().map_or_else(|| name.clone(), str::to_string);
                            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, egui::FontId::proportional(11.0), t.text_dim);
                        }
                    }
                    if v["favorite"] == true {
                        ui.painter().text(
                            r.right_top() + vec2(-8.0, 8.0),
                            Align2::CENTER_CENTER,
                            "★",
                            egui::FontId::proportional(11.0),
                            egui::Color32::from_rgb(250, 204, 21),
                        );
                    }
                    app.auto.add(&format!("library.sticker.{id}"), r, &name);
                    if resp.clicked() {
                        add = Some(id.clone());
                    }
                    if resp.secondary_clicked() {
                        fav = Some(id.clone());
                    }
                    resp.on_hover_text(crate::i18n::t(&name));
                }
            });
            ui.add_space(6.0);
        }
    });
    if let Some(id) = add {
        let a = if app.ui.library.sticker_animation.is_empty() { "pop".to_string() } else { app.ui.library.sticker_animation.clone() };
        app.ui.status = match app.session.execute("stickers.add", json!({"id": id, "animation": a})) {
            Ok(_) => tl!("Sticker added at the playhead; drag it in the Program monitor.").to_string(),
            Err(e) => e.to_string(),
        };
    }
    if attach {
        app.ui.status = match app.session.execute("stickers.followFace", json!({})) {
            Ok(r) => tlf!("The sticker follows the face: {n} keyframes.", n = r["keys"].as_u64().unwrap_or(0)),
            Err(e) => e.to_string(),
        };
    }
    if draw {
        let prompt = app.ui.library.sticker_prompt.trim().to_string();
        let a = if app.ui.library.sticker_animation.is_empty() { "pop".to_string() } else { app.ui.library.sticker_animation.clone() };
        let engine = app.session.prefs.ai.image_provider().id();
        crate::panels::ai_tasks::start(
            app,
            "stickers.generate",
            json!({"prompt": prompt, "animation": a, "engine": engine}),
            "Draw a Sticker",
            "the sticker's description",
        );
    }
    if cut {
        let a = if app.ui.library.sticker_animation.is_empty() { "pop".to_string() } else { app.ui.library.sticker_animation.clone() };
        app.ui.status = match app.session.execute("stickers.fromClip", json!({"animation": a})) {
            Ok(_) => tl!("Cut out: the sticker is in My Stickers and on the picture at the playhead.").to_string(),
            Err(e) => e.to_string(),
        };
    }
    if let Some(id) = fav {
        let _ = app.session.execute("stickers.favorite", json!({"id": id}));
    }
}

/// A Lumetri look's thumbnail (the preview picture graded by it), made once.
fn look_texture(ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("look-tile", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(key)) {
        return t;
    }
    let tex = filmcraft_render::lumetri_presets::presets().iter().find(|p| p.name == name).map(|p| {
        let img = filmcraft_render::lumetri_presets::thumbnail(p, 160, 90);
        ctx.load_texture(
            format!("look-{name}"),
            egui::ColorImage::from_rgba_unmultiplied([img.w, img.h], &img.over_black_rgba8()),
            egui::TextureOptions::LINEAR,
        )
    });
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

/// One entry of a tab.
struct Entry {
    /// Unique within the tab (item id, template id, effect id…).
    key: String,
    name: String,
    detail: String,
    /// A media item for a thumbnail.
    item: Option<filmcraft_project::ItemId>,
}

fn entries(app: &mut FilmcraftApp, kind: &str) -> Vec<Entry> {
    let templates: Vec<Value> = if kind == "text" {
        app.session.execute("graphics.template.list", json!({})).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
    } else {
        Vec::new()
    };
    let p = &app.session.project;
    let media = |want_video: bool| -> Vec<Entry> {
        let mut v: Vec<Entry> = p
            .items
            .iter()
            .filter(|(_, i)| matches!(i.kind, filmcraft_project::ItemKind::Media(_)) && (i.has_video() == want_video) && (want_video || i.has_audio()))
            .map(|(id, i)| {
                let d = i.duration();
                Entry {
                    key: id.0.to_string(),
                    name: i.name.clone(),
                    detail: if d > filmcraft_time::Tick::ZERO { format!("{:.1} s", d.seconds()) } else { String::new() },
                    item: Some(*id),
                }
            })
            .collect();
        v.sort_by_key(|e| e.name.to_lowercase());
        v
    };
    match kind {
        // the project's sound, then the sound library (made on this computer, free to use)
        "audio" => media(false)
            .into_iter()
            .chain(filmcraft_engine::sound_library::SOUNDS.iter().map(|(id, name, cat, sec)| Entry {
                key: format!("sfx:{id}"),
                name: crate::i18n::t(name).to_string(),
                detail: format!("{} · {:.1} s", if *cat == "music" { tl!("Music") } else { tl!("Sound effect") }, sec),
                item: None,
            }))
            .collect(),
        "text" => templates
            .iter()
            .filter_map(|t| {
                Some(Entry {
                    key: t["id"].as_str()?.to_string(),
                    name: crate::i18n::t(t["name"].as_str()?).to_string(),
                    detail: crate::i18n::t(t["category"].as_str().unwrap_or("")).to_string(),
                    item: None,
                })
            })
            .collect(),
        "effects" => filmcraft_project::effect_defs()
            .iter()
            .filter(|d| d.kind == filmcraft_project::EffectKind::Video && !d.intrinsic)
            .map(|d| Entry {
                key: d.id.to_string(),
                name: crate::i18n::t(d.name).to_string(),
                detail: d.category.last().map(|c| crate::i18n::t(c).to_string()).unwrap_or_default(),
                item: None,
            })
            .collect(),
        "transitions" => filmcraft_project::effect_defs()
            .iter()
            .filter(|d| d.kind == filmcraft_project::EffectKind::VideoTransition)
            .map(|d| Entry {
                key: d.id.to_string(),
                name: crate::i18n::t(d.name).to_string(),
                detail: d.category.last().map(|c| crate::i18n::t(c).to_string()).unwrap_or_default(),
                item: None,
            })
            .collect(),
        "captions" => filmcraft_engine::caption_presets::builtin()
            .iter()
            .map(|pr| Entry { key: pr.id.to_string(), name: crate::i18n::t(pr.name).to_string(), detail: String::new(), item: None })
            .collect(),
        "filters" => filmcraft_render::lumetri_presets::presets()
            .iter()
            .map(|pr| Entry { key: pr.name.to_string(), name: crate::i18n::t(pr.name).to_string(), detail: crate::i18n::t(pr.folder).to_string(), item: None })
            .collect(),
        "stickers" => Vec::new(),
        _ => media(true),
    }
}

/// What a double-click on `key` in `kind` does: (command, parameters).
fn action(app: &FilmcraftApp, kind: &str, key: &str) -> Option<(&'static str, Value)> {
    let at = app.session.playhead().0;
    match kind {
        "audio" if key.starts_with("sfx:") => Some(("sounds.add", json!({"id": &key[4..]}))),
        "media" | "audio" => key.parse::<u64>().ok().map(|i| ("timeline.addAtPlayhead", json!({"item": i}))),
        "text" => Some(("graphics.template.apply", json!({"template": key, "time": at}))),
        "effects" => Some(("effects.apply", json!({"effect": key}))),
        "transitions" => Some(("sequence.applyVideoTransition", json!({"effect": key}))),
        "captions" => Some(("captions.applyStylePreset", json!({"preset": key, "track": "all"}))),
        "filters" => Some(("lumetri.applyPreset", json!({"name": key}))),
        _ => None,
    }
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let kind = if app.ui.library.kind.is_empty() { "media".to_string() } else { app.ui.library.kind.clone() };
    // the tabs, wrapping on a narrow panel
    let mut top = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(rect.min + vec2(8.0, 6.0), pos2(rect.max.x - 8.0, rect.max.y))));
    top.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (id, name) in KINDS {
            let r = ui.selectable_label(kind == id, crate::i18n::t(name));
            app.auto.add(&format!("library.tab.{id}"), r.rect, name);
            if r.clicked() {
                app.ui.library.kind = id.to_string();
            }
        }
    });
    let mut q = app.ui.library.search.clone();
    let sr = Rect::from_min_size(pos2(rect.min.x + 8.0, top.min_rect().max.y + 4.0), vec2(rect.width() - 16.0, 22.0));
    let mut sc = ui.new_child(egui::UiBuilder::new().max_rect(sr));
    crate::widgets::search_field(&mut sc, &mut q, tl!("Search"), sr.width(), &t);
    app.auto.add("library.search", sr, "Search");
    app.ui.library.search = q.clone();
    let body = Rect::from_min_max(pos2(rect.min.x + 4.0, sr.max.y + 6.0), rect.max - vec2(4.0, 4.0));
    let q = q.to_lowercase();
    if kind == "stickers" {
        stickers(app, ui, body, &q);
        return;
    }
    let list: Vec<Entry> =
        entries(app, &kind).into_iter().filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q) || e.detail.to_lowercase().contains(&q)).collect();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let mut run: Option<(&'static str, Value)> = None;
    egui::ScrollArea::vertical().id_salt(("library", kind.as_str())).auto_shrink([false, false]).show(&mut child, |ui| {
        if list.is_empty() {
            let text = match kind.as_str() {
                "stickers" => tl!("Stickers are coming: animated emoji, arrows and callouts."),
                "media" | "audio" => tl!("Import media to see it here (File ▸ Import, or drop files)."),
                _ => tl!("Nothing matches the search."),
            };
            ui.add_space(12.0);
            ui.add(egui::Label::new(RichText::new(text).color(t.text_dim)).wrap());
            return;
        }
        let media = matches!(kind.as_str(), "media" | "audio" | "filters");
        let cell = if media { vec2(150.0, 112.0) } else { vec2(150.0, 46.0) };
        let per_row = ((ui.available_width()) / (cell.x + 6.0)).floor().max(1.0) as usize;
        for row in list.chunks(per_row) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for e in row {
                    let (r, resp) = ui.allocate_exact_size(cell, Sense::click_and_drag());
                    let p = ui.painter();
                    p.rect_filled(r, 5.0, if resp.hovered() { t.hover } else { t.field_bg });
                    let mut text_top = r.min.y + 6.0;
                    if media {
                        let th = Rect::from_min_size(r.min + vec2(4.0, 4.0), vec2(cell.x - 8.0, 72.0));
                        p.rect_filled(th, 3.0, t.panel_bg);
                        if kind == "filters"
                            && let Some(tex) = look_texture(ui.ctx(), &e.key)
                        {
                            let size = tex.size_vec2();
                            let s = (th.width() / size.x).min(th.height() / size.y);
                            ui.painter().image(
                                tex.id(),
                                Rect::from_center_size(th.center(), size * s),
                                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        } else if let Some(item) = e.item
                            && kind == "media"
                            && let Some((tex, size)) = app.thumbnail(ui.ctx(), item, filmcraft_time::Tick::ZERO, 160)
                        {
                            let s = (th.width() / size.x).min(th.height() / size.y);
                            let ir = Rect::from_center_size(th.center(), size * s);
                            ui.painter().image(tex, ir, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                        } else {
                            crate::icons::paint(ui.painter(), Rect::from_center_size(th.center(), vec2(22.0, 22.0)), crate::icons::Icon::Speaker, t.text_dim);
                        }
                        text_top = th.max.y + 4.0;
                    }
                    let p = ui.painter();
                    let mut job = egui::text::LayoutJob::simple(e.name.clone(), egui::FontId::proportional(12.0), t.text, cell.x - 10.0);
                    job.wrap.max_rows = 1;
                    let g = p.layout_job(job);
                    p.galley(pos2(r.min.x + 6.0, text_top), g, t.text);
                    if !e.detail.is_empty() {
                        p.text(pos2(r.min.x + 6.0, text_top + 16.0), Align2::LEFT_TOP, &e.detail, egui::FontId::proportional(10.5), t.text_dim);
                    }
                    app.auto.add(&format!("library.item.{kind}.{}", e.key), r, &e.name);
                    // "+" in the corner adds, as a double-click does
                    let plus = Rect::from_min_size(pos2(r.max.x - 24.0, r.min.y + 4.0), vec2(20.0, 20.0));
                    let presp = ui.interact(plus, egui::Id::new(("library-add", kind.as_str(), e.key.as_str())), Sense::click());
                    if resp.hovered() || presp.hovered() {
                        ui.painter().circle_filled(plus.center(), 9.0, if presp.hovered() { t.accent_hover } else { t.accent });
                        ui.painter().text(plus.center(), Align2::CENTER_CENTER, "+", egui::FontId::proportional(14.0), egui::Color32::WHITE);
                    }
                    app.auto.add(&format!("library.add.{kind}.{}", e.key), plus, &e.name);
                    if resp.double_clicked() || presp.clicked() {
                        run = action(app, &kind, &e.key);
                    }
                    if resp.drag_started() {
                        match kind.as_str() {
                            "media" | "audio" => {
                                if let Some(i) = e.item {
                                    super::start_drag_item(ui, i);
                                }
                            }
                            "text" => super::start_drag_template(ui, &e.key, &e.name),
                            "effects" | "transitions" => super::start_drag_effect(ui, &e.key),
                            _ => {}
                        }
                    }
                    // the whole edit at once
                    let all = match kind.as_str() {
                        "transitions" => Some((tl!("Apply to All Cuts"), "sequence.transitionAllCuts", json!({"effect": e.key}))),
                        "effects" => Some((tl!("Apply to All Clips"), "effects.applyToAll", json!({"effect": e.key}))),
                        "filters" => Some((tl!("Apply to All Clips"), "effects.applyToAll", json!({"preset": e.key}))),
                        _ => None,
                    };
                    let has_all = all.is_some();
                    if let Some((label, cmd, params)) = all {
                        resp.context_menu(|ui| {
                            let r = ui.button(label);
                            app.auto.add(&format!("library.all.{kind}.{}", e.key), r.rect, label);
                            if r.clicked() {
                                run = Some((cmd, params.clone()));
                                ui.close();
                            }
                        });
                    }
                    resp.on_hover_text(if has_all { tl!("Double-click to add; right-click for the whole edit") } else { tl!("Double-click to add") });
                }
            });
            ui.add_space(6.0);
        }
    });
    if let Some((cmd, p)) = run {
        app.ui.status = match app.session.execute(cmd, p) {
            Ok(_) => tl!("Added.").to_string(),
            Err(e) => e.to_string(),
        };
    }
}
