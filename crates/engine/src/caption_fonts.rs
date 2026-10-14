//! Caption fonts: a pack of open fonts popular for video captions, downloaded on request (the
//! fonts are not in the repository or the app bundle). Every family in it covers Russian and Tajik
//! (ҳ қ ӯ ӣ ҷ ғ) as well as Latin. Files are pinned to a commit of Google Fonts' repository
//! (github.com/google/fonts, SIL Open Font License 1.1), checked against their SHA-256 and kept
//! with their licences in `<data dir>/fonts/captions/`; the folder is added to the font database
//! when a session starts and right after the download.
//!
//! | Command | |
//! |---|---|
//! | `fonts.captionPack` | whether the pack is installed, its families, size, source and licence |
//! | `fonts.downloadCaptionPack` | download the missing files in a background job (`wait: true` blocks) |

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad};
use crate::{Result, Session};

/// The Google Fonts commit the files are pinned to.
const COMMIT: &str = "e9678a5385322c63730122ba464765baad83cfd8";

macro_rules! file {
    ($name:literal, $path:literal, $sha:literal, $size:literal) => {
        PackFile { name: $name, path: $path, sha256: $sha, size: $size }
    };
}

/// One file of the pack: `path` inside google/fonts at [`COMMIT`].
pub struct PackFile {
    pub name: &'static str,
    pub path: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

/// The families of the pack, as the font menus show them.
pub const FAMILIES: [&str; 8] = ["Montserrat", "Roboto", "Oswald", "Exo 2", "Nunito", "Raleway", "Fira Sans Condensed", "Caveat"];

pub const FILES: [PackFile; 18] = [
    file!("Montserrat[wght].ttf", "ofl/montserrat/Montserrat%5Bwght%5D.ttf", "0f7b311b2f3279e4eef9b2f968bcdbab6e28f4daeb1f049f4f278a902bcd82f7", 744936),
    file!("Roboto[wdth,wght].ttf", "ofl/roboto/Roboto%5Bwdth%2Cwght%5D.ttf", "d7598e12c5dbef095ff8272cfc55da0250bd07fbdecbac8a530b9b277872a134", 488584),
    file!("Oswald[wght].ttf", "ofl/oswald/Oswald%5Bwght%5D.ttf", "5b38c246e255a12f5712d640d56bcced0472466fc68983d2d0410ec0457c2817", 172088),
    file!("Exo2[wght].ttf", "ofl/exo2/Exo2%5Bwght%5D.ttf", "205a448676a2586f9c57c25f3d5c58ca8db7e6cf5edf7506783a010c6fe2bfb5", 303940),
    file!("Nunito[wght].ttf", "ofl/nunito/Nunito%5Bwght%5D.ttf", "bb55a5ca5c2042335b3991af27c4d0705d0ef41cac6164ac737fd8f2a1e85207", 276932),
    file!("Raleway[wght].ttf", "ofl/raleway/Raleway%5Bwght%5D.ttf", "8bbcc3eb8275c388f4bcd998832f8a4b943eadbaf6a595205312774b5951aefb", 312352),
    file!(
        "FiraSansCondensed-Regular.ttf",
        "ofl/firasanscondensed/FiraSansCondensed-Regular.ttf",
        "c67609f14fc9d328442bfffcc31988b0f76e5575cf460fcc1f3dd8e4912fe257",
        459804
    ),
    file!(
        "FiraSansCondensed-Bold.ttf",
        "ofl/firasanscondensed/FiraSansCondensed-Bold.ttf",
        "b9371052e58ec5b709153dd0fffc60b5205b4d3435cf957c2522705b31b0dbcc",
        489176
    ),
    file!(
        "FiraSansCondensed-Black.ttf",
        "ofl/firasanscondensed/FiraSansCondensed-Black.ttf",
        "44a13b01570201ef0ba67a603f227166485ef93d2e1bd5d6e1b60d0be14d0975",
        516292
    ),
    file!("Caveat[wght].ttf", "ofl/caveat/Caveat%5Bwght%5D.ttf", "0bdb6b660482d31531b3945849fba5916b3ef8695da7024a9e6b9ee3c4157988", 403648),
    file!("OFL-Montserrat.txt", "ofl/montserrat/OFL.txt", "8b7141c03fa4f8d44e6345d5d4931709290f0f67875e452e95ac1fd3a027802e", 4400),
    file!("OFL-Roboto.txt", "ofl/roboto/OFL.txt", "061402327a96aadb0bfb694a960ed289ecd38d383e396243831ab81feb109c41", 4394),
    file!("OFL-Oswald.txt", "ofl/oswald/OFL.txt", "0fd731a904b729a4e02eaf5e8ebd06783edd9abe400e8882760160230675b652", 4483),
    file!("OFL-Exo2.txt", "ofl/exo2/OFL.txt", "7f95a8eeecc6904ffc81cd10cc8ebc7fcee72a67922924bf45f02dff03b88127", 4386),
    file!("OFL-Nunito.txt", "ofl/nunito/OFL.txt", "580df76c95a1ec5ab878ceb25bb3d85c6a076804e9c970c8c6972aea775fdf65", 4385),
    file!("OFL-Raleway.txt", "ofl/raleway/OFL.txt", "7e946cf1171784d1015279e7dc35f827957a6b5d1f1f659ae0c98e5f5e37ed9b", 4497),
    file!("OFL-FiraSansCondensed.txt", "ofl/firasanscondensed/OFL.txt", "8f24842e9174beda18a556c2ae7d54f5dc444340c19a3a9ef77e23bca366adbd", 4370),
    file!("OFL-Caveat.txt", "ofl/caveat/OFL.txt", "1f9d81d094273d82f3898a1ee8b598a717d050ecbf5ff7bede105b704880157b", 4385),
];

pub const SOURCE: &str = "https://github.com/google/fonts";
pub const LICENSE: &str = "SIL Open Font License 1.1";

/// Where the pack lives (`<data dir>/fonts/captions`).
pub fn dir() -> Option<PathBuf> {
    crate::autosave::default_data_dir().map(|d| d.join("fonts").join("captions"))
}

/// Total download size in bytes.
pub fn size() -> u64 {
    FILES.iter().map(|f| f.size).sum()
}

/// Whether every file of the pack is present with its size.
pub fn installed() -> bool {
    dir().is_some_and(|d| FILES.iter().all(|f| std::fs::metadata(d.join(f.name)).is_ok_and(|m| m.len() == f.size)))
}

/// Add the pack's folder to the font database (once per process; again after a download).
pub fn register() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if let Some(d) = dir().filter(|d| d.is_dir()) {
            filmcraft_text::fonts::add_dir(d);
        }
    });
}

