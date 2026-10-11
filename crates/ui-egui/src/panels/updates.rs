//! Help ▸ Check for Updates…: the check runs in the background (`app.checkUpdates`, a job in the
//! status bar), then the Software Update window says what it found — a newer release with its
//! notes, that this version is the latest, or why the check failed. With Settings ▸ General ▸
//! Check for updates once a day (on by default; the app host turns it on with
//! `with_daily_update_check`) it also runs once a day in the background, says nothing unless a
//! newer release is out and offers each release once.
//!
//! Download and Install keeps the window open through every step (`app.downloadUpdate`): a
//! progress bar with the megabytes while the disk image downloads (Stop ends it; Hide leaves it
//! running in the status bar and the Progress panel, and the window comes back when it is done),
//! then the check of the new version, then Restart and Install — the app quits (asking about
//! unsaved changes as any quit does), the new version replaces this one and opens — or Install
//! When I Quit. The next launch says the update is installed (or that it wasn't). When the app
//! can't replace itself, the window says why and offers the downloaded disk image to open, with the
//! steps to install it by hand.

use egui::RichText;
use serde_json::{Value, json};

use crate::FilmcraftApp;

/// Start a check (refused while one is running). While an update is downloading or ready, the
/// window shows it instead of checking again.
pub fn start(app: &mut FilmcraftApp) -> Result<Value, String> {
    if app.ui.updates.job.is_some() {
        return Err("updates are already being checked".into());
    }
    if matches!(phase(app).as_deref(), Some("downloading" | "verifying" | "ready" | "manual")) {
        app.ui.updates.result = app.session.execute("app.updateStatus", json!({})).ok();
        return Ok(json!({"shown": true}));
    }
    let r = app.session.execute("app.checkUpdates", json!({})).map_err(|e| e.to_string())?;
    app.ui.updates.job = r["job"].as_u64();
    app.ui.updates.auto = false;
    app.ui.updates.result = None;
    app.ui.status = tl!("Checking for updates…").to_string();
    Ok(json!({"checking": true}))
}

/// The update's phase (`downloading`, `verifying`, `ready`, `manual`, `failed`, `cancelled`).
fn phase(app: &FilmcraftApp) -> Option<String> {
    app.session.update_download.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().map(|d| d.phase.clone())
}

/// The daily check: looked at once at startup and then hourly; runs when a day has passed.
fn daily(app: &mut FilmcraftApp) {
    let now = web_time::Instant::now();
    if !app.ui.updates.host || app.ui.updates.job.is_some() || app.ui.updates.next_look.is_some_and(|t| now < t) {
        return;
    }
    app.ui.updates.next_look = Some(now + std::time::Duration::from_secs(3600));
    if matches!(phase(app).as_deref(), Some("downloading" | "verifying" | "ready" | "manual")) {
        return;
    }
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

/// At launch: an update was being installed when the app last quit — say whether it took.
fn after_restart(app: &mut FilmcraftApp) {
    if !app.ui.updates.host || app.ui.updates.launch_seen {
        return;
    }
    app.ui.updates.launch_seen = true;
    let pending = app.session.prefs.general.update_installing.clone();
    if pending.is_empty() {
        return;
    }
    let _ = app.session.execute("prefs.set", json!({"key": "general.updateInstalling", "value": ""}));
    let current = env!("CARGO_PKG_VERSION");
    let installed = pending == current;
    if installed {
        filmcraft_engine::updater::clean_up(&app.session);
    }
    app.ui.updates.result = Some(json!({"current": current, "afterRestart": true, "installed": installed, "installedVersion": pending}));
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    after_restart(app);
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
    // the download ended (also while the window was hidden): show how it ended
    if let Some(id) = app.ui.updates.download_job
        && app.session.jobs.iter().find(|j| j.id == id).is_none_or(|j| j.to_json()["finished"] == true)
    {
        app.ui.updates.download_job = None;
        app.ui.updates.result = app.session.execute("app.updateStatus", json!({})).ok();
    }
    if app.ui.updates.result.is_some() {
        window(app, ctx);
    }
}

/// The quit was cancelled at the unsaved-changes question: Restart and Install is undone (the
/// update stays ready) and the window comes back.
pub fn quit_cancelled(app: &mut FilmcraftApp) {
    let armed_restart =
        app.session.update_download.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|d| d.install_on_quit && d.relaunch);
    if armed_restart {
        let _ = app.session.execute("app.installUpdate", json!({"cancel": true}));
        app.ui.updates.result = app.session.execute("app.updateStatus", json!({})).ok();
    }
}

enum Action {
    Close,
    Download,
    Stop,
    Hide,
    Restart,
    InstallOnQuit,
    OpenImage,
    OpenPage,
}

