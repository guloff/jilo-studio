//! Clip ▸ Reframe to Face…: whom the frame follows on the selected clips — the speaker, everyone in
//! frame, or a face picked on the frame at the playhead (`clip.facesAt`) — where the face sits, the
//! zoom and how smoothly the camera moves. The faces are found on this computer (Apple Vision) as a
//! job in the status bar and the Progress panel; when it ends, the framing is applied
//! (`clip.reframeToFace`): Position and Scale keyframes in Effect Controls, one undo step.

use egui::{Color32, RichText, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::state::FacePicker;

/// The selected video clips.
fn selected(app: &FilmcraftApp) -> Vec<u64> {
    let Some(q) = app.session.active_sequence() else { return Vec::new() };
    app.session.state.selection.iter().filter(|c| q.find_item(**c).is_some_and(|(tid, _)| q.video_tracks.iter().any(|t| t.id == tid))).map(|c| c.0).collect()
}

/// Open the dialog (refused, with the reason, without face detection or selected video clips).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("clip.reframeToFace").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    if app.session.face_detector.is_none() {
        return Err("face detection is not available on this system (it needs Apple Vision on macOS)".into());
    }
    if selected(app).is_empty() {
        return Err("select the video clips to reframe".into());
    }
    if app.ui.reframe.job.is_some() {
        return Err("faces are already being found".into());
    }
    app.ui.reframe.picker = None;
    app.ui.reframe.dialog = true;
    Ok(json!({"dialog": "reframe"}))
}

fn done_message(r: &Value) -> String {
    let mut m = tlf!("Reframed {clips} clips: {keys} keyframes", clips = r["clips"].as_u64().unwrap_or(0), keys = r["keys"].as_u64().unwrap_or(0));
    let none: Vec<&str> = r["noFaces"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !none.is_empty() {
        m = format!("{m}. {}", tlf!("No faces in {names}", names = none.join(", ")));
    }
    m
}

/// Apply a framing: at once when the faces are known, else after the background analysis.
fn run(app: &mut FilmcraftApp, params: Value) {
    let mut bg = params.clone();
    bg["background"] = json!(true);
    match app.session.execute("clip.reframeToFace", bg) {
        Ok(r) if r["background"] == true => {
            app.ui.reframe.job = r["job"].as_u64().map(|j| (j, params));
            app.ui.status = tl!("Finding faces…").to_string();
        }
        Ok(r) => app.ui.status = done_message(&r),
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// The dialog, and the framing when the analysis it started ends.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if let Some((id, params)) = app.ui.reframe.job.clone() {
        match app.session.jobs.iter().find(|j| j.id == id).map(|j| j.to_json()) {
            Some(j) if j["finished"] != true => {}
            Some(j) if j["result"]["error"].is_string() => {
                app.ui.reframe.job = None;
                app.ui.status = j["result"]["error"].as_str().unwrap_or_default().to_string();
            }
            _ => {
                app.ui.reframe.job = None;
                app.ui.status = match app.session.execute("clip.reframeToFace", params) {
                    Ok(r) => done_message(&r),
                    Err(e) => e.to_string(),
                };
            }
        }
    }
    if app.ui.reframe.dialog {
        dialog(app, ctx);
    }
}

/// The faces at the playhead, found again when the playhead has moved.
fn load_picker(app: &mut FilmcraftApp) {
    let at = app.session.playhead().0;
    if app.ui.reframe.picker.as_ref().is_some_and(|p| p.at == at) {
        return;
    }
    let mut p = FacePicker { at, ..Default::default() };
    match app.session.execute("clip.facesAt", json!({"clips": selected(app)})) {
        Ok(r) => {
            p.clip = r["clip"].as_u64().unwrap_or(0);
            p.item = r["item"].as_u64().unwrap_or(0);
            p.time = r["time"].as_i64().unwrap_or(0);
            p.faces = r["faces"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| [f["x"].as_f64().unwrap_or(0.0), f["y"].as_f64().unwrap_or(0.0), f["w"].as_f64().unwrap_or(0.0), f["h"].as_f64().unwrap_or(0.0)])
                .collect();
            // the biggest face is picked to start with
            p.picked = (0..p.faces.len()).max_by(|a, b| (p.faces[*a][2] * p.faces[*a][3]).total_cmp(&(p.faces[*b][2] * p.faces[*b][3])));
            if p.faces.is_empty() {
                p.error = Some(tl!("No faces on the frame at the playhead. Move the playhead to a frame with the face.").to_string());
            }
        }
        Err(e) => p.error = Some(e.to_string()),
    }
    app.ui.reframe.picker = Some(p);
}

/// The frame the faces were found on, as a texture (kept while the dialog shows it).
fn frame_texture(app: &FilmcraftApp, ctx: &egui::Context, item: u64, time: i64) -> Option<egui::TextureHandle> {
    let key = egui::Id::new(("reframe-frame", item, time));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(key)) {
        return t;
    }
    let tex = app.session.source(filmcraft_project::ItemId(item)).and_then(|src| {
        let w = src.info().video.as_ref().map_or(1920, |v| v.width).max(1);
        let f = src.video_frame(filmcraft_media::FrameRequest { time: filmcraft_time::Tick(time), scale: (900.0 / w as f32).min(1.0) }).ok()?;
        let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.to_rgba8());
        Some(ctx.load_texture(format!("reframe-frame-{item}-{time}"), img, egui::TextureOptions::LINEAR))
    });
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

