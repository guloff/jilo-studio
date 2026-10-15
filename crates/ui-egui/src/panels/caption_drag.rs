//! Captions placed by hand in the Program monitor: the selected caption under the playhead gets a
//! box; dragging it moves the caption block anywhere in the frame (`captions.setStyle` `position`,
//! as fractions of the frame). A caption with a style of its own moves alone; otherwise the
//! whole track moves. A drag is one undo step. Choosing Top / Middle / Bottom in the Caption Style
//! panel puts the block back in its place.
//!
//! Automation id: `program.caption.<id>.box`.

use egui::{Rect, Sense, Stroke, pos2};
use filmcraft_geom::Vec2;
use filmcraft_project::{Caption, CaptionStyle};
use serde_json::json;

use crate::FilmcraftApp;

/// The selected caption under the playhead: its track, the caption and the style it draws in.
fn target(app: &FilmcraftApp) -> Option<(u64, Caption, CaptionStyle)> {
    let q = app.session.active_sequence()?;
    let t = app.session.playhead();
    let sel = &app.session.state.caption_selection;
    q.caption_tracks.iter().filter(|tr| tr.enabled && !tr.locked).find_map(|tr| {
        let c = tr.caption_at(t).filter(|c| sel.contains(&c.id))?;
        Some((tr.id.0, c.clone(), c.style_or(&tr.style).clone()))
    })
}

/// The caption block's rectangle in frame pixels, measured once per look and kept.
fn block(ctx: &egui::Context, c: &Caption, style: &CaptionStyle, frame: (u32, u32)) -> Option<Rect> {
    let key = egui::Id::new(("caption-block", c.id.0));
    type Kept = (String, CaptionStyle, (u32, u32), Option<Rect>);
    if let Some((text, st, f, r)) = ctx.data(|d| d.get_temp::<Kept>(key))
        && text == c.text
        && st == *style
        && f == frame
    {
        return r;
    }
    let r = filmcraft_captions::burn::render_caption(c, style, frame.0 as usize, frame.1 as usize)
        .map(|o| Rect::from_min_size(pos2(o.x as f32, o.y as f32), egui::vec2(o.w as f32, o.h as f32)));
    ctx.data_mut(|d| d.insert_temp::<Kept>(key, (c.text.clone(), style.clone(), frame, r)));
    r
}

/// The drag in progress: where it started in frame pixels and the block's centre then (fractions).
#[derive(Clone, Copy)]
struct Drag {
    start: (f64, f64),
    centre: [f32; 2],
}

pub fn monitor_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    if app.ui.tool != crate::state::Tool::Selection || frame.0 == 0 || frame.1 == 0 {
        return;
    }
    let Some((track, c, style)) = target(app) else { return };
    let Some(r) = block(ui.ctx(), &c, &style, frame) else { return };
    let screen = crate::panels::graphics::frame_to_screen(pic, frame);
    let sp = |x: f32, y: f32| {
        let v = screen.apply(Vec2::new(f64::from(x), f64::from(y)));
        pos2(v.x as f32, v.y as f32)
    };
    let on_screen = Rect::from_two_pos(sp(r.min.x, r.min.y), sp(r.max.x, r.max.y));
    let t = app.tokens;
    ui.painter().with_clip_rect(pic.expand(4.0)).rect_stroke(on_screen, 2.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Outside);
    let mem = egui::Id::new(("caption-drag", c.id.0));
    let resp = ui.interact(on_screen.intersect(pic), mem, Sense::drag());
    app.auto.add(&format!("program.caption.{}.box", c.id.0), on_screen, "caption box");
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    let Some(from_screen) = screen.inverse() else { return };
    let to_frame = |p: egui::Pos2| {
        let v = from_screen.apply(Vec2::new(f64::from(p.x), f64::from(p.y)));
        (v.x, v.y)
    };
    let mut drag: Option<Drag> = ui.ctx().data(|d| d.get_temp(mem));
    let (fw, fh) = (frame.0 as f32, frame.1 as f32);
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let centre = style.position.unwrap_or([r.center().x / fw, r.center().y / fh]);
        drag = Some(Drag { start: to_frame(p), centre });
        ui.ctx().data_mut(|d| d.insert_temp(mem.with("begun"), false));
    }
    if let Some(d) = drag
        && resp.dragged()
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        let now = to_frame(p);
        let pos = [(d.centre[0] + (now.0 - d.start.0) as f32 / fw).clamp(0.0, 1.0), (d.centre[1] + (now.1 - d.start.1) as f32 / fh).clamp(0.0, 1.0)];
        let pos = [(pos[0] * 1000.0).round() / 1000.0, (pos[1] * 1000.0).round() / 1000.0];
        let first = !ui.ctx().data(|x| x.get_temp::<bool>(mem.with("begun")).unwrap_or(false));
        let own = app.session.active_sequence().and_then(|q| q.caption_tracks.iter().find_map(|tr| tr.caption(c.id))).is_some_and(|x| x.style.is_some());
        let mut q = json!({"position": pos, "merge": true, "begin": first});
        if own {
            q["captions"] = json!([c.id.0]);
        } else {
            q["track"] = json!(track);
        }
        if let Err(e) = app.session.execute("captions.setStyle", q) {
            app.ui.status = e.to_string();
        }
        ui.ctx().data_mut(|x| x.insert_temp(mem.with("begun"), true));
    }
    if resp.drag_stopped() {
        drag = None;
    }
    match drag {
        Some(d) => {
            ui.ctx().data_mut(|x| x.insert_temp(mem, d));
        }
        None => ui.ctx().data_mut(|x| x.remove::<Drag>(mem)),
    }
}
