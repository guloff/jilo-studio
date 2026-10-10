//! Running a cloud text feature from the interface (translation, highlights, YouTube texts, zooms
//! by meaning): the one-time consent per provider, the background job (its progress and Cancel are
//! in the status bar and the Progress panel, like every job), and applying the answer when it is ready.
//!
//! A feature starts with [`start`]: its command runs with `background: true`; when the job
//! finishes, the same command runs again without it and applies the fetched answer (see
//! `filmcraft_engine::ai_tasks`).

use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::state::AiTaskUi;

/// Start `cmd` with `params` in the background; `what` names what goes to the cloud (for the
/// consent) and `label` the work (for the progress window).
pub fn start(app: &mut FilmcraftApp, cmd: &str, params: Value, label: &str, what: &str) {
    let provider = params["engine"].as_str().and_then(filmcraft_ai::Provider::from_id).unwrap_or_else(|| app.session.prefs.ai.text_provider());
    let task = AiTaskUi { command: cmd.to_string(), params, label: label.to_string(), what: what.to_string(), ..Default::default() };
    if app.session.prefs.ai.confirmed(provider) {
        launch(app, task);
    } else {
        app.ui.ai_task = Some(AiTaskUi { confirm: true, ..task });
    }
}

fn launch(app: &mut FilmcraftApp, mut task: AiTaskUi) {
    let mut p = task.params.clone();
    p["background"] = json!(true);
    if task.confirmed {
        p["confirm"] = json!(true);
        task.params["confirm"] = json!(true);
    }
    match app.session.execute(&task.command, p) {
        Ok(r) => {
            task.job = r["job"].as_u64();
            task.confirm = false;
            // a cached answer applies at once
            if task.job.is_none() {
                app.ui.status = done_message(&task.label);
                app.ui.ai_task = None;
                app.ui.ai_task_result = Some((task.command.clone(), r));
                return;
            }
            app.ui.ai_task = Some(task);
        }
        Err(e) => {
            app.ui.status = e.to_string();
            app.ui.ai_task = None;
        }
    }
}

fn done_message(label: &str) -> String {
    tlf!("{label}: done", label = crate::i18n::t(label))
}

/// Draw the consent or the progress of the running task; apply its answer when the job is done.
pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(mut task) = app.ui.ai_task.take() else { return };
    let t = app.tokens;
    if task.confirm {
        let provider = task.params["engine"].as_str().and_then(filmcraft_ai::Provider::from_id).unwrap_or_else(|| app.session.prefs.ai.text_provider());
        let (mut ok, mut cancel) = (false, false);
        let w = 440.0;
        egui::Modal::new(egui::Id::new("ai-task-consent")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
            ui.set_width(w);
            let bar = crate::dialog_style::header(ui, &tlf!("Send {what} to {provider}?", what = crate::i18n::t(&task.what), provider = provider.name()), w);
            app.auto.add("aiTask.consent", bar, "Send to the cloud");
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(tlf!(
                    "{app} will send {what} to {provider} with your API key. {provider} handles it under its own terms.",
                    app = crate::brand::APP_NAME,
                    what = crate::i18n::t(&task.what),
                    provider = provider.name()
                ));
                ui.label(egui::RichText::new(tl!("You are asked once per provider; Settings ▸ AI changes the engine.")).color(t.text_dim));
                crate::dialog_style::actions(ui, |ui| {
                    let s = ui.add(crate::dialog_style::primary(tl!("Send")));
                    app.auto.add("aiTask.consent.send", s.rect, "Send");
                    let c = ui.add(crate::dialog_style::secondary(tl!("Cancel")));
                    app.auto.add("aiTask.consent.cancel", c.rect, "Cancel");
                    ok = s.clicked();
                    cancel = c.clicked();
                });
            });
        });
        if ok {
            task.confirmed = true;
            launch(app, task);
        } else if !cancel && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.ui.ai_task = Some(task);
        }
        return;
    }
    let Some(id) = task.job else { return };
    let Some(job) = app.session.jobs.iter().find(|j| j.id == id).cloned() else { return };
    let v = job.to_json();
    if v["finished"] == true {
        match v["result"]["error"].as_str() {
            Some(e) => app.ui.status = e.to_string(),
            None => match app.session.execute(&task.command, task.params.clone()) {
                Ok(r) => {
                    app.ui.status = done_message(&task.label);
                    app.ui.ai_task_result = Some((task.command.clone(), r));
                }
                Err(e) => app.ui.status = e.to_string(),
            },
        }
        return;
    }
    // the status bar and the Progress panel show it (with Cancel), as for exports and renders
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    app.ui.ai_task = Some(task);
}
