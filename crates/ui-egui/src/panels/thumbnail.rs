//! AI ▸ YouTube Thumbnail…: the frame at the playhead with a big title in one of three looks
//! (`youtube.thumbnail`), previewed in the window and saved where the user chooses (or beside the
//! exports). Move the playhead to pick another frame; Preview draws it again.
//!
//! Automation ids: `thumb.window`, `thumb.title`, `thumb.look.<bold|box|banner>`, `thumb.preview`,
//! `thumb.save`.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Default)]
pub struct ThumbUi {
    pub open: bool,
    pub title: String,
    pub look: String,
    /// The preview's PNG and its texture.
    pub preview: Option<(String, egui::TextureHandle)>,
}

impl std::fmt::Debug for ThumbUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbUi").field("open", &self.open).field("title", &self.title).field("look", &self.look).finish_non_exhaustive()
    }
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    let seq = app.session.state.active_sequence.ok_or("no sequence is open")?;
    let u = &mut app.ui.thumbnail;
    if u.title.is_empty() {
        u.title = filmcraft_engine::youtube::texts_of(&app.session, seq)
            .and_then(|t| t.titles.first().cloned())
            .or_else(|| app.session.project.item(seq).map(|i| i.name.clone()))
            .unwrap_or_default();
    }
    if u.look.is_empty() {
        u.look = "bold".into();
    }
    u.open = true;
    u.preview = None;
    Ok(json!({"window": "thumbnail"}))
}

fn draw(app: &mut FilmcraftApp, ctx: &egui::Context, path: &str) -> Result<(), String> {
    let u = &app.ui.thumbnail;
    app.session.execute("youtube.thumbnail", json!({"title": u.title, "look": u.look, "path": path})).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|e| e.to_string())?.to_rgba8();
    let tex = ctx.load_texture(
        "yt-thumbnail",
        egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()),
        egui::TextureOptions::LINEAR,
    );
    app.ui.thumbnail.preview = Some((path.to_string(), tex));
    Ok(())
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.thumbnail.open {
        return;
    }
    let tmp = std::env::temp_dir().join(format!("jilo-thumbnail-{}.png", std::process::id())).to_string_lossy().to_string();
    if app.ui.thumbnail.preview.is_none()
        && let Err(e) = draw(app, ctx, &tmp)
    {
        app.ui.status = e;
        app.ui.thumbnail.open = false;
        return;
    }
    let t = app.tokens;
    let mut open = true;
    let (mut again, mut save) = (false, false);
    crate::dialog_style::Window::new(tl!("YouTube Thumbnail")).id(egui::Id::new("yt-thumbnail")).open(&mut open).collapsible(false).resizable(false).show(
        ctx,
        |ui| {
            ui.set_width(520.0);
            app.auto.add("thumb.window", ui.min_rect(), "YouTube Thumbnail");
            if let Some((_, tex)) = &app.ui.thumbnail.preview {
                ui.add(egui::Image::new((tex.id(), vec2(512.0, 288.0))));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(tl!("Title"));
                let r = ui.add(egui::TextEdit::singleline(&mut app.ui.thumbnail.title).desired_width(380.0));
                app.auto.add("thumb.title", r.rect, "Title");
                again |= r.lost_focus();
            });
            ui.horizontal(|ui| {
                for (id, name) in filmcraft_engine::thumbnail::LOOKS {
                    let r = ui.selectable_label(app.ui.thumbnail.look == id, crate::i18n::t(name));
                    app.auto.add(&format!("thumb.look.{id}"), r.rect, name);
                    if r.clicked() {
                        app.ui.thumbnail.look = id.to_string();
                        again = true;
                    }
                }
            });
            ui.label(RichText::new(tl!("The frame at the playhead: move it to pick another, then Preview.")).small().color(t.text_dim));
            crate::dialog_style::actions(ui, |ui| {
                let s = ui.add(crate::dialog_style::primary(tl!("Save…")));
                app.auto.add("thumb.save", s.rect, "Save");
                save = s.clicked();
                let p = ui.add(crate::dialog_style::secondary(tl!("Preview")));
                app.auto.add("thumb.preview", p.rect, "Preview");
                again |= p.clicked();
            });
        },
    );
    if again && let Err(e) = draw(app, ctx, &tmp) {
        app.ui.status = e;
    }
    if save {
        let name = format!(
            "{} thumbnail.png",
            app.session.state.active_sequence.and_then(|s| app.session.project.item(s)).map_or("Sequence".to_string(), |i| i.name.clone())
        );
        let dest = match app.hooks.pick_save_as.as_mut() {
            Some(f) => f("PNG", &["png"], &name),
            None => Some(filmcraft_engine::export_tools::default_export_dir(&app.session).join(&name).to_string_lossy().to_string()),
        };
        if let Some(d) = dest {
            app.ui.status = match draw(app, ctx, &d) {
                Ok(()) => tlf!("Thumbnail saved: {path}", path = d),
                Err(e) => e,
            };
        }
    }
    if !open {
        app.ui.thumbnail.open = false;
        app.ui.thumbnail.preview = None;
    }
}
