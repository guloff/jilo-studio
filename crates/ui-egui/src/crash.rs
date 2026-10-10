//! Panic reporting: a panic anywhere (UI pass, frame workers, background jobs) is written to a
//! crash log with its thread, location and backtrace, and the last one is kept for the in-app
//! error window. Without this a panic on the UI thread closes the app silently and one in a
//! worker leaves a monitor blank with no trace.

use std::sync::Mutex;

/// The most recent panic, as shown to the user: message, `file:line`, thread.
static LAST: Mutex<Option<String>> = Mutex::new(None);
/// Where crash logs go (desktop); `None` = stderr only (web, tests).
static LOG_DIR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// Install the panic hook (idempotent). Panics are also passed to the previous hook, so they
/// still reach stderr / the browser console.
pub fn install(log_dir: Option<std::path::PathBuf>) {
    *LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()) = log_dir;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
            let summary = format!("{msg} (at {at}, thread {thread})");
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(summary.clone());
            write_log(&summary);
            prev(info);
        }));
    });
}

/// Deliberately panic: the fault-injection hooks (`ui.injectPanic` and the frame-job equivalent)
/// use this to prove the guard above keeps the session alive.
#[allow(clippy::panic)]
pub fn injected_fault(what: &str) -> ! {
    panic!("{what}")
}

/// Write a non-panic fatal error (e.g. the window could not be created) to the crash log.
pub fn record(msg: &str) {
    write_log(msg);
}

/// Take the last panic summary (the error window shows it once).
pub fn take_last() -> Option<String> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The crash log file for today, if logging to disk.
pub fn log_path() -> Option<std::path::PathBuf> {
    let dir = LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Some(dir.join(format!("crash-{}.log", secs / 86_400)))
}

#[cfg(not(target_arch = "wasm32"))]
fn write_log(summary: &str) {
    use std::io::Write;
    let Some(path) = log_path() else { return };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let bt = std::backtrace::Backtrace::force_capture();
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "=== {} FilmCraft {} (unix {secs})\n{summary}\n{bt}\n", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    }
}

#[cfg(target_arch = "wasm32")]
fn write_log(_summary: &str) {}

// ------------------------------------------------------------------ the last run's end

/// A report on the previous run that ended without quitting (a crash, a forced quit, a power
/// cut), found by [`begin_session`]; the window that offers it takes it once.
static PREVIOUS: Mutex<Option<String>> = Mutex::new(None);
/// This run's marker: `<log dir>/Sessions/<pid>.lock`, locked while the app runs.
#[cfg(not(target_arch = "wasm32"))]
static SESSION: Mutex<Option<(std::path::PathBuf, std::fs::File)>> = Mutex::new(None);

/// Panic entries of the crash logs in `dir` written at or after `since` (unix seconds), the
/// newest last, each cut to its first `lines` lines.
#[cfg(not(target_arch = "wasm32"))]
fn panics_since(dir: &std::path::Path, since: u64, lines: usize) -> Vec<String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log")))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for entry in text.split("=== ").filter(|e| !e.trim().is_empty()) {
            let secs = entry.lines().next().and_then(|l| l.rsplit("(unix ").next()).and_then(|t| t.trim_end_matches(')').trim().parse::<u64>().ok());
            if secs.is_some_and(|t| t >= since) {
                out.push(format!("=== {}", entry.lines().take(lines).collect::<Vec<_>>().join("\n")));
            }
        }
    }
    out
}

/// macOS's own crash reports of this app (`~/Library/Logs/DiagnosticReports`) newer than `since`.
#[cfg(not(target_arch = "wasm32"))]
fn system_reports_since(since: u64) -> Vec<std::path::PathBuf> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let dir = std::path::Path::new(&home).join("Library/Logs/DiagnosticReports");
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| (n.starts_with("Jilo Studio") || n.starts_with("filmcraft")) && n.ends_with(".ips")))
        .filter(|e| {
            e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).is_some_and(|d| d.as_secs() >= since)
        })
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

