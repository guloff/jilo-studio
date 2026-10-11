//! Installing an update from inside the app (`app.downloadUpdate`, `app.installUpdate`,
//! `app.openUpdateImage`): the disk image the last check found ([`crate::updates::Asset`]) is
//! downloaded in a job with progress and Stop, and checked against the size and SHA-256 GitHub
//! publishes for it. On a Mac, the app inside is then checked with `codesign` — a valid signature
//! from the same Developer ID team as this copy, the same bundle identifier and the version the
//! release names — and copied aside, ready.
//!
//! Installing waits for the app to quit (`app.installUpdate` arms it; the host runs
//! [`install_on_exit`] as it exits): a small script waits for this process to end, puts the new
//! app where this one is (keeping this one if anything fails) and, when asked, opens it again. The
//! next launch says whether the update took (`general.updateInstalling`).
//!
//! When the app can't replace itself — a development build, a copy that is not signed with a
//! Developer ID, one macOS runs from a temporary place, one in a folder this user can't write to —
//! the download still happens and the disk image is offered to open, to drag the new version to
//! Applications by hand. Nothing here asks for a password.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p};
use crate::updates::Asset;
use crate::{Result, Session};

/// Where an update is downloaded and checked: [`Session::updates_dir`], else `<data dir>/Updates`.
pub fn updates_dir(s: &Session) -> Option<PathBuf> {
    s.updates_dir.clone().or_else(|| crate::autosave::default_data_dir().map(|d| d.join("Updates")))
}

/// The update in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDownload {
    /// The version being installed.
    pub version: String,
    /// `downloading`, `verifying`, `ready` (checked and copied aside: quit to install), `manual`
    /// (downloaded; the disk image is to be opened and the app dragged to Applications by hand),
    /// `failed` or `cancelled`.
    pub phase: String,
    /// The job downloading and checking it.
    pub job: Option<u64>,
    /// The downloaded disk image.
    pub disk_image: Option<String>,
    /// The checked app, ready to be installed.
    pub staged: Option<String>,
    /// The app it replaces (this copy).
    pub target: Option<String>,
    /// Why it must be installed by hand (`manual`) or why it failed (`failed`).
    pub reason: Option<String>,
    /// Install when the app quits.
    pub install_on_quit: bool,
    /// ... and open the new version then.
    pub relaunch: bool,
}

pub type DownloadState = Arc<Mutex<Option<UpdateDownload>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The update's state as JSON (null: none).
pub fn status_json(s: &Session) -> Value {
    lock(&s.update_download).as_ref().map_or(Value::Null, |d| serde_json::to_value(d).unwrap_or_default())
}

/// `Sha256` over everything written through it.
struct Hashing<W: Write> {
    inner: W,
    hash: sha2::Sha256,
    bytes: u64,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use sha2::Digest;
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn sha256_of(path: &Path) -> Option<String> {
    use sha2::Digest;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn mb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e6)
}

/// Download `asset` into `dir` (a part file, renamed once its size and SHA-256 match). A file
/// already there with the right checksum is used as it is.
pub fn fetch(
    t: &dyn filmcraft_ai::Transport,
    asset: &Asset,
    dir: &Path,
    prog: &filmcraft_export::Progress,
    user_agent: &str,
) -> std::result::Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dest = dir.join(&asset.name);
    if std::fs::metadata(&dest).is_ok_and(|m| m.len() == asset.size) && sha256_of(&dest).as_deref() == Some(asset.sha256.as_str()) {
        prog.done.store(asset.size, Ordering::Relaxed);
        return Ok(dest);
    }
    let part = dir.join(format!("{}.part", asset.name));
    let file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut out = Hashing { inner: std::io::BufWriter::new(file), hash: Default::default(), bytes: 0 };
    let req = filmcraft_ai::transport::Request::get(&asset.url).header("User-Agent", user_agent).header("Accept", "application/octet-stream");
    let mut progress = |done: u64, total: Option<u64>| {
        let total = total.unwrap_or(asset.size).max(1);
        prog.done.store(done, Ordering::Relaxed);
        prog.total.store(total, Ordering::Relaxed);
        *lock(&prog.status) = format!("Downloading… {} of {} MB", mb(done), mb(total));
        // a file far larger than the release says is not the release's
        !prog.cancel.load(Ordering::Relaxed) && done <= asset.size.saturating_mul(2)
    };
    let r = t.download(&req, &mut out, &mut progress);
    let flushed = out.flush();
    let (bytes, sha) = {
        use sha2::Digest;
        let Hashing { hash, bytes, .. } = out;
        (bytes, hash.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>())
    };
    let fail = |e: String| {
        let _ = std::fs::remove_file(&part);
        Err(e)
    };
    match r {
        Err(_) if prog.cancel.load(Ordering::Relaxed) => return fail(filmcraft_ai::transport::CANCELLED.into()),
        Err(e) => return fail(format!("the download stopped: {e}")),
        Ok(status) if !(200..300).contains(&status) => return fail(format!("GitHub answered {status}")),
        Ok(_) => {}
    }
    if let Err(e) = flushed {
        return fail(format!("{}: {e}", part.display()));
    }
    if bytes != asset.size || sha != asset.sha256 {
        return fail("the downloaded file is damaged (its size or checksum is not the release's); try again".into());
    }
    std::fs::rename(&part, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(dest)
}