fn window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let t = app.tokens;
    let Some(v) = app.ui.updates.result.clone() else { return };
    let d = app.session.update_download.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let latest = v["latest"].as_str().unwrap_or_default().to_string();
    // the update in progress, when it is this release's
    let d = d.filter(|d| d.version == latest && v["newer"] == true);
    let progress = app.ui.updates.download_job.and_then(|id| app.session.jobs.iter().find(|j| j.id == id)).map(|j| {
        use std::sync::atomic::Ordering;
        (j.progress.done.load(Ordering::Relaxed), j.progress.total.load(Ordering::Relaxed))
    });
    let mut open = true;
    let mut act: Option<Action> = None;
    let current = v["current"].as_str().unwrap_or_default();
    let newer = v["newer"] == true;
    let busy = d.as_ref().is_some_and(|d| d.phase == "downloading" || d.phase == "verifying");
    let app_name = crate::brand::APP_NAME;
    let mb = |b: u64| format!("{:.1}", b as f64 / 1e6);
    crate::dialog_style::Window::new(tl!("Software Update"))
        .id(egui::Id::new("software-update"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            let dim = |s: &str| RichText::new(s.to_string()).color(t.text_dim);
            if v["afterRestart"] == true {
                let version = v["installedVersion"].as_str().unwrap_or_default();
                if v["installed"] == true {
                    ui.label(RichText::new(tlf!("{app} {version} is installed", app = app_name, version)).strong().size(14.0));
                    ui.label(dim(tl!("The update is done: you are using the new version.")));
                } else {
                    ui.label(RichText::new(tlf!("{app} {version} was not installed", app = app_name, version)).strong().size(14.0));
                    ui.add(egui::Label::new(dim(&tlf!("You are still using {version}. Check for updates to try again, or download it from the release page.", version = current))).wrap());
                }
                crate::dialog_style::actions(ui, |ui| {
                    let c = ui.add(crate::dialog_style::primary(tl!("OK")));
                    app.auto.add("updates.ok", c.rect, "OK");
                    if c.clicked() {
                        act = Some(Action::Close);
                    }
                });
                return;
            }
            if let Some(e) = v["error"].as_str() {
                ui.label(RichText::new(tl!("Could not check for updates")).strong());
                ui.add(egui::Label::new(dim(e)).wrap());
            } else if v["none"] == true {
                ui.label(RichText::new(tlf!("{app} {version} is the latest version.", app = app_name, version = current)).strong());
                ui.label(dim(tl!("No release is published on GitHub yet.")));
            } else if newer {
                ui.label(RichText::new(tlf!("{app} {version} is available", app = app_name, version = latest)).strong().size(14.0));
                ui.label(dim(&tlf!("You have {version}.", version = current)));
                match d.as_ref().map(|d| d.phase.as_str()) {
                    Some("downloading") => {
                        let (done, total) = progress.unwrap_or((0, 0));
                        let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
                        ui.add_space(4.0);
                        ui.label(RichText::new(tl!("Downloading the new version…")).strong());
                        let bar = ui.add(egui::ProgressBar::new(frac.clamp(0.0, 1.0)).desired_width(440.0).show_percentage());
                        app.auto.add("updates.progress", bar.rect, "Download progress");
                        ui.label(dim(&tlf!("{done} of {total} MB", done = mb(done), total = mb(total))));
                        ui.add(egui::Label::new(dim(tl!("You can hide this window and keep working: the download goes on, and this window comes back when it is done."))).wrap());
                    }
                    Some("verifying") => {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new());
                            ui.label(RichText::new(tl!("Checking the new version's signature…")).strong());
                        });
                    }
                    Some("ready") => {
                        ui.add_space(4.0);
                        ui.label(RichText::new(tlf!("{app} {version} is downloaded and checked.", app = app_name, version = latest)).strong());
                        if d.as_ref().is_some_and(|d| d.install_on_quit) {
                            ui.add(egui::Label::new(dim(tl!("It will be installed when you quit FilmCraft; the next launch opens the new version."))).wrap());
                        } else {
                            ui.add(egui::Label::new(dim(&tlf!(
                                "Restart and Install quits {app} (asking first about unsaved changes), puts {version} in place of this version and opens it again. It takes a few seconds.",
                                app = app_name,
                                version = latest
                            )))
                            .wrap());
                        }
                    }
                    Some("manual") => {
                        ui.add_space(4.0);
                        ui.label(RichText::new(tlf!("{app} {version} is downloaded.", app = app_name, version = latest)).strong());
                        let why = d.as_ref().and_then(|d| d.reason.clone()).map(|w| crate::i18n::t(&w).to_string()).unwrap_or_default();
                        ui.add(egui::Label::new(dim(&tlf!("It can't install itself here: {why}.", why))).wrap());
                        ui.label(RichText::new(tl!("To install it:")).strong());
                        for step in [
                            tl!("1. Open the disk image."),
                            tl!("2. Drag FilmCraft to Applications and choose Replace."),
                            tl!("3. Quit this version and open FilmCraft again."),
                        ] {
                            ui.label(step);
                        }
                    }
                    phase => {
                        if let Some(why) = d.as_ref().filter(|_| phase == Some("failed")).and_then(|d| d.reason.clone()).map(|w| crate::i18n::t(&w).to_string()) {
                            ui.add(egui::Label::new(RichText::new(tlf!("The download failed: {why}", why)).color(t.danger)).wrap());
                        }
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
                        let note = if v["asset"].is_object() {
                            tl!("Download and Install fetches the new version, checks its signature and installs it when you restart FilmCraft.")
                        } else {
                            tl!("This release has no disk image for this Mac: its page on GitHub opens instead.")
                        };
                        ui.add(egui::Label::new(dim(note).small()).wrap());
                    }
                }
            } else {
                ui.label(RichText::new(tlf!("{app} {version} is the latest version.", app = app_name, version = current)).strong());
            }
            ui.add_space(4.0);
            crate::dialog_style::actions(ui, |ui| {
                let mut button = |ui: &mut egui::Ui, primary: bool, id: &str, label: &str, a: Action| {
                    let b = ui.add(if primary { crate::dialog_style::primary(label) } else { crate::dialog_style::secondary(label) });
                    app.auto.add(id, b.rect, label);
                    if b.clicked() {
                        act = Some(a);
                    }
                };
                if !newer || v["error"].is_string() {
                    button(ui, true, "updates.ok", tl!("OK"), Action::Close);
                    return;
                }
                match d.as_ref().map(|d| d.phase.as_str()) {
                    Some("downloading") => {
                        button(ui, true, "updates.hide", tl!("Hide"), Action::Hide);
                        button(ui, false, "updates.stop", tl!("Stop"), Action::Stop);
                    }
                    Some("verifying") => button(ui, true, "updates.hide", tl!("Hide"), Action::Hide),
                    Some("ready") if d.as_ref().is_some_and(|d| d.install_on_quit) => button(ui, true, "updates.ok", tl!("OK"), Action::Close),
                    Some("ready") => {
                        button(ui, true, "updates.restart", tl!("Restart and Install"), Action::Restart);
                        button(ui, false, "updates.onQuit", tl!("Install When I Quit"), Action::InstallOnQuit);
                    }
                    Some("manual") => {
                        button(ui, true, "updates.openImage", tl!("Open Disk Image"), Action::OpenImage);
                        button(ui, false, "updates.close", tl!("Close"), Action::Close);
                    }
                    _ => {
                        if v["asset"].is_object() {
                            let label = if d.as_ref().is_some_and(|d| d.phase == "failed") { tl!("Try Again") } else { tl!("Download and Install") };
                            button(ui, true, "updates.download", label, Action::Download);
                        } else {
                            button(ui, true, "updates.page", tl!("Open Release Page"), Action::OpenPage);
                        }
                        button(ui, false, "updates.later", tl!("Later"), Action::Close);
                    }
                }
            });
            if newer && v["asset"].is_object() && !matches!(d.as_ref().map(|d| d.phase.as_str()), Some("ready" | "manual")) {
                let l = ui.add(egui::Link::new(RichText::new(tl!("Release page on GitHub")).small().color(t.accent)));
                app.auto.add("updates.pageLink", l.rect, "Release page on GitHub");
                if l.clicked() {
                    act = Some(Action::OpenPage);
                }
            }
        });
    let escape = crate::widgets::escape_closes(ctx);
    match act {
        Some(Action::Download) => match app.session.execute("app.downloadUpdate", json!({})) {
            Ok(r) => {
                app.ui.updates.download_job = r["job"].as_u64();
                app.ui.updates.result = app.session.execute("app.updateStatus", json!({})).ok();
            }
            Err(e) => app.ui.status = e.to_string(),
        },
        Some(Action::Stop) => {
            if let Some(j) = app.ui.updates.download_job.and_then(|id| app.session.jobs.iter().find(|j| j.id == id)) {
                j.progress.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Some(Action::Restart) => match app.session.execute("app.installUpdate", json!({"relaunch": true})) {
            Ok(_) => {
                app.ui.updates.result = None;
                // the ordinary quit: unsaved changes are asked about first
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => app.ui.status = e.to_string(),
        },
        Some(Action::InstallOnQuit) => match app.session.execute("app.installUpdate", json!({"relaunch": false})) {
            Ok(_) => app.ui.updates.result = app.session.execute("app.updateStatus", json!({})).ok(),
            Err(e) => app.ui.status = e.to_string(),
        },
        Some(Action::OpenImage) => {
            if let Err(e) = app.session.execute("app.openUpdateImage", json!({})) {
                app.ui.status = e.to_string();
            }
        }
        Some(Action::OpenPage) => {
            // the engine keeps the page to this repository's releases
            crate::links::open(ctx, v["url"].as_str().unwrap_or(filmcraft_engine::updates::RELEASES_PAGE));
        }
        Some(Action::Close | Action::Hide) => app.ui.updates.result = None,
        // closing while it downloads hides it: the download goes on
        None if !open || escape => app.ui.updates.result = None,
        None => {}
    }
    if busy {
        // the progress bar moves while nothing else happens
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}
