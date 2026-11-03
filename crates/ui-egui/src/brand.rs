//! The Jilo Studio brand: the product name, the icon mark and the wordmark (the owner's marks; see
//! `assets/brand/jilo/LICENSE-brand.txt`).
//!
//! Jilo Studio is based on FilmCraft by the ArtCraft team, credited in Help ▸ About and the
//! Help menu ([`UPSTREAM_NAME`]). The ArtCraft marks are not in this build: their licence keeps them
//! out of modified versions.

/// The product name shown to people.
pub const APP_NAME: &str = "Jilo Studio";

/// The project Jilo Studio is based on.
pub const UPSTREAM_NAME: &str = "FilmCraft";

const MARK: &[u8] = include_bytes!("../../../assets/brand/jilo/jilo-studio-mark-256.png");

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