fn status(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!({
        "installed": installed(),
        "families": FAMILIES,
        "bytes": size(),
        "source": SOURCE,
        "commit": COMMIT,
        "license": LICENSE,
        "dir": dir().map(|d| d.display().to_string()),
    }))
}

fn download(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "fonts.downloadCaptionPack";
    let dir = dir().ok_or_else(|| bad(cmd, "no data directory for fonts"))?;
    if installed() {
        filmcraft_text::fonts::add_dir(&dir);
        return Ok(json!({"job": null, "installed": true}));
    }
    if !cfg!(feature = "speech-download") {
        return Err(bad(cmd, "downloads are not available in this build (built without the `speech-download` feature)"));
    }
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let job = crate::narration::new_job(s, "Downloading caption fonts".into());
    job.progress.total.store(size(), std::sync::atomic::Ordering::Relaxed);
    let (prog, res) = (job.progress.clone(), job.result.clone());
    let id = crate::narration::spawn(s, job, wait, "filmcraft-font-download", move || {
        let t0 = web_time::Instant::now();
        let r = fetch(&dir, &prog).map(|()| {
            filmcraft_text::fonts::add_dir(&dir);
            "Caption fonts installed".to_string()
        });
        crate::narration::finish(&prog, &res, r, t0);
    })?;
    Ok(json!({"job": id, "installed": false}))
}

#[cfg(feature = "speech-download")]
fn fetch(dir: &std::path::Path, prog: &filmcraft_export::Progress) -> std::result::Result<(), String> {
    use std::sync::PoisonError;
    use std::sync::atomic::Ordering;
    // the URLs are built once and live for the process (the downloader takes `&'static str`)
    static URLS: OnceLock<Vec<String>> = OnceLock::new();
    let urls = URLS.get_or_init(|| FILES.iter().map(|f| format!("https://raw.githubusercontent.com/google/fonts/{COMMIT}/{}", f.path)).collect());
    let files: Vec<filmcraft_speech::models::ModelFile> =
        FILES.iter().zip(urls).map(|(f, url)| filmcraft_speech::models::ModelFile { name: f.name, url, sha256: f.sha256, size: f.size }).collect();
    let mut progress = |done: u64, total: u64, file: &str| {
        prog.done.store(done, Ordering::Relaxed);
        prog.total.store(total, Ordering::Relaxed);
        *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Downloading {file}");
        !prog.cancel.load(Ordering::Relaxed)
    };
    filmcraft_speech::models::download_files(dir, &files, &mut progress).map_err(|e| e.to_string())
}

#[cfg(not(feature = "speech-download"))]
fn fetch(_: &std::path::Path, _: &filmcraft_export::Progress) -> std::result::Result<(), String> {
    Err("downloads are not available in this build".into())
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec { id: "fonts.captionPack", label: "Caption Fonts", menu: &[], shortcut: None, params: "{}", enabled: always, run: status, journal: false },
        CommandSpec {
            id: "fonts.downloadCaptionPack",
            label: "Download Caption Fonts",
            menu: &[],
            shortcut: None,
            params: r#"{"wait":bool?}"#,
            enabled: always,
            run: download,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pack_is_pinned_and_small() {
        let mut names: Vec<&str> = FILES.iter().map(|f| f.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FILES.len(), "file names are unique");
        for f in &FILES {
            assert_eq!(f.sha256.len(), 64, "{}", f.name);
            assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{}", f.name);
        }
        assert!(size() < 8_000_000, "{}", size());
        assert_eq!(COMMIT.len(), 40);
    }
}
