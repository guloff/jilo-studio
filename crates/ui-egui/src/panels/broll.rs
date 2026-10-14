//! AI ▸ Automatic Editing ▸ Auto B-roll…: footage of the project that fits what is being said goes
//! on the track above the speech (`broll.auto`). By words on this computer, or by the text engine
//! in the cloud (then a job in the status bar; the clips are placed when it ends). Pictures for
//! the phrases left without footage can be drawn by the image engine.
//!
//! Automation ids: `broll.dialog`, `broll.method.<keywords|ai>`, `broll.seconds`, `broll.minGap`,
//! `broll.max`, `broll.generate`, `broll.go`, `broll.cancel`.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;

#[derive(Clone, Debug)]
pub struct BrollUi {
    pub dialog: bool,
    /// `keywords` or `ai`.
    pub method: String,
    pub seconds: f32,
    pub min_gap: f32,
    pub max: u32,
    pub generate: bool,
    /// The cloud choice running in the background, and the parameters to place with when it ends.
    pub job: Option<(u64, Value)>,
}

impl Default for BrollUi {
    fn default() -> Self {
        Self { dialog: false, method: "keywords".into(), seconds: 3.0, min_gap: 4.0, max: 12, generate: false, job: None }
    }
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("broll.auto").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    if filmcraft_engine::transcript::sequence_words(&app.session).is_empty() {
        return Err("the sequence has no transcript (AI ▸ Speech and Captions ▸ Transcribe first)".into());
    }
    if app.ui.broll.job.is_some() {
        return Err("B-roll is already being chosen".into());
    }
    app.ui.broll.dialog = true;
    Ok(json!({"dialog": "broll"}))
}

fn done(r: &Value) -> String {
    let n = r["placed"].as_array().map_or(0, Vec::len);
    let mut m = tlf!("B-roll placed: {n}", n);
    if let Some(d) = r["drawing"].as_u64().filter(|d| *d > 0) {
        m = format!("{m}. {}", tlf!("Pictures being drawn: {n}", n = d));
    }
    if n == 0 && r["drawing"].as_u64().unwrap_or(0) == 0 {
        m = tl!("No footage fits what is said. Name the clips after what they show, or try the AI method.").to_string();
    }
    m
}

fn run(app: &mut FilmcraftApp, params: Value) {
    let mut bg = params.clone();
    bg["background"] = json!(true);
    match app.session.execute("broll.auto", bg) {
        Ok(r) if r["background"] == true => {
            app.ui.broll.job = r["job"].as_u64().map(|j| (j, params));
            app.ui.status = tl!("Choosing B-roll…").to_string();
        }
        Ok(r) => app.ui.status = done(&r),
        Err(e) => app.ui.status = e.to_string(),
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if let Some((id, params)) = app.ui.broll.job.clone() {
        match app.session.jobs.iter().find(|j| j.id == id).map(|j| j.to_json()) {
            Some(j) if j["finished"] != true => {}
            Some(j) if j["result"]["error"].is_string() => {
                app.ui.broll.job = None;
                app.ui.status = j["result"]["error"].as_str().unwrap_or_default().to_string();
            }
            _ => {
                app.ui.broll.job = None;
                app.ui.status = match app.session.execute("broll.auto", params) {
                    Ok(r) => done(&r),
                    Err(e) => e.to_string(),
                };
            }
        }
    }
    if !app.ui.broll.dialog {
        return;
    }
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 460.0;
    let chosen = app
        .session
        .state
        .project_selection
        .iter()
        .filter(|i| app.session.project.item(**i).is_some_and(|x| matches!(x.kind, filmcraft_project::ItemKind::Media(_)) && x.has_video()))
        .count();
    let text = app.session.prefs.ai.text_provider().name();
    let image = app.session.prefs.ai.image_provider().name();
    egui::Modal::new(egui::Id::new("broll-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Auto B-roll"), w);
        app.auto.add("broll.dialog", bar, "Auto B-roll");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let from = if chosen > 0 {
                tlf!("Footage: the {n} clips selected in the Project panel.", n = chosen)
            } else {
                tl!("Footage: every video and still of the project that the sequence does not use yet.").to_string()
            };
            ui.label(RichText::new(from).color(t.text_dim));
            let u = &mut app.ui.broll;
            let ai_about = tlf!("The phrases and the clips' descriptions go to {provider}, which picks what fits. Any language.", provider = text);
            for (id, name, about) in [
                (
                    "keywords",
                    tl!("By words, on this computer"),
                    tl!("Clip names, descriptions and keywords, and what Apple Vision sees in the clips, matched with the words of each phrase."),
                ),
                ("ai", tl!("By meaning, with AI"), ai_about.as_str()),
            ] {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r = ui.radio_value(&mut u.method, id.to_string(), name);
                    app.auto.add(&format!("broll.method.{id}"), r.rect, id);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.add(egui::Label::new(RichText::new(about).color(t.text_dim).small()).wrap());
                    });
                });
            }
            let u = &mut app.ui.broll;
            let mut rects = Vec::new();
            egui::Grid::new("broll-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("Each clip"));
                rects.push(("broll.seconds", ui.add(egui::Slider::new(&mut u.seconds, 1.0..=10.0).suffix(tl!(" s")).max_decimals(1)).rect));
                ui.end_row();
                ui.label(tl!("At least apart"));
                rects.push(("broll.minGap", ui.add(egui::Slider::new(&mut u.min_gap, 0.0..=20.0).suffix(tl!(" s")).max_decimals(1)).rect));
                ui.end_row();
                ui.label(tl!("At most"));
                rects.push(("broll.max", ui.add(egui::Slider::new(&mut u.max, 1..=50)).rect));
                ui.end_row();
            });
            let r = ui.checkbox(&mut u.generate, tlf!("Draw pictures for phrases without footage ({provider}, cloud)", provider = image));
            rects.push(("broll.generate", r.rect));
            for (id, r) in rects {
                app.auto.add(id, r, id);
            }
            let cloud = u.method == "ai" || u.generate;
            let note = if cloud {
                tl!("Starting agrees to send what is described above to the cloud. One Undo takes the footage back.")
            } else {
                tl!("Nothing is sent. One Undo takes the footage back.")
            };
            ui.label(RichText::new(note).color(t.text_dim).small());
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add(crate::dialog_style::primary(tl!("Place B-roll")));
                app.auto.add("broll.go", g.rect, "Place B-roll");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("broll.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let u = &app.ui.broll;
        let p = json!({
            "method": u.method, "seconds": (f64::from(u.seconds) * 10.0).round() / 10.0, "minGap": (f64::from(u.min_gap) * 10.0).round() / 10.0,
            "maxClips": u.max, "generate": u.generate, "confirm": true,
        });
        app.ui.broll.dialog = false;
        run(app, p);
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.broll.dialog = false;
    }
}