/// The app bundle this process runs from (`…/Name.app/Contents/MacOS/<exe>`), if it is one.
pub fn running_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let bundle = exe.parent()?.parent()?.parent()?;
    (exe.parent()?.ends_with("Contents/MacOS") && bundle.extension().is_some_and(|e| e == "app")).then(|| bundle.to_path_buf())
}

/// What a checked update is.
#[derive(Clone, Debug, PartialEq)]
pub enum Prepared {
    /// Checked and copied aside: installing replaces `target` with `staged`.
    Ready { staged: PathBuf, target: PathBuf },
    /// The app can't replace itself (why): the disk image is opened instead.
    Manual(String),
}

/// Why `bundle` can't be replaced by the app itself, if it can't.
pub fn install_blocker(bundle: &Path) -> Option<String> {
    let s = bundle.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        return Some("macOS runs this copy from a temporary place; move Jilo Studio to Applications first".into());
    }
    if s.starts_with("/Volumes/") {
        return Some("Jilo Studio is running from its disk image or another disk".into());
    }
    let parent = bundle.parent()?;
    let probe = parent.join(format!(".jilo-update-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(_) => Some("this account can't change the folder Jilo Studio is in".into()),
    }
}

/// Check the disk image `dmg` of `version` against the app at `current` (None: not an app
/// bundle) and copy its app into `dir/Staged`.
#[cfg(target_os = "macos")]
pub fn prepare(dmg: &Path, version: &str, current: Option<&Path>, dir: &Path) -> std::result::Result<Prepared, String> {
    let Some(current) = current else {
        return Ok(Prepared::Manual("this copy of Jilo Studio is not an installed app (a development build)".into()));
    };
    if let Some(why) = install_blocker(current) {
        return Ok(Prepared::Manual(why));
    }
    let Some(team) = mac::team_id(current) else {
        return Ok(Prepared::Manual("this copy of Jilo Studio is not signed with a Developer ID".into()));
    };
    let ident = mac::plist_value(current, "CFBundleIdentifier").ok_or("this copy of Jilo Studio has no bundle identifier")?;
    // the app is copied out of the image without the Finder's extended attributes (the image's
    // window layout leaves some, which a strict signature check refuses), then checked: what is
    // checked is what gets installed
    let staged_dir = dir.join("Staged");
    let _ = std::fs::remove_dir_all(&staged_dir);
    std::fs::create_dir_all(&staged_dir).map_err(|e| format!("{}: {e}", staged_dir.display()))?;
    let staged = {
        let mount = mac::Mount::attach(dmg, &dir.join(format!("Mount-{}", std::process::id())))?;
        let app = mount.app().ok_or("the disk image holds no app")?;
        let staged = staged_dir.join(app.file_name().ok_or("the app has no name")?);
        let plain = ["--norsrc", "--noextattr", "--noqtn", "--noacl"].map(std::ffi::OsStr::new);
        let args: Vec<&std::ffi::OsStr> = plain.iter().copied().chain([app.as_os_str(), staged.as_os_str()]).collect();
        mac::run("/usr/bin/ditto", &args).map_err(|e| format!("copying the new version failed: {e}"))?;
        staged
    };
    let refuse = |e: String| {
        let _ = std::fs::remove_dir_all(&staged_dir);
        Err(e)
    };
    if let Err(e) = mac::verify(&staged) {
        return refuse(e);
    }
    if mac::team_id(&staged).as_deref() != Some(team.as_str()) {
        return refuse("the new version is not signed by the same developer as this one".into());
    }
    if mac::plist_value(&staged, "CFBundleIdentifier").as_deref() != Some(ident.as_str()) {
        return refuse("the disk image holds a different app".into());
    }
    let got = mac::plist_value(&staged, "CFBundleShortVersionString").unwrap_or_default();
    if got != version {
        return refuse(format!("the disk image holds version {got}, not {version}"));
    }
    Ok(Prepared::Ready { staged, target: current.to_path_buf() })
}

