//! AI ▸ Automatic Editing ▸ Find Highlights… (also the Text panel's Transcript toolbar): the search dialog — what
//! to look for, how many, how long — runs `highlights.find` as a cloud task (consent, progress and
//! Cancel in the status bar and the Progress panel, see `panels::ai_tasks`). The highlights land on
//! the sequence as range markers; the results window lists them (click one to go there) and
//! assembles the chosen ones into subsequences, one new sequence or export queue items
//! (`highlights.assemble`).

use egui::{RichText, vec2};
use filmcraft_time::{Tick, TimeDisplay, format_time};
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// The search modes: id, name, what it finds.
const MODES: [(&str, &str, &str); 3] = [
    ("viral", "Viral moments", "A strong hook, emotion or surprise: clips that work on their own."),
    ("meaningful", "Meaningful ideas", "Complete thoughts with a setup and a conclusion."),
    ("facts", "Facts", "Numbers, names and definitions: precise, quotable passages."),
];

/// Open the search dialog (refused, with the reason, when the sequence has no transcript).
pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("highlights.find").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    app.ui.highlights.dialog = true;
    Ok(json!({"dialog": "highlights"}))
}

/// The search dialog, the results window, and taking over a finished search's result.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.ui.ai_task_result.as_ref().is_some_and(|(cmd, _)| cmd == "highlights.find")
        && let Some((_, r)) = app.ui.ai_task_result.take()
    {
        let found = r["highlights"].as_array().cloned().unwrap_or_default();
        app.ui.highlights.chosen = vec![true; found.len()];
        app.ui.highlights.found = found;
    }
    if app.ui.highlights.dialog {
        dialog(app, ctx);
    }
    if !app.ui.highlights.found.is_empty() {
        results(app, ctx);
    }
}

fn dialog(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let (mut go, mut cancel) = (false, false);
    let w = 440.0;
    let words = filmcraft_engine::transcript::sequence_words(&app.session).len();
    let seq_name = app.session.state.active_sequence.and_then(|id| app.session.project.item(id)).map(|i| i.name.clone()).unwrap_or_default();
    let provider = app.session.prefs.ai.text_provider();
    egui::Modal::new(egui::Id::new("highlights-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Find Highlights"), w);
        app.auto.add("highlights.dialog", bar, "Find Highlights");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.label(RichText::new(tlf!("{name} · words: {n}", name = seq_name, n = words)).color(t.text_dim));
            ui.label(RichText::new(tl!("Look for")).strong());
            let h = &mut app.ui.highlights;
            for (id, name, about) in MODES {
                // each description right under its choice
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r = ui.radio_value(&mut h.mode, id.to_string(), crate::i18n::t(name));
                    app.auto.add(&format!("highlights.mode.{id}"), r.rect, name);
                    ui.horizontal(|ui| {
                        ui.add_space(22.0);
                        ui.add(egui::Label::new(RichText::new(crate::i18n::t(about)).color(t.text_dim).small()).wrap());
                    });
                });
            }
            ui.add_space(4.0);
            egui::Grid::new("highlights-limits").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
                ui.label(tl!("How many"));
                let r = ui.add(egui::DragValue::new(&mut h.count).range(1..=20));
                app.auto.add("highlights.count", r.rect, "How many");
                ui.end_row();
                ui.label(tl!("Length, seconds"));
                ui.horizontal(|ui| {
                    let a = ui.add(egui::DragValue::new(&mut h.min_seconds).range(5.0..=180.0).speed(1.0).max_decimals(0));
                    ui.label("–");
                    let lo = h.min_seconds + 5.0;
                    let b = ui.add(egui::DragValue::new(&mut h.max_seconds).range(lo..=600.0).speed(1.0).max_decimals(0));
                    app.auto.add("highlights.minSeconds", a.rect, "Shortest");
                    app.auto.add("highlights.maxSeconds", b.rect, "Longest");
                });
                ui.end_row();
            });
            h.max_seconds = h.max_seconds.max(h.min_seconds + 5.0);
            ui.label(
                RichText::new(tlf!("The transcript goes to {provider}. Each highlight becomes a range marker on the sequence.", provider = provider.name()))
                    .color(t.text_dim)
                    .small(),
            );
            crate::dialog_style::actions(ui, |ui| {
                let g = ui.add_enabled(app.ui.ai_task.is_none(), crate::dialog_style::primary(tl!("Find")));
                app.auto.add("highlights.go", g.rect, "Find");
                let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                app.auto.add("highlights.cancel", c.rect, "Cancel");
                go = g.clicked();
                cancel = c.clicked();
            });
        });
    });
    if go {
        let h = &mut app.ui.highlights;
        h.dialog = false;
        let params = json!({"mode": h.mode, "count": h.count, "minSeconds": h.min_seconds.round(), "maxSeconds": h.max_seconds.round()});
        crate::panels::ai_tasks::start(app, "highlights.find", params, "Finding highlights", "the transcript");
    } else if cancel || crate::widgets::escape_closes(ctx) {
        app.ui.highlights.dialog = false;
    }
}

