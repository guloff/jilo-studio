//! The inbox folder: media files that appear in a watched folder are imported into an "Inbox" bin
//! and run through a batch (`batch.start` with the inbox's steps: transcribe, captions, a recipe, a
//! brand kit, formats, export…), with no one at the keyboard.
//!
//! | Command | |
//! |---|---|
//! | `inbox.get` | the inbox settings and what it has taken in |
//! | `inbox.set` | `{enabled?, folder?, batch?}`: `batch` is `batch.start`'s parameters without `items` |
//! | `inbox.scan` | look at the folder now (`settle: false` takes files without waiting for them to stop growing) |
//!
//! The settings live in the preferences (`inbox.*`). A file is taken once its size has stayed the
//! same for one look (a copy in progress is left for later), and only once (`seen`). While a
//! batch runs, new files wait for it to end. Looked at every [`SCAN_SECONDS`] seconds, by
//! [`pump`].

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p};
use crate::{EngineError, Result, Session};

/// Seconds between looks at the folder.
pub const SCAN_SECONDS: f64 = 5.0;

/// Files remembered as taken in (the oldest are forgotten past this).
const SEEN_LIMIT: usize = 2000;

/// The media files the inbox takes, by extension.
const MEDIA: [&str; 19] =
    ["mov", "mp4", "m4v", "mkv", "webm", "avi", "mxf", "mts", "m2ts", "wav", "mp3", "m4a", "aac", "flac", "aif", "aiff", "png", "jpg", "jpeg"];

/// Name of the bin new files go to.
pub const BIN: &str = "Inbox";

/// The inbox settings (`Preferences::inbox`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct InboxPrefs {
    pub enabled: bool,
    pub folder: String,
    /// `batch.start` parameters without `items`.
    pub batch: Value,
    /// Files already taken in.
    pub seen: Vec<String>,
}

/// What the inbox is watching between looks.
#[derive(Debug, Default)]
pub struct InboxState {
    last: Option<web_time::Instant>,
    /// Files found but not yet taken, with their size at the last look.
    pending: Vec<(String, u64)>,
    /// Files taken in and waiting for the running batch to end.
    waiting: Vec<String>,
    /// What the last look did (for `inbox.get`).
    pub last_message: String,
}

fn is_media(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| MEDIA.contains(&e.to_ascii_lowercase().as_str()))
}

