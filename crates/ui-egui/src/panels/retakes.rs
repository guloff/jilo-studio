//! AI ▸ Speech and Captions ▸ Remove Retakes and Stumbles…: the failed takes found in the transcript
//! (`transcript.findRetakes`) — a phrase said again (the earlier attempt goes, the last stays), a
//! false start, a stutter — each with a checkbox, what is cut (struck through) and what stays after
//! it. Clicking a row moves the playhead there to listen. Remove cuts the checked ones in one undo
//! step (`transcript.removeRetakes`).
//!
//! Automation ids: `retakes.dialog`, `retakes.item.<n>`, `retakes.go.<n>`, `retakes.remove`,
//! `retakes.cancel`.

use egui::RichText;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The dialog's state.
#[derive(Clone, Debug, Default)]
pub struct RetakesUi {
    pub open: bool,
    /// As `transcript.findRetakes` lists them.
    pub found: Vec<Value>,
    pub checked: Vec<bool>,
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("transcript.removeRetakes").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    let r = app.session.execute("transcript.findRetakes", json!({})).map_err(|e| e.to_string())?;
    let found = r["retakes"].as_array().cloned().unwrap_or_default();
    if found.is_empty() {
        app.ui.status = tl!("No retakes or stumbles found in the transcript.").into();
        return Ok(json!({"retakes": 0}));
    }
    app.ui.retakes = RetakesUi { open: true, checked: vec![true; found.len()], found };
    Ok(json!({"dialog": "retakes", "retakes": app.ui.retakes.found.len()}))
}

fn kind_name(kind: &str) -> &'static str {
    match kind {
        "retake" => tl!("Retake"),
        "falsestart" => tl!("False start"),
        _ => tl!("Stutter"),
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.retakes.open {
        return;
    }
    let t = app.tokens;
    let (mut go, mut cancel, mut seek) = (false, false, None::<f64>);
    let w = 560.0;
    egui::Modal::new(egui::Id::new("retakes-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Remove Retakes and Stumbles"), w);
        app.auto.add("retakes.dialog", bar, "Remove Retakes and Stumbles");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.label(RichText::new(tl!("Checked stretches are cut; the take after each one stays. Click a row to move the playhead there.")).color(t.text_dim));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                let r = &mut app.ui.retakes;
                for (k, f) in r.found.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let c = ui.checkbox(&mut r.checked[k], "");
                        app.auto.add(&format!("retakes.item.{k}"), c.rect, "Cut");
                        let at = f["start"].as_f64().unwrap_or(0.0);
                        let time = format!("{}:{:04.1}", (at / 60.0) as u64, at % 60.0);
                        let row = ui.add(
                            egui::Label::new(
                                RichText::new(format!("{time}  {}", kind_name(f["kind"].as_str().unwrap_or_default()))).size(11.0).color(t.text_dim),
                            )
                            .sense(egui::Sense::click()),
                        );
                        app.auto.add(&format!("retakes.go.{k}"), row.rect, "Go to");
                        if row.clicked() {
                            seek = Some(at);
                        }
                        ui.label(RichText::new(f["removed"].as_str().unwrap_or_default()).strikethrough().color(t.text));
                        ui.label(RichText::new(format!("→ {}…", f["kept"].as_str().unwrap_or_default())).color(t.text_faint));
                    });
                }
            });
            crate::dialog_style::actions(ui, |ui| {
                let n = app.ui.retakes.checked.iter().filter(|c| **c).count();
                let g = ui.add_enabled(n > 0, crate::dialog_style::primary(tlf!("Remove {n}", n)));
                app.auto.add("retakes.remove", g.rect, "Remove");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("retakes.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if let Some(at) = seek {
        let _ = app.session.execute("playhead.set", json!({"seconds": (at - 0.5).max(0.0)}));
    }
    if go {
        let r = &app.ui.retakes;
        let ranges: Vec<Value> = r.found.iter().zip(&r.checked).filter(|(_, c)| **c).map(|(f, _)| json!([f["from"], f["to"]])).collect();
        app.ui.status = match app.session.execute("transcript.removeRetakes", json!({"ranges": ranges})) {
            Ok(v) => tlf!(
                "Removed {n} retakes ({seconds} s)",
                n = v["removed"].as_u64().unwrap_or(0),
                seconds = format!("{:.1}", v["seconds"].as_f64().unwrap_or(0.0))
            ),
            Err(e) => e.to_string(),
        };
        app.ui.retakes.open = false;
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.retakes.open = false;
    }
}