/// Start this run's marker in `<log dir>/Sessions` and look for runs that ended without quitting:
/// a marker whose lock is free belongs to a process that is gone without [`end_session`]. Their
/// report (version, start, the panics logged since, macOS's crash reports) is kept for the window.
/// Returns it too.
#[cfg(not(target_arch = "wasm32"))]
pub fn begin_session() -> Option<String> {
    let dir = LOG_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let sessions = dir.join("Sessions");
    let _ = std::fs::create_dir_all(&sessions);
    let mut ended: Vec<(u64, String)> = Vec::new();
    for e in std::fs::read_dir(&sessions).into_iter().flatten().flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "lock") {
            continue;
        }
        let Ok(f) = std::fs::File::options().read(true).write(true).open(&path) else { continue };
        if f.try_lock().is_err() {
            continue; // another copy of the app is running
        }
        let meta = std::fs::read_to_string(&path).unwrap_or_default();
        let field = |k: &str| meta.lines().find_map(|l| l.strip_prefix(k)).map(str::trim).unwrap_or_default().to_string();
        ended.push((field("started ").parse().unwrap_or(0), field("version ")));
        drop(f);
        let _ = std::fs::remove_file(&path);
    }
    let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mine = sessions.join(format!("{}.lock", std::process::id()));
    if let Ok(f) = std::fs::File::create(&mine) {
        use std::io::Write;
        let _ = f.try_lock();
        let _ = write!(&f, "version {}\nstarted {now}\n", env!("CARGO_PKG_VERSION"));
        *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Some((mine, f));
    }
    let (since, version) = ended.into_iter().max()?;
    let mut report =
        format!("Jilo Studio {version} (started at unix {since}) quit unexpectedly.\nThis run: Jilo Studio {} on macOS.\n", env!("CARGO_PKG_VERSION"));
    let panics = panics_since(&dir, since, 40);
    if panics.is_empty() {
        report.push_str("\nNo panic was logged: the app was stopped from outside, or crashed in system code.\n");
    }
    for p in panics.iter().rev().take(3).rev() {
        report.push('\n');
        report.push_str(p);
        report.push('\n');
    }
    for r in system_reports_since(since) {
        report.push_str(&format!("\nmacOS crash report: {}\n", r.display()));
    }
    *PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(report.clone());
    Some(report)
}

#[cfg(target_arch = "wasm32")]
pub fn begin_session() -> Option<String> {
    None
}

/// End this run's marker (the app quits normally).
pub fn end_session() {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some((path, f)) = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take() {
        drop(f);
        let _ = std::fs::remove_file(path);
    }
}

/// Take the report on the previous run's unexpected end (shown once).
pub fn take_previous() -> Option<String> {
    PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Set the report the window shows (UI tests).
#[doc(hidden)]
pub fn set_previous(report: Option<String>) {
    *PREVIOUS.lock().unwrap_or_else(|e| e.into_inner()) = report;
}

#[cfg(test)]
mod tests {
    /// The log directory is process-global: the tests that set it take turns.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A run whose marker is left unlocked ended without quitting: the next run reports it with
    /// the panics logged since it started, once; a normal quit leaves nothing to report.
    #[test]
    fn a_run_that_did_not_quit_is_reported_once() {
        let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("fc-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sessions")).unwrap();
        std::fs::write(dir.join("Sessions/99999.lock"), "version 0.5.9\nstarted 1000\n").unwrap();
        std::fs::write(
            dir.join("crash-1.log"),
            "=== filmcraft FilmCraft 0.5.9 (unix 900)\nold panic\n\n=== filmcraft FilmCraft 0.5.9 (unix 1500)\nboom in the renderer (at x.rs:1)\nframe 1\n",
        )
        .unwrap();
        let panics = super::panics_since(&dir, 1000, 2);
        assert_eq!(panics.len(), 1, "{panics:?}");
        assert!(panics[0].contains("boom in the renderer") && !panics[0].contains("old panic"));
        // the hook's directory is process-global: point it here for the session calls
        *super::LOG_DIR.lock().unwrap() = Some(dir.clone());
        let r = super::begin_session().expect("reported");
        assert!(r.contains("0.5.9") && r.contains("boom in the renderer"), "{r}");
        assert_eq!(super::take_previous().as_deref(), Some(r.as_str()));
        assert!(super::take_previous().is_none(), "shown once");
        assert!(!dir.join("Sessions/99999.lock").exists());
        let mine = dir.join(format!("Sessions/{}.lock", std::process::id()));
        assert!(mine.exists());
        super::end_session();
        assert!(!mine.exists(), "a normal quit removes the marker");
        assert!(super::begin_session().is_none(), "nothing to report after a normal quit");
        super::end_session();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hook_records_and_logs_worker_panics() {
        let _turn = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("fc-crash-{}", std::process::id()));
        super::install(Some(dir.clone()));
        let _ = super::take_last();
        let r = std::thread::Builder::new().name("filmcraft-test-worker".into()).spawn(|| panic!("boom in a worker")).unwrap().join();
        assert!(r.is_err());
        let last = super::take_last().expect("recorded");
        assert!(last.contains("boom in a worker") && last.contains("filmcraft-test-worker"), "{last}");
        let log = std::fs::read_to_string(super::log_path().unwrap()).unwrap();
        assert!(log.contains("boom in a worker"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