#[cfg(not(target_os = "macos"))]
pub fn prepare(_: &Path, _: &str, _: Option<&Path>, _: &Path) -> std::result::Result<Prepared, String> {
    Ok(Prepared::Manual("updates install themselves only on macOS".into()))
}

/// Waits for the app's process to end, puts the new app in place of the old one (the old one
/// comes back if that fails), removes what was downloaded and, with `relaunch`, opens the app.
/// Arguments: pid, new app, app to replace, relaunch (1/0), log file, updates folder.
const INSTALL_SCRIPT: &str = r#"
pid=$1; new=$2; app=$3; relaunch=$4; log=$5; dir=$6
i=0
while kill -0 "$pid" 2>/dev/null; do
  sleep 0.2; i=$((i+1))
  if [ $i -gt 900 ]; then echo "the app did not quit" >>"$log"; exit 1; fi
done
tmp="$app.update-new"; old="$app.update-old"
rm -rf "$tmp" "$old"
if /usr/bin/ditto "$new" "$tmp" && mv "$app" "$old"; then
  if mv "$tmp" "$app"; then
    rm -rf "$old"
    rm -rf "$dir/Staged" "$dir"/*.dmg "$dir"/*.part
    echo "installed $(date)" >>"$log"
  else
    mv "$old" "$app"; rm -rf "$tmp"
    echo "could not put the new version in place" >>"$log"
  fi
else
  rm -rf "$tmp"
  echo "could not copy the new version" >>"$log"
fi
if [ "$relaunch" = 1 ]; then /usr/bin/open "$app"; fi
"#;

/// Start the installer for `staged` → `target`: it runs on after this process exits.
#[cfg(unix)]
pub fn spawn_installer(pid: u32, staged: &Path, target: &Path, relaunch: bool, dir: &Path) -> std::result::Result<(), String> {
    use std::os::unix::process::CommandExt;
    let log = dir.join("install.log");
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(INSTALL_SCRIPT)
        .arg("jilo-update")
        .arg(pid.to_string())
        .arg(staged)
        .arg(target)
        .arg(if relaunch { "1" } else { "0" })
        .arg(&log)
        .arg(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("the installer could not start: {e}"))
}

#[cfg(not(unix))]
pub fn spawn_installer(_: u32, _: &Path, _: &Path, _: bool, _: &Path) -> std::result::Result<(), String> {
    Err("updates install themselves only on macOS".into())
}

/// Run by the host as the app exits: install an armed update. Ok(true) when the installer started.
pub fn install_on_exit(s: &Session) -> std::result::Result<bool, String> {
    let Some(d) = lock(&s.update_download).clone() else { return Ok(false) };
    if d.phase != "ready" || !d.install_on_quit {
        return Ok(false);
    }
    let (Some(staged), Some(target)) = (d.staged.as_deref(), d.target.as_deref()) else { return Ok(false) };
    // the folder it was downloaded to (Staged's parent)
    let Some(dir) = Path::new(staged).parent().and_then(Path::parent) else { return Ok(false) };
    spawn_installer(std::process::id(), Path::new(staged), Path::new(target), d.relaunch, dir)?;
    Ok(true)
}

/// After an update: remove what is left of it.
pub fn clean_up(s: &Session) {
    if let Some(dir) = updates_dir(s) {
        let _ = std::fs::remove_dir_all(dir.join("Staged"));
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "dmg" || x == "part") {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }
}

fn download_update(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "app.downloadUpdate";
    let check = lock(&s.update_check).clone();
    let Some(c) = check.filter(|c| c.newer) else {
        return Err(bad(cmd, "no newer release is known: run app.checkUpdates first"));
    };
    let version = c.latest.clone().unwrap_or_default();
    let asset = c.asset.clone().ok_or_else(|| bad(cmd, "the release has no disk image for this Mac: download it from its page"))?;
    if let Some(d) =
        lock(&s.update_download).as_ref().filter(|d| d.version == version && ["downloading", "verifying", "ready", "manual"].contains(&d.phase.as_str()))
    {
        return Ok(serde_json::to_value(d).unwrap_or_default());
    }
    let dir = match crate::commands::str_p(p, "dir") {
        Some(d) => PathBuf::from(d),
        None => updates_dir(s).ok_or_else(|| bad(cmd, "no data directory to download into"))?,
    };
    let transport = crate::ai::transport(s).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    let wait = bool_p(p, "wait").unwrap_or(false);
    let app_name = "Jilo Studio";
    let job = crate::narration::new_job(s, format!("Downloading {app_name} {version}"));
    job.progress.total.store(asset.size, Ordering::Relaxed);
    let (prog, res, state) = (job.progress.clone(), job.result.clone(), s.update_download.clone());
    *lock(&state) = Some(UpdateDownload { version: version.clone(), phase: "downloading".into(), job: Some(job.id), ..Default::default() });
    let current = running_bundle();
    let agent = format!("JiloStudio/{}", env!("CARGO_PKG_VERSION"));
    let id = crate::narration::spawn(s, job, wait, "filmcraft-update", move || {
        let t0 = web_time::Instant::now();
        let set = |f: &dyn Fn(&mut UpdateDownload)| {
            if let Some(d) = lock(&state).as_mut() {
                f(d);
            }
        };
        let r = fetch(&*transport, &asset, &dir, &prog, &agent).and_then(|dmg| {
            let image = dmg.to_string_lossy().into_owned();
            set(&|d| {
                d.phase = "verifying".into();
                d.disk_image = Some(image.clone());
            });
            *lock(&prog.status) = "Checking the new version…".into();
            prepare(&dmg, &version, current.as_deref(), &dir)
        });
        let out = match r {
            Ok(Prepared::Ready { staged, target }) => {
                set(&|d| {
                    d.phase = "ready".into();
                    d.staged = Some(staged.to_string_lossy().into_owned());
                    d.target = Some(target.to_string_lossy().into_owned());
                });
                Ok(format!("{app_name} {version} is ready to install"))
            }
            Ok(Prepared::Manual(why)) => {
                set(&|d| {
                    d.phase = "manual".into();
                    d.reason = Some(why.clone());
                });
                Ok(format!("{app_name} {version} is downloaded"))
            }
            Err(e) => {
                let cancelled = e == filmcraft_ai::transport::CANCELLED;
                set(&|d| {
                    d.phase = if cancelled { "cancelled" } else { "failed" }.into();
                    d.reason = Some(e.clone());
                });
                Err(if cancelled { "The download was stopped".to_string() } else { e })
            }
        };
        crate::narration::finish(&prog, &res, out, t0);
    })?;
    Ok(json!({"job": id, "version": lock(&s.update_download).as_ref().map(|d| d.version.clone())}))
}

fn install_update(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "app.installUpdate";
    let cancel = bool_p(p, "cancel") == Some(true);
    let relaunch = bool_p(p, "relaunch").unwrap_or(true);
    let version = {
        let mut st = lock(&s.update_download);
        let d = st.as_mut().filter(|d| d.phase == "ready").ok_or_else(|| bad(cmd, "no update is ready: run app.downloadUpdate first"))?;
        d.install_on_quit = !cancel;
        d.relaunch = relaunch && !cancel;
        d.version.clone()
    };
    let marker = if cancel { String::new() } else { version.clone() };
    s.execute("prefs.set", json!({"key": "general.updateInstalling", "value": marker}))?;
    Ok(json!({"armed": !cancel, "relaunch": relaunch && !cancel, "version": version, "quitToInstall": !cancel}))
}

fn open_update_image(s: &mut Session, _: &Value) -> Result<Value> {
    let cmd = "app.openUpdateImage";
    let image = lock(&s.update_download).as_ref().and_then(|d| d.disk_image.clone()).ok_or_else(|| bad(cmd, "no update is downloaded"))?;
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open").arg(&image).spawn().map_err(|e| bad(cmd, e.to_string()))?;
    }
    Ok(json!({"opened": image}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "app.downloadUpdate",
            label: "Download Update",
            menu: &[],
            shortcut: None,
            params: r#"{"wait":bool?,"dir":str?}"#,
            enabled: crate::commands::always,
            run: download_update,
            journal: false,
        },
        CommandSpec {
            id: "app.installUpdate",
            label: "Install Update",
            menu: &[],
            shortcut: None,
            params: r#"{"relaunch":bool=true,"cancel":bool?}"#,
            enabled: crate::commands::always,
            run: install_update,
            journal: false,
        },
        CommandSpec {
            id: "app.openUpdateImage",
            label: "Open Update Disk Image",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: crate::commands::always,
            run: open_update_image,
            journal: false,
        },
    ]
}

/// `codesign`, `hdiutil` and `plutil`, the tools macOS checks and opens apps with.
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    pub fn run(tool: &str, args: &[&OsStr]) -> Result<String, String> {
        let out = std::process::Command::new(tool).args(args).stdin(std::process::Stdio::null()).output().map_err(|e| format!("{tool}: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if out.status.success() { Ok(text) } else { Err(text.lines().last().unwrap_or("failed").trim().to_string()) }
    }

    /// The Developer ID team that signed `app` (None: ad hoc or unsigned).
    pub fn team_id(app: &Path) -> Option<String> {
        let out = run("/usr/bin/codesign", &[OsStr::new("-dv"), OsStr::new("--verbose=2"), app.as_os_str()]).ok()?;
        out.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).map(str::trim).filter(|t| !t.is_empty() && *t != "not set").map(str::to_string)
    }

    /// A valid signature over the whole bundle.
    pub fn verify(app: &Path) -> Result<(), String> {
        run("/usr/bin/codesign", &[OsStr::new("--verify"), OsStr::new("--deep"), OsStr::new("--strict"), app.as_os_str()])
            .map(|_| ())
            .map_err(|e| format!("the new version's signature is not valid: {e}"))
    }

    /// A value of `app`'s Info.plist.
    pub fn plist_value(app: &Path, key: &str) -> Option<String> {
        let plist = app.join("Contents/Info.plist");
        run("/usr/bin/plutil", &[OsStr::new("-extract"), OsStr::new(key), OsStr::new("raw"), OsStr::new("-o"), OsStr::new("-"), plist.as_os_str()])
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// A disk image attached read-only, hidden from Finder; detached when dropped.
    pub struct Mount {
        point: PathBuf,
    }

    impl Mount {
        pub fn attach(dmg: &Path, point: &Path) -> Result<Mount, String> {
            let _ = std::fs::create_dir_all(point);
            run(
                "/usr/bin/hdiutil",
                &[
                    OsStr::new("attach"),
                    OsStr::new("-nobrowse"),
                    OsStr::new("-readonly"),
                    OsStr::new("-noautoopen"),
                    OsStr::new("-mountpoint"),
                    point.as_os_str(),
                    dmg.as_os_str(),
                ],
            )
            .map_err(|e| format!("the disk image could not be opened: {e}"))?;
            Ok(Mount { point: point.to_path_buf() })
        }

        /// The app at the top of the image (the Applications shortcut beside it is a link).
        pub fn app(&self) -> Option<PathBuf> {
            std::fs::read_dir(&self.point)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "app") && std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        }
    }

    impl Drop for Mount {
        fn drop(&mut self) {
            if run("/usr/bin/hdiutil", &[OsStr::new("detach"), self.point.as_os_str(), OsStr::new("-quiet")]).is_err() {
                let _ = run("/usr/bin/hdiutil", &[OsStr::new("detach"), self.point.as_os_str(), OsStr::new("-force"), OsStr::new("-quiet")]);
            }
            let _ = std::fs::remove_dir(&self.point);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_ai::Response;
    use filmcraft_ai::transport::FakeTransport;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("filmcraft-updater-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sha(data: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A session whose last check found release `99.0.0` with a disk image of `image`'s bytes.
    fn session_with_release(image: &[u8], digest: &str) -> (Session, Arc<FakeTransport>) {
        let url = format!("https://github.com/{}/releases/download/v99.0.0/jilo-studio-99.0.0-macos-aarch64.dmg", crate::updates::REPO);
        let release = json!({"tag_name": "v99.0.0", "html_url": format!("{}/tag/v99.0.0", crate::updates::RELEASES_PAGE), "body": "",
            "assets": [{"name": "jilo-studio-99.0.0-macos-aarch64.dmg", "browser_download_url": url, "size": image.len(), "digest": format!("sha256:{digest}")}]});
        let fake = Arc::new(FakeTransport::new());
        let body = release.to_string().into_bytes();
        fake.route(|r| r.url == crate::updates::RELEASES_API, move |_| Ok(Response::new(200, body.clone())));
        let bytes = image.to_vec();
        fake.route(
            move |r| r.url == url,
            move |r| {
                assert!(r.header_value("User-Agent").is_some_and(|v| v.starts_with("JiloStudio/")));
                Ok(Response::new(200, bytes.clone()))
            },
        );
        let mut s = Session { ai_transport: Some(fake.clone()), ..Default::default() };
        let c = s.execute("app.checkUpdates", json!({"wait": true})).unwrap();
        assert_eq!(c["asset"]["size"], image.len(), "{c}");
        (s, fake)
    }

    #[test]
    fn only_this_repositorys_disk_image_with_a_checksum_is_offered() {
        let good = json!({"name": "jilo-studio-1.0.0-macos-aarch64.dmg", "size": 10,
            "browser_download_url": "https://github.com/guloff/jilo-studio/releases/download/v1.0.0/jilo-studio-1.0.0-macos-aarch64.dmg",
            "digest": format!("sha256:{}", "AB".repeat(32))});
        let a = crate::updates::asset_of(&json!({"assets": [good]}), "1.0.0").unwrap();
        assert_eq!((a.size, a.sha256.as_str()), (10, "ab".repeat(32).as_str()));
        let with = |k: &str, v: Value| {
            let mut x = good.clone();
            x[k] = v;
            crate::updates::asset_of(&json!({"assets": [x]}), "1.0.0")
        };
        for url in [
            "https://evil.example/jilo-studio-1.0.0-macos-aarch64.dmg",
            "https://github.com/other/jilo-studio/releases/download/v1/jilo-studio-1.0.0-macos-aarch64.dmg",
            "https://github.com/guloff/jilo-studio/releases/download/../x/jilo-studio-1.0.0-macos-aarch64.dmg",
        ] {
            assert!(with("browser_download_url", json!(url)).is_none(), "{url}");
        }
        assert!(with("digest", Value::Null).is_none(), "no checksum, no download");
        assert!(with("digest", json!("sha1:abc")).is_none());
        assert!(with("size", json!(0)).is_none());
        assert!(with("name", json!("jilo-studio-cli-1.0.0-macos-aarch64.zip")).is_none());
        assert!(crate::updates::asset_of(&json!({"assets": [good]}), "1.0.1").is_none(), "another version's image");
    }

    #[test]
    fn the_disk_image_is_downloaded_checked_and_offered() {
        let image = b"a disk image".repeat(1000);
        let (mut s, fake) = session_with_release(&image, &sha(&image));
        let dir = temp("ok");
        let r = s.execute("app.downloadUpdate", json!({"wait": true, "dir": dir.to_string_lossy()})).unwrap();
        assert!(r["job"].is_u64(), "{r}");
        let st = s.execute("app.updateStatus", json!({})).unwrap();
        let d = &st["download"];
        // a test binary is no app bundle: the image is offered to install by hand
        assert_eq!((d["version"].as_str(), d["phase"].as_str()), (Some("99.0.0"), Some("manual")), "{st}");
        let path = d["diskImage"].as_str().unwrap();
        assert_eq!(std::fs::read(path).unwrap(), image);
        assert!(!dir.join("jilo-studio-99.0.0-macos-aarch64.dmg.part").exists());
        // asked again, nothing is downloaded twice
        let sent = fake.sent().len();
        s.execute("app.downloadUpdate", json!({"wait": true, "dir": dir.to_string_lossy()})).unwrap();
        assert_eq!(fake.sent().len(), sent);
        // a manual update can't be armed
        assert!(s.execute("app.installUpdate", json!({})).is_err());
        assert!(!install_on_exit(&s).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_download_is_thrown_away() {
        let image = b"a disk image".repeat(1000);
        let (mut s, _) = session_with_release(&image, &sha(b"something else"));
        let dir = temp("bad");
        assert!(s.execute("app.downloadUpdate", json!({"wait": true, "dir": dir.to_string_lossy()})).is_ok());
        let d = s.execute("app.updateStatus", json!({})).unwrap()["download"].clone();
        assert_eq!(d["phase"], "failed", "{d}");
        assert!(d["reason"].as_str().unwrap().contains("damaged"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_newer_release_no_download() {
        let mut s = Session { ai_transport: Some(Arc::new(FakeTransport::new())), ..Default::default() };
        assert!(s.execute("app.downloadUpdate", json!({"wait": true})).unwrap_err().to_string().contains("checkUpdates"));
        assert!(s.execute("app.installUpdate", json!({})).is_err());
        assert!(s.execute("app.openUpdateImage", json!({})).is_err());
    }

    #[test]
    fn a_ready_update_is_armed_for_quitting_and_disarmed() {
        let mut s = Session::default();
        *lock(&s.update_download) = Some(UpdateDownload {
            version: "99.0.0".into(),
            phase: "ready".into(),
            staged: Some("/x/Staged/J.app".into()),
            target: Some("/Applications/J.app".into()),
            ..Default::default()
        });
        let r = s.execute("app.installUpdate", json!({})).unwrap();
        assert_eq!((r["armed"].as_bool(), r["relaunch"].as_bool()), (Some(true), Some(true)));
        assert_eq!(s.prefs.general.update_installing, "99.0.0");
        let r = s.execute("app.installUpdate", json!({"cancel": true})).unwrap();
        assert_eq!(r["armed"], false);
        assert_eq!(s.prefs.general.update_installing, "");
        assert!(!install_on_exit(&s).unwrap(), "disarmed: nothing runs at exit");
    }

    #[test]
    fn a_folder_this_account_cannot_write_to_is_said() {
        let dir = temp("blocker");
        let app = dir.join("J.app");
        std::fs::create_dir_all(&app).unwrap();
        assert_eq!(install_blocker(&app), None);
        assert!(install_blocker(Path::new("/private/var/folders/x/AppTranslocation/ABC/d/J.app")).unwrap().contains("temporary"));
        assert!(install_blocker(Path::new("/Volumes/Jilo Studio/J.app")).is_some());
        assert!(install_blocker(Path::new("/System/Applications/J.app")).unwrap().contains("can't change"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End to end on a real release: `JILO_UPDATE_DMG` (a release's disk image) installed over a
    /// copy of `JILO_UPDATE_APP` (an older signed Jilo Studio) — the checks, the copy aside and
    /// the installer script. Prints SKIPPED without them.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_signed_release_replaces_a_copy_of_the_app() {
        let (Some(dmg), Some(app)) = (std::env::var_os("JILO_UPDATE_DMG"), std::env::var_os("JILO_UPDATE_APP")) else {
            eprintln!("SKIPPED: set JILO_UPDATE_DMG and JILO_UPDATE_APP");
            return;
        };
        let (dmg, app) = (PathBuf::from(dmg), PathBuf::from(app));
        let version = dmg.file_name().unwrap().to_string_lossy().trim_start_matches("jilo-studio-").trim_end_matches("-macos-aarch64.dmg").to_string();
        let dir = temp("e2e");
        let install = dir.join("Apps");
        std::fs::create_dir_all(&install).unwrap();
        let copy = install.join(app.file_name().unwrap());
        mac::run("/usr/bin/ditto", &[app.as_os_str(), copy.as_os_str()]).unwrap();
        let before = mac::plist_value(&copy, "CFBundleShortVersionString").unwrap();
        assert_ne!(before, version);
        // a wrong version is refused
        let e = prepare(&dmg, "0.0.1", Some(&copy), &dir).unwrap_err();
        assert!(e.contains("not 0.0.1"), "{e}");
        let Prepared::Ready { staged, target } = prepare(&dmg, &version, Some(&copy), &dir).unwrap() else { panic!("not ready") };
        assert_eq!(target, copy);
        assert!(!dir.join(format!("Mount-{}", std::process::id())).exists(), "the image is detached");
        // a process that has already ended stands for the app that quit
        let mut done = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = done.id();
        done.wait().unwrap();
        spawn_installer(pid, &staged, &target, false, &dir).unwrap();
        let log = dir.join("install.log");
        for _ in 0..300 {
            if std::fs::read_to_string(&log).is_ok_and(|l| !l.is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let l = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(l.starts_with("installed"), "{l}");
        assert_eq!(mac::plist_value(&copy, "CFBundleShortVersionString").unwrap(), version);
        mac::verify(&copy).unwrap();
        assert!(!dir.join("Staged").exists());
        assert!(std::fs::read_dir(&install).unwrap().count() == 1, "no leftovers beside the app");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
