//! A system notification when an export or a long AI task ends while Jilo Studio is in the
//! background (Settings ▸ General ▸ "Notify when…", on by default), through
//! [`crate::HostHooks::notify`]. Jobs that end within a few seconds, and jobs that end while the
//! window is in front, notify nobody: the status bar says it already.

use std::collections::HashMap;

use web_time::Instant;

use crate::FilmcraftApp;

/// A job that ran at least this long (seconds) is worth a notification.
pub const MIN_SECONDS: f64 = 8.0;

/// The running jobs and when they were first seen.
pub struct Watch {
    running: HashMap<u64, Instant>,
    /// Shortest job that notifies, in seconds ([`MIN_SECONDS`]).
    pub min_seconds: f64,
}

impl Default for Watch {
    fn default() -> Self {
        Self { running: HashMap::new(), min_seconds: MIN_SECONDS }
    }
}

/// What a finished job says: (title, body).
pub fn message(label: &str, error: Option<&str>) -> (String, String) {
    let title = crate::brand::APP_NAME.to_string();
    let body = match error {
        Some(e) => tlf!("{task} stopped: {error}", task = crate::i18n::t(label), error = e),
        None => tlf!("{task} is done", task = crate::i18n::t(label)),
    };
    (title, body)
}

/// Note running jobs; notify about the ones that ended (call once a frame).
pub fn watch(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let now = Instant::now();
    let mut ended: Vec<(String, Option<String>)> = Vec::new();
    for j in &app.session.jobs {
        let v = j.to_json();
        let finished = v["finished"] == true;
        match (app.notify_watch.running.get(&j.id).copied(), finished) {
            (None, false) => {
                app.notify_watch.running.insert(j.id, now);
            }
            (Some(since), true) => {
                app.notify_watch.running.remove(&j.id);
                if now.duration_since(since).as_secs_f64() >= app.notify_watch.min_seconds {
                    ended.push((j.label.clone(), v["result"]["error"].as_str().map(str::to_string)));
                }
            }
            _ => {}
        }
    }
    // jobs that left the list
    let ids: Vec<u64> = app.session.jobs.iter().map(|j| j.id).collect();
    app.notify_watch.running.retain(|id, _| ids.contains(id));
    if ended.is_empty() || !app.session.prefs.general.notify_when_done {
        return;
    }
    let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
    if focused {
        return;
    }
    if let Some(n) = app.hooks.notify.as_mut() {
        for (label, error) in ended {
            let (title, body) = message(&label, error.as_deref());
            n(&title, &body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_say_what_ended_and_how() {
        let (t, b) = message("Export", None);
        assert_eq!(t, "Jilo Studio");
        assert!(b.contains("Export") && b.contains("done"), "{b}");
        let (_, b) = message("Export", Some("disk full"));
        assert!(b.contains("disk full"), "{b}");
    }
}
