//! `filmcraft --process '<steps>' <files…>`: process files without a window — a Finder Quick
//! Action or a Shortcuts "Run Shell Script" step runs this (`batch.processFiles`). The steps are
//! `batch.start`'s parameters as JSON (empty: just export). The result is printed as JSON; when the
//! app runs from its bundle a notification says how it went. Exit status 0 when every file was
//! processed and exported, 1 otherwise.

use serde_json::{Value, json};

/// Run the steps on the files; returns the exit status.
pub fn run(params: &str, files: &[String], data_dir: Option<std::path::PathBuf>) -> i32 {
    let mut p: Value = match serde_json::from_str(if params.trim().is_empty() { "{}" } else { params }) {
        Ok(v @ Value::Object(_)) => v,
        _ => {
            eprintln!("filmcraft --process: the steps must be a JSON object, got `{params}`");
            return 2;
        }
    };
    let paths: Vec<String> = files.iter().map(|f| std::fs::canonicalize(f).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| f.clone())).collect();
    if paths.is_empty() {
        eprintln!("filmcraft --process: no files");
        return 2;
    }
    p["paths"] = json!(paths);
    let mut s = filmcraft_engine::Session::default();
    s.face_detector = crate::face_detector();
    s.image_classifier = crate::image_classifier();
    s.emoji_renderer = crate::emoji_renderer();
    s.cutout = crate::cutout();
    // the user's preferences, export presets and keys, read only
    if let Some(dir) = data_dir.or_else(filmcraft_engine::autosave::default_data_dir) {
        s.prefs = filmcraft_engine::autosave::Preferences::load(&dir.join("preferences.json"));
        s.export_presets.set_dir(&dir);
    }
    let (code, summary) = match s.execute("batch.processFiles", p) {
        Ok(v) => {
            println!("{v}");
            let n = v["exports"].as_array().map_or(0, Vec::len);
            if v["ok"] == true {
                (0, format!("{} files processed, {n} exported", paths.len()))
            } else {
                (1, format!("Some files could not be processed ({n} exported)"))
            }
        }
        Err(e) => {
            eprintln!("filmcraft --process: {e}");
            (1, e.to_string())
        }
    };
    if filmcraft_platform::notify::available() {
        let _ = filmcraft_platform::notify::send(filmcraft_ui_egui::brand::APP_NAME, &summary);
    }
    code
}
