//! Quick actions: when something is selected, a row of the actions that fit it appears at the bottom
//! of the Program monitor's picture, as in phone editors — a video clip (split at the playhead,
//! speed, reframe to face, auto zoom, delete), an audio clip (split, volume, delete), a graphic
//! (its properties), captions (style, translate, delete). Hidden while playing and when nothing is
//! selected; View ▸ Quick Actions turns it off. Every button runs the same command as its menu item.
//!
//! Automation ids: `quick.<command id>`.

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use filmcraft_project::ItemKind;
use serde_json::json;

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// What the selection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Video,
    Audio,
    Graphic,
    Captions,
}

fn selection_kind(app: &FilmcraftApp) -> Option<Kind> {
    let s = &app.session;
    if !s.state.caption_selection.is_empty() {
        return Some(Kind::Captions);
    }
    let seq = s.active_sequence()?;
    let c = *s.state.selection.first()?;
    let (track, it) = seq.find_item(c)?;
    if matches!(s.project.item(it.item).map(|i| &i.kind), Some(ItemKind::Graphic { .. })) {
        return Some(Kind::Graphic);
    }
    Some(if seq.audio_tracks.iter().any(|t| t.id == track) { Kind::Audio } else { Kind::Video })
}

/// The actions for a selection: (command id, label).
fn actions(kind: Kind) -> &'static [(&'static str, &'static str)] {
    match kind {
        Kind::Video => &[
            ("sequence.addEdit", "Split"),
            ("clip.speedDuration", "Speed"),
            ("clip.reframeToFace", "Reframe"),
            ("clip.autoZoom", "Auto Zoom"),
            ("edit.rippleDelete", "Delete"),
        ],
        Kind::Audio => {
            &[("sequence.addEdit", "Split"), ("clip.audioGain", "Volume"), ("window.panel.EssentialSound", "Sound"), ("edit.rippleDelete", "Delete")]
        }
        Kind::Graphic => &[("window.panel.EssentialGraphics", "Edit Graphic"), ("sequence.addEdit", "Split"), ("edit.rippleDelete", "Delete")],
        Kind::Captions => &[("graphics.captionStyles", "Caption Style"), ("captions.translate", "Translate"), ("captions.delete", "Delete")],
    }
}

/// Draw the row over the bottom of `area` (the visible picture).
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, area: Rect) {
    if !app.ui.quick_actions || app.playback.playing {
        return;
    }
    let Some(kind) = selection_kind(app) else { return };
    let list = actions(kind);
    let font = Tokens::semibold(12.0);
    let pad = 10.0;
    let widths: Vec<f32> =
        list.iter().map(|(_, l)| ui.painter().layout_no_wrap(crate::i18n::t(l).to_string(), font.clone(), Color32::WHITE).size().x + 2.0 * pad).collect();
    let total = widths.iter().sum::<f32>() + 4.0 * (list.len() as f32 - 1.0) + 8.0;
    let h = 28.0;
    if total > area.width() - 8.0 || area.height() < 3.0 * h {
        return;
    }
    let bar = Rect::from_center_size(pos2(area.center().x, area.max.y - h / 2.0 - 10.0), vec2(total, h + 8.0));
    ui.painter().rect_filled(bar, 18.0, Color32::from_black_alpha(170));
    let mut x = bar.min.x + 4.0;
    let mut run: Option<&str> = None;
    for ((id, label), w) in list.iter().zip(&widths) {
        let r = Rect::from_min_size(pos2(x, bar.min.y + 4.0), vec2(*w, h));
        let enabled =
            id.starts_with("window.") || id.starts_with("graphics.") || filmcraft_engine::find_command(id).is_none_or(|c| (c.enabled)(&app.session).is_ok());
        let resp = ui.interact(r, egui::Id::new(("quick-action", *id)), if enabled { Sense::click() } else { Sense::hover() });
        let shown = crate::i18n::t(label);
        app.auto.add(&format!("quick.{id}"), r, shown);
        if resp.hovered() && enabled {
            ui.painter().rect_filled(r, 14.0, Color32::from_white_alpha(28));
        }
        let col = if enabled { Color32::WHITE } else { Color32::from_white_alpha(90) };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, shown, font.clone(), col);
        if resp.clicked() {
            run = Some(id);
        }
        x += w + 4.0;
    }
    if let Some(id) = run {
        let ctx = ui.ctx().clone();
        if let Err(e) = crate::menus::invoke(app, &ctx, id, json!({})) {
            app.ui.status = e;
        }
    }
}
