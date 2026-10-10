//! Help ▸ Check for Updates…: the check runs in the background (`app.checkUpdates`, a job in the
//! status bar), then a window says what it found — a newer release with its notes and a Download
//! button that opens the release's GitHub page, or that this version is the latest, or why the
//! check failed. Nothing is checked unless asked, or — with Settings ▸ General ▸ Check for updates
//! once a day — once a day in the background, which says nothing unless a newer release is out and
//! offers each release once.

use egui::RichText;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// Start a check (refused while one is running).
pub fn start(app: &mut FilmcraftApp) -> Result<Value, String> {
    if app.ui.updates.job.is_some() {
        return Err("updates are already being checked".into());
    }
    let r = app.session.execute("app.checkUpdates", json!({})).map_err(|e| e.to_string())?;
    app.ui.updates.job = r["job"].as_u64();
    app.ui.updates.auto = false;
    app.ui.updates.result = None;
    app.ui.status = tl!("Checking for updates…").to_string();
    Ok(json!({"checking": true}))
}

/// The daily check: looked at once at startup and then hourly; runs when a day has passed.
fn daily(app: &mut FilmcraftApp) {
    let now = web_time::Instant::now();
    if app.ui.updates.job.is_some() || app.ui.updates.next_look.is_some_and(|t| now < t) {
        return;
    }
    app.ui.updates.next_look = Some(now + std::time::Duration::from_secs(3600));
    let unix = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    if !filmcraft_engine::updates::daily_check_due(&app.session.prefs.general, unix) {
        return;
    }
    // recorded before the check, so a failing network is not asked again for a day
    let _ = app.session.execute("prefs.set", json!({"key": "general.lastUpdateCheck", "value": unix}));
    if let Ok(r) = app.session.execute("app.checkUpdates", json!({})) {
        app.ui.updates.job = r["job"].as_u64();
        app.ui.updates.auto = true;
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    daily(app);
    if let Some(id) = app.ui.updates.job
        && app.session.jobs.iter().find(|j| j.id == id).is_none_or(|j| j.to_json()["finished"] == true)
    {
        app.ui.updates.job = None;
        let r = app.session.execute("app.updateStatus", json!({})).ok();
        if app.ui.updates.auto {
            // the daily check speaks only of a newer release, once per release
            let latest = r.as_ref().and_then(|v| (v["newer"] == true).then(|| v["latest"].as_str().unwrap_or_default().to_string()));
            if let Some(latest) = latest.filter(|l| *l != app.session.prefs.general.update_offered) {
                let _ = app.session.execute("prefs.set", json!({"key": "general.updateOffered", "value": latest}));
                app.ui.updates.result = r;
            }
        } else {
            app.ui.updates.result = r;
            app.ui.status.clear();
        }
    }
    if app.ui.updates.result.is_some() {
        window(app, ctx);
    }
}

fn window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let Some(v) = app.ui.updates.result.clone() else { return };
    let (mut open, mut close, mut download) = (true, false, false);
    let current = v["current"].as_str().unwrap_or_default();
    let newer = v["newer"] == true;
    crate::dialog_style::Window::new(tl!("Software Update"))
        .id(egui::Id::new("software-update"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(440.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(440.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            let app_name = crate::brand::APP_NAME;
            if let Some(e) = v["error"].as_str() {
                ui.label(RichText::new(tl!("Could not check for updates")).strong());
                ui.add(egui::Label::new(RichText::new(e).color(t.text_dim)).wrap());
            } else if v["none"] == true {
                ui.label(RichText::new(tlf!("{app} {version} is the latest version.", app = app_name, version = current)).strong());
                ui.label(RichText::new(tl!("No release is published on GitHub yet.")).color(t.text_dim));
            } else if newer {
                let latest = v["latest"].as_str().unwrap_or_default();
                ui.label(RichText::new(tlf!("{app} {version} is available", app = app_name, version = latest)).strong().size(14.0));
                ui.label(RichText::new(tlf!("You have {version}.", version = current)).color(t.text_dim));
                let notes = v["notes"].as_str().unwrap_or_default();
                if !notes.is_empty() {
                    ui.add_space(2.0);
                    ui.label(RichText::new(tl!("What's new")).strong());
                    egui::Frame::new().fill(t.field_bg).corner_radius(4.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(notes).small()).wrap());
                        });
                    });
                }
                ui.label(
                    RichText::new(tl!("Download opens the release page on GitHub; the new version replaces this one in Applications."))
                        .color(t.text_dim)
                        .small(),
                );
            } else {
                ui.label(RichText::new(tlf!("{app} {version} is the latest version.", app = app_name, version = current)).strong());
            }
            ui.add_space(4.0);
            crate::dialog_style::actions(ui, |ui| {
                if newer {
                    let d = ui.add(crate::dialog_style::primary(tl!("Download…")));
                    app.auto.add("updates.download", d.rect, "Download…");
                    let l = ui.add(crate::dialog_style::secondary(tl!("Later")));
                    app.auto.add("updates.later", l.rect, "Later");
                    download = d.clicked();
                    close = l.clicked();
                } else {
                    let c = ui.add(crate::dialog_style::primary(tl!("OK")));
                    app.auto.add("updates.ok", c.rect, "OK");
                    close = c.clicked();
                }
            });
        });
    if download {
        // the engine keeps the page to this repository's releases
        crate::links::open(ctx, v["url"].as_str().unwrap_or(filmcraft_engine::updates::RELEASES_PAGE));
        close = true;
    }
    if !open || close || crate::widgets::escape_closes(ctx) {
        app.ui.updates.result = None;
    }
}
