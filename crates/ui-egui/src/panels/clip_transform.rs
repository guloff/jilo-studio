//! Direct manipulation in the Program monitor for the selected picture clip that is not a graphic
//! — a sticker, a photo, a video over another: a box around it at the playhead; dragging inside
//! moves it (Motion ▸ Position), dragging a corner scales it about its centre (Motion ▸ Scale).
//! A drag is one undo step. An animated parameter gets a keyframe at the playhead. Graphics keep
//! their own layer handles (`panels::graphics`).
//!
//! Automation ids: `program.clip.<id>.box`, `program.clip.<id>.corner.<0-3>`.

use egui::{Pos2, Rect, Sense, Stroke, pos2, vec2};
use filmcraft_geom::{Affine, Vec2};
use filmcraft_project::{ItemKind, TrackItem};
use serde_json::json;

use crate::FilmcraftApp;

/// The selected clip to manipulate: one picture clip (not a graphic), under the playhead.
fn target(app: &FilmcraftApp) -> Option<(TrackItem, (u32, u32))> {
    let q = app.session.active_sequence()?;
    let sel = &app.session.state.selection;
    let t = app.session.playhead();
    let videos: Vec<&TrackItem> = q.video_tracks.iter().flat_map(|tr| tr.items.iter()).filter(|i| sel.contains(&i.id)).collect();
    let [it] = videos.as_slice() else { return None };
    if !it.range().contains(t) {
        return None;
    }
    let item = app.session.project.item(it.item)?;
    if !matches!(item.kind, ItemKind::Media(_)) || !item.has_video() {
        return None;
    }
    let src = app.session.source(it.item)?;
    let v = src.info().video.as_ref()?;
    Some(((*it).clone(), (v.width, v.height)))
}

fn sp(a: &Affine, x: f64, y: f64) -> Pos2 {
    let v = a.apply(Vec2::new(x, y));
    pos2(v.x as f32, v.y as f32)
}

/// The drag in progress: the corner (None: a move), where it started in frame pixels, and the
/// scale, position and centre then.
#[derive(Clone, Copy)]
struct Drag {
    corner: Option<usize>,
    start: (f64, f64),
    scale: f64,
    position: (f64, f64),
    centre: (f64, f64),
}

pub fn monitor_overlay(app: &mut FilmcraftApp, ui: &mut egui::Ui, pic: Rect, frame: (u32, u32)) {
    if app.ui.tool != crate::state::Tool::Selection {
        return;
    }
    let Some((it, size)) = target(app) else { return };
    let Some(seq) = app.session.active_sequence() else { return };
    let t = app.session.playhead();
    let mt = it.effect_time_at(t);
    let screen = crate::panels::graphics::frame_to_screen(pic, frame);
    let motion = filmcraft_render::motion_matrix(seq, &it, size, None, it.source_time_at(t));
    let to_screen = screen.then_apply(&motion);
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let quad = [sp(&to_screen, 0.0, 0.0), sp(&to_screen, w, 0.0), sp(&to_screen, w, h), sp(&to_screen, 0.0, h)];
    let t_ = app.tokens;
    let painter = ui.painter().with_clip_rect(pic.expand(12.0));
    painter.add(egui::Shape::closed_line(quad.to_vec(), Stroke::new(1.0, t_.accent)));
    let Some(from_screen) = screen.inverse() else { return };
    let to_frame = |p: Pos2| {
        let v = from_screen.apply(Vec2::new(f64::from(p.x), f64::from(p.y)));
        (v.x, v.y)
    };
    let m = it.effect("motion");
    let scale = m.map_or(100.0, |e| e.f64_at("scale", mt));
    let pv = m.map(|e| e.vec2_at("position", mt));
    let position = pv.map_or((f64::from(seq.settings.width) / 2.0, f64::from(seq.settings.height) / 2.0), |p| (p.x, p.y));
    let c = motion.apply(Vec2::new(w / 2.0, h / 2.0));
    let centre = (c.x, c.y);
    let mem = egui::Id::new(("clip-transform", it.id.0));
    let mut drag: Option<Drag> = ui.ctx().data(|d| d.get_temp(mem));
    let mut changes: Vec<(&str, serde_json::Value, bool)> = Vec::new();
    // the body: move
    let body = Rect::from_points(&quad).intersect(pic);
    let bresp = ui.interact(body, mem.with("body"), Sense::drag());
    app.auto.add(&format!("program.clip.{}.box", it.id.0), body, "clip box");
    let mut handles = Vec::new();
    for (n, c) in quad.iter().enumerate() {
        let hr = Rect::from_center_size(*c, vec2(9.0, 9.0));
        painter.rect_filled(hr, 0.0, egui::Color32::WHITE);
        painter.rect_stroke(hr, 0.0, Stroke::new(1.0, t_.accent), egui::StrokeKind::Inside);
        let r = ui.interact(hr.expand(3.0), mem.with(("corner", n)), Sense::drag());
        app.auto.add(&format!("program.clip.{}.corner.{n}", it.id.0), hr, "scale handle");
        handles.push(r);
    }
    for (n, r) in handles.iter().enumerate() {
        if r.drag_started()
            && let Some(p) = r.interact_pointer_pos()
        {
            drag = Some(Drag { corner: Some(n), start: to_frame(p), scale, position, centre });
        }
    }
    if bresp.drag_started()
        && drag.is_none()
        && let Some(p) = bresp.interact_pointer_pos()
    {
        drag = Some(Drag { corner: None, start: to_frame(p), scale, position, centre });
    }
    let dragging = bresp.dragged() || handles.iter().any(egui::Response::dragged);
    let stopped = bresp.drag_stopped() || handles.iter().any(egui::Response::drag_stopped);
    if let Some(d) = drag
        && dragging
        && let Some(p) = ui.ctx().pointer_interact_pos()
    {
        let now = to_frame(p);
        match d.corner {
            None => {
                let pos = (d.position.0 + now.0 - d.start.0, d.position.1 + now.1 - d.start.1);
                changes.push(("position", json!([pos.0.round(), pos.1.round()]), true));
            }
            Some(_) => {
                let r0 = ((d.start.0 - d.centre.0).powi(2) + (d.start.1 - d.centre.1).powi(2)).sqrt().max(1.0);
                let r1 = ((now.0 - d.centre.0).powi(2) + (now.1 - d.centre.1).powi(2)).sqrt();
                changes.push(("scale", json!(((d.scale * r1 / r0) * 10.0).round() / 10.0), true));
            }
        }
    }
    let first = drag.is_some() && !ui.ctx().data(|d| d.get_temp::<bool>(mem.with("begun")).unwrap_or(false));
    for (param, value, _) in changes {
        let animated = m.and_then(|e| e.param(param)).is_some_and(|p| p.is_animated());
        let mut q = json!({"clip": it.id.0, "effect": "motion", "param": param, "value": value, "merge": true, "begin": first});
        if animated {
            q["time"] = json!(t.0);
            q["keyframe"] = json!(true);
        }
        if let Err(e) = app.session.execute("effects.setParam", q) {
            app.ui.status = e.to_string();
        }
        ui.ctx().data_mut(|d| d.insert_temp(mem.with("begun"), true));
    }
    if stopped {
        drag = None;
        ui.ctx().data_mut(|d| d.insert_temp(mem.with("begun"), false));
    }
    match drag {
        Some(d) => {
            ui.ctx().data_mut(|x| x.insert_temp(mem, d));
        }
        None => ui.ctx().data_mut(|x| x.remove::<Drag>(mem)),
    }
}