/// The media files in the folder now, with their sizes (hidden files and folders left out).
fn listing(folder: &str) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = std::fs::read_dir(folder)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_media(&e.path()))
                .filter_map(|e| Some((e.path().to_string_lossy().to_string(), e.metadata().ok()?.len())))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Look at the folder: files whose size has not changed since the last look (or every new file,
/// unless `settle`) are imported and, when no batch is running, processed. Returns what happened.
pub fn scan(s: &mut Session, settle: bool) -> Result<Value> {
    let cfg = s.prefs.inbox.clone();
    if cfg.folder.is_empty() {
        return Err(bad("inbox.scan", "no inbox folder: inbox.set {\"folder\": …}"));
    }
    if !std::path::Path::new(&cfg.folder).is_dir() {
        return Err(EngineError::Other(format!("the inbox folder is not there: {}", cfg.folder)));
    }
    let now = listing(&cfg.folder);
    let mut ready: Vec<String> = Vec::new();
    let mut pending: Vec<(String, u64)> = Vec::new();
    for (path, size) in now {
        if cfg.seen.contains(&path) {
            continue;
        }
        let steady = s.inbox.pending.iter().any(|(p, n)| *p == path && *n == size);
        if size > 0 && (steady || !settle) {
            ready.push(path);
        } else {
            pending.push((path, size));
        }
    }
    s.inbox.pending = pending;
    let mut imported: Vec<u64> = Vec::new();
    if !ready.is_empty() {
        let bin = inbox_bin(s);
        let r = s.execute("file.import", json!({"paths": ready, "bin": bin}))?;
        imported.extend(r["items"].as_array().into_iter().flatten().filter_map(Value::as_u64));
        imported.extend(r["duplicates"].as_array().into_iter().flatten().filter_map(|d| d["item"].as_u64()));
        let mut prefs = s.prefs.clone();
        prefs.inbox.seen.extend(ready.iter().cloned());
        let over = prefs.inbox.seen.len().saturating_sub(SEEN_LIMIT);
        prefs.inbox.seen.drain(..over);
        s.set_prefs(prefs).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
        s.inbox.waiting.extend(imported.iter().map(|i| i.to_string()));
    }
    let started = start_waiting(s)?;
    let msg = match (ready.len(), started) {
        (0, None) => String::new(),
        (n, None) => format!("Inbox: {n} new files imported"),
        (_, Some(n)) => format!("Inbox: processing {n} new files"),
    };
    if !msg.is_empty() {
        s.inbox.last_message = msg.clone();
        s.log.push(crate::panels::Level::Info, "inbox", msg.clone());
    }
    Ok(json!({"imported": imported, "pending": s.inbox.pending.len(), "batch": started, "message": msg}))
}

/// The id of the Inbox bin, made if needed.
fn inbox_bin(s: &mut Session) -> Option<u64> {
    let find = |s: &Session| {
        s.project.root.children.iter().find_map(|e| match e {
            filmcraft_project::BinEntry::Bin(b) if b.name == BIN => Some(b.id.0),
            _ => None,
        })
    };
    if let Some(b) = find(s) {
        return Some(b);
    }
    let _ = s.execute("file.newBin", json!({"name": BIN}));
    find(s)
}

/// Start a batch on the files waiting, when one can run and the inbox has steps. Returns how
/// many files it took.
fn start_waiting(s: &mut Session) -> Result<Option<usize>> {
    if s.inbox.waiting.is_empty() || s.batch.as_ref().is_some_and(|b| b.to_json()["finished"] != true) {
        return Ok(None);
    }
    let mut p = match &s.prefs.inbox.batch {
        Value::Object(m) if !m.is_empty() => Value::Object(m.clone()),
        _ => return Ok(None),
    };
    let items: Vec<u64> = std::mem::take(&mut s.inbox.waiting).iter().filter_map(|i| i.parse().ok()).collect();
    let n = items.len();
    p["items"] = json!(items);
    p["wait"] = json!(false);
    s.execute("batch.start", p)?;
    Ok(Some(n))
}

/// Look at the inbox folder when it is time (call regularly, as `poll_persistence` does).
pub fn pump(s: &mut Session) {
    if !s.prefs.inbox.enabled || s.prefs.inbox.folder.is_empty() {
        return;
    }
    let now = web_time::Instant::now();
    if s.inbox.last.is_some_and(|t| now.duration_since(t).as_secs_f64() < SCAN_SECONDS) {
        return;
    }
    s.inbox.last = Some(now);
    if let Err(e) = scan(s, true) {
        let m = e.to_string();
        if s.inbox.last_message != m {
            s.log.push(crate::panels::Level::Warning, "inbox", m.clone());
            s.inbox.last_message = m;
        }
    }
}

fn get(s: &mut Session, _: &Value) -> Result<Value> {
    let c = &s.prefs.inbox;
    Ok(
        json!({"enabled": c.enabled, "folder": c.folder, "batch": c.batch, "seen": c.seen.len(), "pending": s.inbox.pending.len(), "message": s.inbox.last_message}),
    )
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inbox.set";
    let mut prefs = s.prefs.clone();
    if let Some(f) = str_p(p, "folder") {
        let f = crate::export_tools::expand_home(f);
        if !f.is_empty() && !std::path::Path::new(&f).is_dir() {
            return Err(bad(cmd, format!("`folder`: no folder {f}")));
        }
        if f != prefs.inbox.folder {
            // files already in a newly chosen folder are not new
            prefs.inbox.seen = listing(&f).into_iter().map(|x| x.0).collect();
            s.inbox = InboxState::default();
        }
        prefs.inbox.folder = f;
    }
    if let Some(b) = p.get("batch") {
        if !b.is_object() && !b.is_null() {
            return Err(bad(cmd, "`batch`: batch.start's parameters"));
        }
        let mut b = b.clone();
        if let Some(m) = b.as_object_mut() {
            m.remove("items");
        }
        prefs.inbox.batch = b;
    }
    if let Some(on) = bool_p(p, "enabled") {
        if on && prefs.inbox.folder.is_empty() {
            return Err(bad(cmd, "choose the inbox folder first"));
        }
        prefs.inbox.enabled = on;
    }
    if prefs != s.prefs {
        s.set_prefs(prefs).map_err(|e| EngineError::Other(format!("saving preferences: {e}")))?;
    }
    get(s, &json!({}))
}

fn scan_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    scan(s, bool_p(p, "settle").unwrap_or(true))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec { id: "inbox.get", label: "Inbox Folder Settings", menu: &[], shortcut: None, params: "{}", enabled: always, run: get, journal: false },
        CommandSpec {
            id: "inbox.set",
            label: "Set Inbox Folder",
            menu: &[],
            shortcut: None,
            params: r#"{"enabled":bool?,"folder":dir?,"batch":{batch.start parameters without items}?}"#,
            enabled: always,
            run: set,
            journal: false,
        },
        CommandSpec {
            id: "inbox.scan",
            label: "Check Inbox Folder",
            menu: &[],
            shortcut: None,
            params: r#"{"settle":bool=true}"#,
            enabled: always,
            run: scan_cmd,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_are_imported_once_settled_and_processed() {
        let dir = std::env::temp_dir().join(format!("jilo-inbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(64, 36, image::Rgba([1, 2, 3, 255])).save(dir.join("old.png")).unwrap();
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let folder = dir.to_string_lossy().to_string();
        assert!(s.execute("inbox.set", json!({"enabled": true})).is_err(), "no folder yet");
        s.execute("inbox.set", json!({"folder": folder, "enabled": true, "batch": {"export": true, "preset": "Waveform Audio 48 kHz 16-bit", "path": dir.join("out").to_string_lossy()}})).unwrap();
        // what was there before is not new
        let r = s.execute("inbox.scan", json!({})).unwrap();
        assert_eq!(r["imported"], json!([]));
        // a new file is taken on the look after the one that found it
        image::RgbaImage::from_pixel(64, 36, image::Rgba([9, 9, 9, 255])).save(dir.join("new.png")).unwrap();
        let r = s.execute("inbox.scan", json!({})).unwrap();
        assert_eq!((r["imported"].as_array().map(Vec::len), r["pending"].as_u64()), (Some(0), Some(1)), "{r}");
        let r = s.execute("inbox.scan", json!({})).unwrap();
        assert_eq!(r["imported"].as_array().map(Vec::len), Some(1), "{r}");
        assert_eq!(r["batch"], 1, "{r}");
        let item = filmcraft_project::ItemId(r["imported"][0].as_u64().unwrap());
        assert!(s.project.root.children.iter().any(|e| matches!(e, filmcraft_project::BinEntry::Bin(b) if b.name == BIN)), "the Inbox bin");
        assert!(s.project.item(item).is_some());
        // the batch is running on it; once taken, a file is never taken again
        assert!(s.batch.is_some());
        let r = s.execute("inbox.scan", json!({"settle": false})).unwrap();
        assert_eq!(r["imported"], json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
