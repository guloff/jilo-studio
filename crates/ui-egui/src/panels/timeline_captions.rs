//! Caption track rows of the Timeline: drawn in their own area above the video tracks (Premiere's
//! layout). Each row has a header (C1…, name, format, output eye, lock) and caption blocks in a
//! distinct colour showing their text. Click selects, Cmd-click adds or takes one out, Shift-click
//! selects every caption from the last one clicked to this one; dragging the body moves the
//! selected captions (and the selected clips with them, by the same time), dragging an edge trims,
//! double-click opens the Text panel's Captions tab, right-click opens the caption menu. Every
//! gesture ends in one command; every element registers an automation id (`timeline.caption.*`,
//! `timeline.captionTrack.C1.*`, `timeline.captionMenu.*`).

use egui::{Align2, Color32, CursorIcon, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use filmcraft_project::{CaptionTrack, ClipId, Sequence};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use super::timeline::Layout;
use crate::FilmcraftApp;
use crate::dock::PanelKind;
use crate::icons::Icon;
use crate::theme::Tokens;

/// Height of one caption track row.
pub const ROW_H: f32 = 28.0;
/// Caption block colours (fill, selected fill).
const FILL: Color32 = Color32::from_rgb(0x6b, 0x4f, 0x8f);
const FILL_SEL: Color32 = Color32::from_rgb(0x8c, 0x6c, 0xb4);

/// Row rects (full width incl. header) for each caption track, top first.
pub fn rows(area: Rect, n: usize) -> Vec<Rect> {
    (0..n).map(|i| Rect::from_min_size(pos2(area.min.x, area.min.y + i as f32 * ROW_H), vec2(area.width(), ROW_H))).collect()
}

#[derive(Clone, Copy, Debug)]
enum Gesture {
    Move,
    TrimIn,
    TrimOut,
}

fn drag_id() -> egui::Id {
    egui::Id::new("caption-drag")
}

/// Live drag state: (caption, gesture, delta ticks).
fn drag_state(ctx: &egui::Context) -> Option<(u64, u8, i64)> {
    ctx.data(|d| d.get_temp::<(u64, u8, i64)>(drag_id()))
}

/// How far the selected captions are being moved right now: by their own drag, or by a drag of
/// the clips selected with them.
fn group_move(app: &FilmcraftApp, drag: Option<(u64, u8, i64)>) -> Option<Tick> {
    match (drag, &app.tl.drag) {
        (Some((_, 0, d)), _) => Some(Tick(d)),
        (_, Some(super::timeline::Drag::Move { offset, .. })) => Some(*offset),
        _ => None,
    }
}

fn block_rect(c: &filmcraft_project::Caption, row: Rect, layout: &Layout, drag: Option<(u64, u8, i64)>, moving: Option<Tick>, selected: bool) -> Rect {
    let (mut s, mut e) = (c.start, c.end());
    match (drag.filter(|d| d.0 == c.id.0), moving) {
        (Some((_, 1, d)), _) => s = (s + Tick(d)).min(e),
        (Some((_, 2, d)), _) => e = (e + Tick(d)).max(s),
        (Some((_, 0, d)), _) => {
            s += Tick(d);
            e += Tick(d);
        }
        (_, Some(d)) if selected => {
            s += d;
            e += d;
        }
        _ => {}
    }
    Rect::from_min_max(pos2(layout.x_of(s), row.min.y + 2.0), pos2(layout.x_of(e).max(layout.x_of(s) + 1.0), row.max.y - 2.0))
}

/// Paint headers and caption blocks.
pub fn paint(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout, t: &Tokens) {
    if seq.caption_tracks.is_empty() {
        return;
    }
    let hw = app.ui.timeline.header_w;
    let content = layout.content;
    let painter = ui.painter().with_clip_rect(area);
    let sel = app.session.state.caption_selection.clone();
    let drag = drag_state(ui.ctx());
    let moving = group_move(app, drag);
    let mut actions: Vec<(String, Value)> = Vec::new();
    for (i, (tr, row)) in seq.caption_tracks.iter().zip(rows(area, seq.caption_tracks.len())).enumerate() {
        let lane = Rect::from_min_max(pos2(content.min.x, row.min.y), pos2(content.max.x, row.max.y));
        painter.rect_filled(lane, 0.0, t.caption_track_bg);
        painter.line_segment([pos2(row.min.x, row.max.y - 0.5), pos2(row.max.x, row.max.y - 0.5)], Stroke::new(1.0, t.tl_bg));
        // header
        let label = format!("C{}", i + 1);
        let hrect = Rect::from_min_max(row.min, pos2(row.min.x + hw, row.max.y));
        painter.rect_filled(hrect, 0.0, t.tl_header_bg);
        painter.line_segment([pos2(hrect.min.x, hrect.max.y - 0.5), pos2(hrect.max.x, hrect.max.y - 0.5)], Stroke::new(1.0, t.separator));
        let badge = Rect::from_min_size(pos2(hrect.min.x + 13.0, row.min.y + 3.0), vec2(24.0, ROW_H - 6.0));
        painter.rect_filled(badge, 3.0, if tr.enabled { FILL } else { t.field_bg });
        painter.text(badge.center(), Align2::CENTER_CENTER, &label, Tokens::semibold(10.5), Color32::WHITE);
        app.auto.add(&format!("timeline.captionTrack.{label}"), badge, &tr.name);
        let lock_r = Rect::from_center_size(pos2(hrect.min.x + 49.0, row.center().y), vec2(18.0, 18.0));
        let lresp = crate::widgets::icon_toggle(
            ui,
            lock_r,
            if tr.locked { Icon::Lock } else { Icon::Unlock },
            true,
            t,
            egui::Id::new(("cap-lock", tr.id.0)),
            Some(if tr.locked { t.icon_active } else { t.text_dim }),
        );
        app.auto.add(&format!("timeline.captionTrack.{label}.locked"), lock_r, "Toggle Caption Track Lock");
        if lresp.clicked() {
            actions.push(("captions.setTrack".into(), json!({"track": tr.id.0, "locked": !tr.locked})));
        }
        let eye_r = Rect::from_center_size(pos2(hrect.min.x + 73.0, row.center().y), vec2(18.0, 18.0));
        let eresp = crate::widgets::icon_toggle(
            ui,
            eye_r,
            if tr.enabled { Icon::Eye } else { Icon::EyeOff },
            true,
            t,
            egui::Id::new(("cap-eye", tr.id.0)),
            Some(t.text_dim),
        );
        app.auto.add(&format!("timeline.captionTrack.{label}.enabled"), eye_r, "Toggle Caption Track Output");
        if eresp.clicked() {
            actions.push(("captions.setTrack".into(), json!({"track": tr.id.0, "enabled": !tr.enabled})));
        }
        let name = format!("{} · {}", tr.name, crate::i18n::t(tr.format.label()));
        painter.with_clip_rect(Rect::from_min_max(pos2(hrect.min.x + 86.0, row.min.y), pos2(hrect.max.x - 4.0, row.max.y))).text(
            pos2(hrect.min.x + 90.0, row.center().y),
            Align2::LEFT_CENTER,
            name,
            Tokens::ui(11.0),
            t.text_dim,
        );
        // caption blocks
        let p = painter.with_clip_rect(lane);
        let (v0, v1) = (layout.tick_at(content.min.x - 2.0), layout.tick_at(content.max.x + 2.0));
        for c in tr.captions.iter().filter(|c| c.end() >= v0 && c.start <= v1) {
            let selected = sel.contains(&c.id);
            let r = block_rect(c, row, layout, drag, moving, selected);
            p.rect_filled(r, 3.0, if selected { FILL_SEL } else { FILL });
            if selected {
                p.rect_stroke(r, 3.0, Stroke::new(1.0, t.clip_selected_border), StrokeKind::Inside);
            }
            if r.width() > 14.0 {
                let text = filmcraft_project::plain_text(&c.text).replace('\n', " ");
                p.with_clip_rect(r.shrink2(vec2(4.0, 0.0)).intersect(lane)).text(
                    pos2(r.min.x + 5.0, r.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    Tokens::ui(11.0),
                    if tr.enabled { Color32::WHITE } else { t.text_dim },
                );
            }
            app.auto.add(&format!("timeline.caption.{}", c.id.0), r.intersect(lane), &c.text);
        }
        if tr.locked {
            p.rect_filled(lane, 0.0, Color32::from_black_alpha(70));
        }
    }
    // column separator
    painter.line_segment([pos2(area.min.x + hw - 0.5, area.min.y), pos2(area.min.x + hw - 0.5, area.max.y)], Stroke::new(1.0, t.separator));
    run(app, ui, actions);
}

/// Pointer interaction on caption blocks (call after the timeline's own interaction so captions
/// are on top).
pub fn interact(app: &mut FilmcraftApp, ui: &mut egui::Ui, seq: &Sequence, area: Rect, layout: &Layout) {
    let ctx = ui.ctx().clone();
    let rate = seq.settings.frame_rate;
    let drag = drag_state(&ctx);
    // a caption drag that ended without a stop (its caption went away) leaves no move behind
    if drag.is_none_or(|d| d.1 != 0) {
        app.tl.caption_move = None;
    }
    let moving = group_move(app, drag);
    let mut actions: Vec<(String, Value)> = Vec::new();
    for (tr, row) in seq.caption_tracks.iter().zip(rows(area, seq.caption_tracks.len())) {
        let lane = Rect::from_min_max(pos2(layout.content.min.x, row.min.y), pos2(layout.content.max.x, row.max.y));
        for c in &tr.captions {
            let selected = app.session.state.caption_selection.contains(&c.id);
            let r = block_rect(c, row, layout, drag, moving, selected).intersect(lane);
            if r.width() <= 0.0 {
                continue;
            }
            interact_block(app, ui, seq, tr, c, r, rate, &mut actions);
        }
    }
    run(app, ui, actions);
}

/// The caption menu (right-click on a caption): groups of (label, action id). The ids are commands,
/// or `caption.*` for what the menu does itself.
pub(crate) const CAPTION_MENU: &[&[(&str, &str)]] = &[
    &[("Cut", "edit.cut"), ("Copy", "edit.copy"), ("Paste", "edit.paste")],
    &[
        ("Edit Text…", "caption.editText"),
        ("Split at Playhead", "captions.split"),
        ("Merge Captions", "captions.merge"),
        ("Split Long Captions", "captions.splitLong"),
    ],
    &[("Select All on Track", "caption.selectTrack"), ("Select to End of Track", "caption.selectForward")],
    &[("Caption Style…", "caption.style"), ("Translate Captions…", "captions.translate"), ("Export Captions…", "captions.export")],
    &[("Delete", "edit.clear"), ("Ripple Delete", "caption.rippleDelete")],
];

/// The command and parameters of a caption menu item for caption `c` on track `tr`, whether it can
/// run now, and what the menu does besides (the panel to show).
fn menu_action(app: &FilmcraftApp, seq: &Sequence, tr: &CaptionTrack, c: &filmcraft_project::Caption, id: &str) -> (String, Value, bool) {
    let st = &app.session.state;
    let playhead = app.session.playhead();
    let unlocked = !tr.locked;
    let selected: Vec<u64> = st.caption_selection.iter().map(|x| x.0).collect();
    match id {
        "caption.editText" => ("captions.goTo".into(), json!({"caption": c.id.0}), true),
        "captions.split" => ("captions.split".into(), json!({"caption": c.id.0, "time": playhead.0}), unlocked && playhead > c.start && playhead < c.end()),
        "captions.merge" => ("captions.merge".into(), json!({"captions": selected}), unlocked && selected.len() >= 2),
        "captions.splitLong" => ("captions.splitLong".into(), json!({"captions": selected}), unlocked),
        "caption.selectTrack" => ("captions.select".into(), json!({"track": tr.id.0, "all": true}), true),
        "caption.selectForward" => ("captions.select".into(), json!({"from": c.id.0}), true),
        "caption.style" => (String::new(), Value::Null, true),
        "captions.translate" => ("captions.translate".into(), json!({}), seq.caption_tracks.iter().any(|t| !t.captions.is_empty())),
        "captions.export" => ("captions.export".into(), json!({}), true),
        "caption.rippleDelete" => ("captions.delete".into(), json!({"captions": selected, "ripple": true}), unlocked && !selected.is_empty()),
        cmd => (cmd.to_string(), json!({}), app.session.is_enabled(cmd)),
    }
}

fn caption_menu(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    seq: &Sequence,
    tr: &CaptionTrack,
    c: &filmcraft_project::Caption,
    actions: &mut Vec<(String, Value)>,
) {
    ui.set_min_width(220.0);
    for (k, group) in CAPTION_MENU.iter().enumerate() {
        if k > 0 {
            ui.separator();
        }
        for &(label, id) in *group {
            let (cmd, params, enabled) = menu_action(app, seq, tr, c, id);
            let label = crate::i18n::t(label);
            let r = ui.add_enabled(enabled, egui::Button::new(label));
            app.auto.add(&format!("timeline.captionMenu.{id}"), r.rect, label);
            if r.clicked() {
                match id {
                    "caption.editText" => {
                        app.show_panel(PanelKind::Text);
                        app.ui.text_tab = "Captions".into();
                    }
                    "caption.style" => app.show_panel(PanelKind::CaptionStyle),
                    _ => {}
                }
                if !cmd.is_empty() {
                    actions.push((cmd, params));
                }
                ui.close();
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn interact_block(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    seq: &Sequence,
    tr: &CaptionTrack,
    c: &filmcraft_project::Caption,
    r: Rect,
    rate: filmcraft_time::FrameRate,
    actions: &mut Vec<(String, Value)>,
) {
    let ctx = ui.ctx().clone();
    let resp = ui.interact(r, egui::Id::new(("caption-block", c.id.0)), Sense::click_and_drag());
    let edge = 5.0f32.min(r.width() / 4.0);
    let gesture_at = |x: f32| {
        if x - r.min.x < edge {
            Gesture::TrimIn
        } else if r.max.x - x < edge {
            Gesture::TrimOut
        } else {
            Gesture::Move
        }
    };
    if let Some(p) = resp.hover_pos()
        && !tr.locked
    {
        ctx.set_cursor_icon(match gesture_at(p.x) {
            Gesture::Move => CursorIcon::Default,
            _ => CursorIcon::ResizeColumn,
        });
    }
    if resp.double_clicked() {
        app.show_panel(PanelKind::Text);
        app.ui.text_tab = "Captions".into();
        actions.push(("captions.goTo".into(), json!({"caption": c.id.0})));
        return;
    }
    if resp.clicked() {
        let m = ctx.input(|i| i.modifiers);
        let p = if m.shift {
            json!({"captions": [c.id.0], "range": true})
        } else if m.command {
            json!({"captions": [c.id.0], "toggle": true})
        } else {
            json!({"captions": [c.id.0]})
        };
        actions.push(("captions.select".into(), p));
    }
    // a right-click on a caption outside the selection selects it first
    if resp.secondary_clicked() && !app.session.state.caption_selection.contains(&c.id) {
        actions.push(("captions.select".into(), json!({"captions": [c.id.0]})));
    }
    resp.context_menu(|ui| caption_menu(app, ui, seq, tr, c, actions));
    if tr.locked {
        return;
    }
    // decide on what was under the pointer when the button went down, not where egui recognised
    // the drag (6 pt later, past the 5 px edge zones) (#259)
    if resp.drag_started()
        && let Some(p) = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        let g = match gesture_at(p.x) {
            Gesture::Move => 0u8,
            Gesture::TrimIn => 1,
            Gesture::TrimOut => 2,
        };
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("caption-drag-origin"), p.x));
        ctx.data_mut(|d| d.insert_temp(drag_id(), (c.id.0, g, 0i64)));
        if !app.session.state.caption_selection.contains(&c.id) {
            // select it now (not as a queued action) so the clips' preview knows what moves
            let _ = app.session.execute("captions.select", json!({"captions": [c.id.0]}));
        }
        if g == 0 {
            app.tl.caption_move = Some(Tick::ZERO);
        }
    }
    if resp.dragged()
        && let (Some((id, g, _)), Some(p)) = (drag_state(&ctx), resp.interact_pointer_pos())
        && id == c.id.0
    {
        let x0 = ctx.data(|d| d.get_temp::<f32>(egui::Id::new("caption-drag-origin"))).unwrap_or(p.x);
        let pps = app.ui.timeline.pps;
        let d = rate.snap_nearest(Tick::from_seconds_f64((p.x - x0) as f64 / pps));
        ctx.data_mut(|dd| dd.insert_temp(drag_id(), (id, g, d.0)));
        if g == 0 {
            app.tl.caption_move = Some(d);
        }
    }
    if resp.drag_stopped()
        && let Some((id, g, d)) = drag_state(&ctx)
        && id == c.id.0
    {
        ctx.data_mut(|dd| dd.remove::<(u64, u8, i64)>(drag_id()));
        app.tl.caption_move = None;
        // egui ends a drag on Escape: that stop abandons it (#580)
        if d != 0 && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            match g {
                0 => {
                    let sel: Vec<u64> = if app.session.state.caption_selection.contains(&ClipId(id)) {
                        app.session.state.caption_selection.iter().map(|c| c.0).collect()
                    } else {
                        vec![id]
                    };
                    // clips selected with the captions move with them, by the same time, in one step
                    let clips: Vec<Value> = app
                        .session
                        .state
                        .selection
                        .iter()
                        .filter_map(|c| seq.find_item(*c).map(|(tid, it)| json!({"clip": c.0, "track": tid.0, "time": (it.start + Tick(d)).max(Tick::ZERO).0})))
                        .collect();
                    if clips.is_empty() {
                        actions.push(("captions.move".into(), json!({"captions": sel, "delta": d})));
                    } else {
                        actions.push(("timeline.move".into(), json!({"moves": clips, "linked": false, "captions": sel})));
                    }
                }
                1 => actions.push(("captions.trim".into(), json!({"caption": id, "edge": "in", "delta": d}))),
                _ => actions.push(("captions.trim".into(), json!({"caption": id, "edge": "out", "delta": d}))),
            }
        }
    }
}

fn run(app: &mut FilmcraftApp, ui: &egui::Ui, actions: Vec<(String, Value)>) {
    let ctx = ui.ctx().clone();
    for (cmd, p) in actions {
        if let Err(e) = crate::menus::invoke(app, &ctx, &cmd, p) {
            app.ui.status = e;
        }
    }
}
