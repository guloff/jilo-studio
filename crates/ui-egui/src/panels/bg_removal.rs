//! AI ▸ Automatic Editing ▸ Remove Background (the AI panel, the clip's quick actions): the
//! selected clip's frames are matted in a job (progress and Stop in the status bar and the
//! Progress panel), then the clip shows them (`clip.removeBackground`).

use serde_json::{Value, json};

use crate::FilmcraftApp;

/// Start on the selected clip (or `params`' clip).
pub fn start(app: &mut FilmcraftApp, params: Value) -> Result<Value, String> {
    let mut bg = params.clone();
    bg["background"] = json!(true);
    match app.session.execute("clip.removeBackground", bg) {
        Ok(r) if r["background"] == true => {
            let clip = app.session.state.selection.first().map(|c| c.0);
            let mut p = params;
            if p.get("clip").is_none()
                && let Some(c) = clip
            {
                p["clip"] = json!(c);
            }
            app.ui.bg_removal = r["job"].as_u64().map(|j| (j, p));
            app.ui.status = tl!("Removing the background: every frame is analysed on this computer…").to_string();
            Ok(r)
        }
        Ok(r) => {
            app.ui.status = tl!("The background is removed.").to_string();
            Ok(r)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Put the result in when the job ends.
pub fn show(app: &mut FilmcraftApp) {
    let Some((id, params)) = app.ui.bg_removal.clone() else { return };
    match app.session.jobs.iter().find(|j| j.id == id).map(|j| j.to_json()) {
        Some(j) if j["finished"] != true => {}
        Some(j) if j["result"]["error"].is_string() => {
            app.ui.bg_removal = None;
            app.ui.status = j["result"]["error"].as_str().unwrap_or_default().to_string();
        }
        _ => {
            app.ui.bg_removal = None;
            app.ui.status = match app.session.execute("clip.removeBackground", params) {
                Ok(_) => tl!("The background is removed.").to_string(),
                Err(e) => e.to_string(),
            };
        }
    }
}