/// What the results window's buttons ask for.
enum Act {
    Go(f64),
    Assemble(&'static str),
    Close,
}

fn results(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let rate = app.session.sequence_rate();
    let df = app.session.active_sequence().is_some_and(|q| q.settings.drop_frame);
    let tc = |s: f64| format_time(Tick::from_seconds_f64(s), rate, df, TimeDisplay::Timecode, 48_000);
    // the colour of the markers (all of one search share it)
    let color = app
        .session
        .active_sequence()
        .and_then(|q| q.markers.iter().find(|m| Some(m.id.0) == app.ui.highlights.found.first().and_then(|f| f["marker"].as_u64())))
        .map(|m| crate::panels::settings::label_color(app, m.color));
    let mut act: Option<Act> = None;
    let mut open = true;
    let found = app.ui.highlights.found.clone();
    crate::dialog_style::Window::new(tl!("Highlights Found"))
        .id(egui::Id::new("highlights-results"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(460.0)
        .default_height(420.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(RichText::new(tl!("Each highlight is a range marker on the sequence. Click a title to go there.")).color(t.text_dim).small());
            let list_h = (ui.available_height() - 44.0).max(120.0);
            egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                for (i, f) in found.iter().enumerate() {
                    let (start, end) = (f["start"].as_f64().unwrap_or(0.0), f["end"].as_f64().unwrap_or(0.0));
                    egui::Frame::new().fill(t.field_bg).corner_radius(4).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            if let Some(chosen) = app.ui.highlights.chosen.get_mut(i) {
                                let c = ui.checkbox(chosen, "");
                                app.auto.add(&format!("highlights.item.{i}.chosen"), c.rect, "Use this highlight");
                            }
                            if let Some(c) = color {
                                let (r, _) = ui.allocate_exact_size(vec2(4.0, 16.0), egui::Sense::hover());
                                ui.painter().rect_filled(r, 1.0, c);
                            }
                            let title = ui
                                .add(egui::Label::new(RichText::new(f["title"].as_str().unwrap_or_default()).strong()).sense(egui::Sense::click()).truncate());
                            app.auto.add(&format!("highlights.item.{i}"), title.rect, f["title"].as_str().unwrap_or_default());
                            if title.clicked() {
                                act = Some(Act::Go(start));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(f["score"].as_u64().unwrap_or(0).to_string()).color(t.hot_text).strong());
                            });
                        });
                        ui.label(
                            RichText::new(tlf!("{start} – {end} · {n} s", start = tc(start), end = tc(end), n = (end - start).round() as i64))
                                .color(t.text_dim)
                                .small(),
                        );
                        if let Some(reason) = f["reason"].as_str().filter(|r| !r.is_empty()) {
                            ui.add(egui::Label::new(RichText::new(reason).color(t.text_dim)).wrap());
                        }
                    });
                }
            });
            ui.add_space(6.0);
            let any = app.ui.highlights.chosen.iter().any(|c| *c);
            crate::dialog_style::actions(ui, |ui| {
                // right to left: the primary action outermost, as in the other dialogs
                let b = ui.add_enabled(any, crate::dialog_style::primary(tl!("Subsequences")));
                app.auto.add("highlights.subsequences", b.rect, "Subsequences");
                let s = ui.add_enabled(any, crate::dialog_style::secondary(tl!("New Sequence")));
                app.auto.add("highlights.sequence", s.rect, "New Sequence");
                let q = ui.add_enabled(any, crate::dialog_style::secondary(tl!("Add to Export Queue")));
                app.auto.add("highlights.queue", q.rect, "Add to Export Queue");
                if b.clicked() {
                    act = Some(Act::Assemble("subsequences"));
                } else if s.clicked() {
                    act = Some(Act::Assemble("sequence"));
                } else if q.clicked() {
                    act = Some(Act::Assemble("queue"));
                }
            });
            ui.add_space(4.0);
        });
    if !open {
        act = Some(Act::Close);
    }
    match act {
        Some(Act::Go(start)) => {
            if let Err(e) = app.session.execute("playhead.set", json!({"seconds": start})) {
                app.ui.status = e.to_string();
            }
        }
        Some(Act::Assemble(how)) => {
            let markers: Vec<u64> = found.iter().zip(&app.ui.highlights.chosen).filter(|(_, c)| **c).filter_map(|(f, _)| f["marker"].as_u64()).collect();
            match app.session.execute("highlights.assemble", json!({"markers": markers, "as": how})) {
                Ok(r) => {
                    app.ui.status = match how {
                        "queue" => tlf!("Highlights: {n} exports added to the queue", n = r["queued"].as_array().map_or(0, Vec::len)),
                        "sequence" => tl!("Highlights: a new sequence is made").to_string(),
                        _ => tlf!("Highlights: {n} subsequences made", n = r["sequences"].as_array().map_or(0, Vec::len)),
                    };
                    app.ui.highlights.found.clear();
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        Some(Act::Close) => app.ui.highlights.found.clear(),
        None => {}
    }
}