/// The frame with its faces boxed; a click picks the face nearest to it.
fn picker(app: &mut FilmcraftApp, ui: &mut egui::Ui, width: f32) {
    let t = app.tokens;
    let Some(p) = app.ui.reframe.picker.clone() else { return };
    if let Some(e) = &p.error {
        ui.add(egui::Label::new(RichText::new(e).color(t.hot_text).small()).wrap());
        return;
    }
    let tex = frame_texture(app, ui.ctx(), p.item, p.time);
    let aspect = tex.as_ref().map_or(16.0 / 9.0, |x| x.aspect_ratio());
    let size = if width / aspect > 240.0 { vec2(240.0 * aspect, 240.0) } else { vec2(width, width / aspect) };
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let r = ui.painter_at(rect);
    match &tex {
        Some(tex) => {
            r.image(tex.id(), rect, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            r.rect_filled(rect, 2.0, t.panel_bg);
        }
    }
    let boxes: Vec<egui::Rect> = p
        .faces
        .iter()
        .map(|f| egui::Rect::from_min_size(rect.min + vec2(f[0] as f32 * size.x, f[1] as f32 * size.y), vec2(f[2] as f32 * size.x, f[3] as f32 * size.y)))
        .collect();
    for (i, b) in boxes.iter().enumerate() {
        let on = p.picked == Some(i);
        r.rect_stroke(*b, 2.0, Stroke::new(if on { 2.5 } else { 1.0 }, if on { t.accent } else { Color32::from_white_alpha(200) }), StrokeKind::Outside);
    }
    app.auto.add("reframe.frame", rect, "Frame at the playhead");
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
        && let Some(i) = (0..boxes.len()).min_by(|a, b| boxes[*a].center().distance(at).total_cmp(&boxes[*b].center().distance(at)))
        && let Some(pk) = app.ui.reframe.picker.as_mut()
    {
        pk.picked = Some(i);
    }
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 480.0;
    let clips = selected(app);
    if app.ui.reframe.target == "face" {
        load_picker(app);
    }
    egui::Modal::new(egui::Id::new("reframe-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Reframe to Face"), w);
        app.auto.add("reframe.dialog", bar, "Reframe to Face");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(tlf!("The selected clips are reframed: {n}", n = clips.len())).color(t.text_dim));
            ui.label(RichText::new(tl!("Follow")).strong());
            let choices = [
                ("speaker", tl!("Speaker"), tl!("The face that stays in frame, not someone passing by.")),
                ("group", tl!("Everyone in frame"), tl!("All the steady faces, framed by their middle.")),
                ("face", tl!("Chosen face"), tl!("Click the face to follow on the frame at the playhead.")),
            ];
            for (id, name, about) in choices {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r = ui.radio_value(&mut app.ui.reframe.target, id.to_string(), name);
                    app.auto.add(&format!("reframe.target.{id}"), r.rect, name);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.add(egui::Label::new(RichText::new(about).color(t.text_dim).small()).wrap());
                    });
                });
            }
            if app.ui.reframe.target == "face" {
                picker(app, ui, w - 36.0);
            }
            ui.add_space(4.0);
            let z = &mut app.ui.reframe;
            egui::Grid::new("reframe-options").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Face height in the frame"));
                let r = ui.add(egui::Slider::new(&mut z.face_height, 20.0..=60.0).suffix(" %").max_decimals(0));
                app.auto.add("reframe.faceHeight", r.rect, "Face height in the frame");
                ui.end_row();
                ui.label(tl!("Zoom"));
                let r = ui.add(egui::Slider::new(&mut z.zoom, 100.0..=200.0).suffix(" %").max_decimals(0));
                app.auto.add("reframe.zoom", r.rect, "Zoom");
                ui.end_row();
                ui.label(tl!("Smoothing, seconds"));
                let r = ui.add(egui::DragValue::new(&mut z.smoothing).range(0.0..=6.0).speed(0.1).max_decimals(1));
                app.auto.add("reframe.smoothing", r.rect, "Smoothing, seconds");
                ui.end_row();
            });
            ui.add(
                egui::Label::new(
                    RichText::new(tl!(
                        "Faces are found on this computer with Apple Vision; nothing is sent. The framing is Position and Scale keyframes in Effect Controls."
                    ))
                    .color(t.text_dim)
                    .small(),
                )
                .wrap(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let picked = z.target != "face" || z.picker.as_ref().is_some_and(|p| p.picked.is_some());
                let g = ui.add_enabled(!clips.is_empty() && picked && z.job.is_none(), crate::dialog_style::primary(tl!("Reframe")));
                app.auto.add("reframe.go", g.rect, "Reframe");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("reframe.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let z = &mut app.ui.reframe;
        z.dialog = false;
        let mut params = json!({
            "clips": clips,
            "target": z.target,
            "anchorY": f64::from(z.face_height.round()) / 100.0,
            "zoom": f64::from(z.zoom.round()),
            "smoothing": (f64::from(z.smoothing) * 10.0).round() / 10.0,
        });
        if z.target == "face"
            && let Some(p) = &z.picker
            && let Some(f) = p.picked.and_then(|i| p.faces.get(i))
        {
            params["x"] = json!(f[0] + f[2] / 2.0);
            params["y"] = json!(f[1] + f[3] / 2.0);
            params["time"] = json!(p.at);
        }
        run(app, params);
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.reframe.dialog = false;
    }
}
