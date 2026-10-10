//! The Jilo Studio brand: the product name, the icon mark and the wordmark (the owner's marks; see
//! `assets/brand/jilo/LICENSE-brand.txt`).
//!
//! Jilo Studio is based on FilmCraft by the ArtCraft team. The ArtCraft marks are not in this build:
//! their licence (`docs/brand/LICENSE-brand.txt`) keeps them out of modified versions. The interface
//! strings still say "FilmCraft" where the upstream code and catalogs do; [`rebrand`] shows them
//! with this product's name (strings that name the ArtCraft team speak about the original project
//! and keep it), so upstream strings and translations merge unchanged.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

/// The product name shown to people.
pub const APP_NAME: &str = "Jilo Studio";

/// The project Jilo Studio is based on.
pub const UPSTREAM_NAME: &str = "FilmCraft";

const MARK: &[u8] = include_bytes!("../../../assets/brand/jilo/jilo-studio-mark-256.png");

/// `s` with the upstream product name replaced by [`APP_NAME`]. Strings that mention the ArtCraft
/// team (credits for the original project) are left as written. Each distinct rebranded string is
/// made once and kept for the life of the process (the interface has a fixed set of them).
pub fn rebrand(s: &str) -> &str {
    if !s.contains(UPSTREAM_NAME) || s.contains("ArtCraft") {
        return s;
    }
    static CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(v) = cache.get(s) {
        return v;
    }
    let v: &'static str = Box::leak(s.replace(UPSTREAM_NAME, APP_NAME).into_boxed_str());
    cache.insert(s.to_string(), v);
    v
}

/// The catalog wording of a string [`rebrand`] produced (the product name back to upstream's).
pub fn unbrand(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(APP_NAME) { std::borrow::Cow::Owned(s.replace(APP_NAME, UPSTREAM_NAME)) } else { std::borrow::Cow::Borrowed(s) }
}

/// The icon mark, decoded once and kept as a texture.
pub fn mark(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("jilo-mark");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory_with_format(MARK, image::ImageFormat::Png).ok()?.to_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let tex = ctx.load_texture("jilo-mark", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// Draw the wordmark (the icon, then "Jilo Studio") `height` points tall with its left edge at
/// `left_center` (vertically centred), with light ink on dark themes (`dark = true`). Returns the
/// drawn rect.
pub fn paint_wordmark(ui: &egui::Ui, left_center: egui::Pos2, height: f32, dark: bool) -> Option<egui::Rect> {
    let tex = mark(ui.ctx())?;
    // the icon's plate fills 80 % of its canvas (the macOS icon grid): the plate is `height` tall
    let side = height / 0.8;
    let icon = egui::Rect::from_center_size(egui::pos2(left_center.x + height / 2.0, left_center.y), egui::vec2(side, side));
    ui.painter().image(tex.id(), icon, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    let ink = if dark { egui::Color32::from_rgb(0xf2, 0xf2, 0xf2) } else { egui::Color32::from_rgb(0x0a, 0x0a, 0x0a) };
    let g = ui.painter().layout_no_wrap(APP_NAME.to_string(), crate::theme::Tokens::semibold(height * 0.9), ink);
    let pos = egui::pos2(left_center.x + height * 1.45, left_center.y - g.size().y / 2.0);
    let right = pos.x + g.size().x;
    ui.painter().galley(pos, g, ink);
    Some(egui::Rect::from_min_max(egui::pos2(left_center.x, left_center.y - height / 2.0), egui::pos2(right, left_center.y + height / 2.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_name_is_shown_as_jilo_studio_except_in_credits() {
        assert_eq!(rebrand("About FilmCraft"), "About Jilo Studio");
        assert_eq!(rebrand("FilmCraft Project"), "Jilo Studio Project");
        // each distinct string is made once
        assert!(std::ptr::eq(rebrand("Quit FilmCraft"), rebrand("Quit FilmCraft")));
        assert_eq!(rebrand("Based on FilmCraft by the ArtCraft team"), "Based on FilmCraft by the ArtCraft team");
        assert_eq!(rebrand("filmcraft-cli and FILMCRAFT_DATA_DIR"), "filmcraft-cli and FILMCRAFT_DATA_DIR");
        assert_eq!(unbrand(rebrand("Selection as FilmCraft Project…")), "Selection as FilmCraft Project…");
    }
}
