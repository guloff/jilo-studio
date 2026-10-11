//! Help ▸ Check for Updates… (`app.checkUpdates`, `app.updateStatus`): the latest release of Jilo
//! Studio on GitHub (`releases/latest`, which leaves drafts and pre-releases out) compared with this
//! build's version, on request or once a day, and nothing but the request itself is sent. The
//! answer is untrusted text: the notes are shown as plain text, cut to a length, only a release
//! page of this repository is ever opened, and only this repository's disk image for this Mac,
//! with the SHA-256 GitHub gives for it, is offered for download ([`crate::updater`]).

use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bool_p};
use crate::{EngineError, Result, Session};

/// The repository releases are published in.
pub const REPO: &str = "guloff/jilo-studio";
/// GitHub's API for the latest published release.
pub const RELEASES_API: &str = "https://api.github.com/repos/guloff/jilo-studio/releases/latest";
/// The releases page, opened when a release names no page of its own.
pub const RELEASES_PAGE: &str = "https://github.com/guloff/jilo-studio/releases";
/// Release notes longer than this are cut (characters).
const NOTES_MAX: usize = 4000;

/// What a check found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// This build's version.
    pub current: String,
    /// The latest release's version (its tag without the leading `v`), when there is one.
    pub latest: Option<String>,
    /// The latest release is newer than this build.
    pub newer: bool,
    /// The release's page (always on this repository's releases).
    pub url: String,
    /// The release's notes, plain text.
    pub notes: String,
    /// When it was published (ISO 8601, as GitHub gives it).
    pub published: Option<String>,
    /// No release is published yet.
    pub none: bool,
    /// Why the check failed.
    pub error: Option<String>,
    /// The release's disk image for this Mac (None: the release has none; its page is offered).
    pub asset: Option<Asset>,
}

/// A release's disk image: its name, download address, size and SHA-256 (as GitHub publishes it).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// The disk image of release `version` for an Apple silicon Mac, from GitHub's `assets`: only
/// `jilo-studio-<version>-macos-aarch64.dmg`, downloaded from this repository, with a size and a
/// SHA-256 digest.
pub fn asset_of(release: &Value, version: &str) -> Option<Asset> {
    let name = format!("jilo-studio-{version}-macos-aarch64.dmg");
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    release["assets"].as_array()?.iter().find_map(|a| {
        if a["name"].as_str()? != name {
            return None;
        }
        let url = a["browser_download_url"].as_str()?;
        if !url.starts_with(&prefix) || !url.ends_with(&format!("/{name}")) || url.contains(['\n', ' ', '"', '?', '#', '\\']) || url.contains("..") {
            return None;
        }
        let size = a["size"].as_u64().filter(|s| *s > 0 && *s < 2 << 30)?;
        let sha = a["digest"].as_str()?.strip_prefix("sha256:")?;
        if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        Some(Asset { name: name.clone(), url: url.to_string(), size, sha256: sha.to_ascii_lowercase() })
    })
}

/// The last check of this session, written by the background job.
pub type UpdateState = Arc<Mutex<Option<UpdateCheck>>>;

/// A version's numbers: `v0.6.1`, `0.6`, `0.6.1-beta.2` (the suffix ignored) — None without a
/// leading number.
pub fn version(tag: &str) -> Option<[u64; 3]> {
    let t = tag.trim().trim_start_matches(['v', 'V']);
    let core = t.split(['-', '+', ' ']).next()?;
    let mut out = [0u64; 3];
    let mut any = false;
    for (i, part) in core.split('.').take(3).enumerate() {
        out[i] = part.parse().ok()?;
        any = true;
    }
    any.then_some(out)
}

/// Release notes (GitHub Markdown) as plain text for the window: headings without their `#`,
/// list items as bullets, links as their text, no emphasis or code marks, and menu paths with
/// `›` (the window's font has no `▸`).
pub fn plain(md: &str) -> String {
    let link = |l: &str| {
        // [text](url) → text
        let mut out = String::with_capacity(l.len());
        let mut rest = l;
        while let Some(a) = rest.find('[') {
            let Some(b) = rest[a..].find("](").map(|b| a + b) else { break };
            let Some(c) = rest[b..].find(')').map(|c| b + c) else { break };
            out.push_str(&rest[..a]);
            out.push_str(&rest[a + 1..b]);
            rest = &rest[c + 1..];
        }
        out.push_str(rest);
        out
    };
    md.lines()
        .map(|l| {
            let t = l.trim_end();
            let lead = t.len() - t.trim_start().len();
            let body = t.trim_start();
            let body = if body.starts_with('#') { body.trim_start_matches('#').trim_start().to_string() } else { body.to_string() };
            let body = match body.strip_prefix("- ").or_else(|| body.strip_prefix("* ")) {
                Some(item) => format!("{}• {item}", " ".repeat(lead)),
                None => format!("{}{body}", " ".repeat(lead)),
            };
            link(&body).replace("**", "").replace('`', "").replace('▸', "›")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Read GitHub's answer (`status`, `body`) for a build of version `current`.
pub fn read_release(current: &str, status: u16, body: &[u8]) -> UpdateCheck {
    let mut c = UpdateCheck { current: current.to_string(), url: RELEASES_PAGE.to_string(), ..Default::default() };
    match status {
        200..=299 => {}
        404 => {
            c.none = true;
            return c;
        }
        403 | 429 => {
            c.error = Some("GitHub is limiting requests from this network; try again in an hour".into());
            return c;
        }
        s => {
            c.error = Some(format!("GitHub answered {s}"));
            return c;
        }
    }
    let Some(v) = serde_json::from_slice::<Value>(body).ok().filter(Value::is_object) else {
        c.error = Some("GitHub's answer could not be read".into());
        return c;
    };
    let tag = v["tag_name"].as_str().unwrap_or_default();
    let Some(latest) = version(tag) else {
        c.error = Some(format!("the latest release has no version number (`{}`)", tag.chars().take(40).collect::<String>()));
        return c;
    };
    let name: String = tag.trim().trim_start_matches(['v', 'V']).chars().take(40).collect();
    c.newer = version(current).is_some_and(|cur| latest > cur);
    c.asset = asset_of(&v, &name);
    c.latest = Some(name);
    // only a page of this repository's releases is opened
    if let Some(u) = v["html_url"].as_str().filter(|u| u.starts_with(&format!("{RELEASES_PAGE}/")) && !u.contains(['\n', ' ', '"'])) {
        c.url = u.to_string();
    }
    let notes = plain(&v["body"].as_str().unwrap_or_default().replace("\r\n", "\n"));
    c.notes = if notes.chars().count() > NOTES_MAX {
        format!("{}…", notes.chars().take(NOTES_MAX).collect::<String>().trim_end())
    } else {
        notes.trim().to_string()
    };
    c.published = v["published_at"].as_str().map(|s| s.chars().take(40).collect());
    c
}

/// Whether the daily check is due: on, and a day since the last one (`now`, unix seconds).
pub fn daily_check_due(prefs: &crate::settings::GeneralPrefs, now: i64) -> bool {
    prefs.check_for_updates && (now - prefs.last_update_check >= 86_400 || prefs.last_update_check > now)
}

/// Ask GitHub (in the calling thread).
fn check(transport: &dyn filmcraft_ai::transport::Transport, current: &str) -> UpdateCheck {
    let mut req = filmcraft_ai::transport::Request::get(RELEASES_API);
    req.headers.push(("Accept".into(), "application/vnd.github+json".into()));
    // GitHub refuses requests without one
    req.headers.push(("User-Agent".into(), format!("JiloStudio/{current}")));
    match transport.send(&req) {
        Ok(r) => read_release(current, r.status, &r.body),
        Err(e) => UpdateCheck { current: current.to_string(), url: RELEASES_PAGE.to_string(), error: Some(e), ..Default::default() },
    }
}

fn check_updates(s: &mut Session, p: &Value) -> Result<Value> {
    let transport = crate::ai::transport(s).map_err(|e| EngineError::Other(e.to_string()))?;
    let current = env!("CARGO_PKG_VERSION");
    if bool_p(p, "wait").unwrap_or(false) {
        let c = check(&*transport, current);
        *s.update_check.lock().unwrap_or_else(PoisonError::into_inner) = Some(c.clone());
        return Ok(serde_json::to_value(c).unwrap_or_default());
    }
    let job = crate::narration::new_job(s, "Checking for updates".into());
    let (prog, res, state) = (job.progress.clone(), job.result.clone(), s.update_check.clone());
    let id = crate::narration::spawn(s, job, false, "filmcraft-updates", move || {
        let t0 = web_time::Instant::now();
        let c = check(&*transport, current);
        let status = match (&c.error, c.newer) {
            (Some(e), _) => Err(e.clone()),
            (None, true) => Ok(format!("Jilo Studio {} is available", c.latest.clone().unwrap_or_default())),
            (None, false) => Ok("Jilo Studio is up to date".to_string()),
        };
        *state.lock().unwrap_or_else(PoisonError::into_inner) = Some(c);
        crate::narration::finish(&prog, &res, status, t0);
    })?;
    Ok(json!({"job": id}))
}

fn update_status(s: &mut Session, _: &Value) -> Result<Value> {
    let mut v = match s.update_check.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        Some(c) => {
            let mut v = serde_json::to_value(c).unwrap_or_default();
            v["checked"] = json!(true);
            v
        }
        None => json!({"checked": false, "current": env!("CARGO_PKG_VERSION")}),
    };
    v["download"] = crate::updater::status_json(s);
    Ok(v)
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "app.checkUpdates",
            label: "Check for Updates…",
            menu: &["Help"],
            shortcut: None,
            params: r#"{"wait":bool?}"#,
            enabled: crate::commands::always,
            run: check_updates,
            journal: false,
        },
        CommandSpec {
            id: "app.updateStatus",
            label: "Update Status",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: crate::commands::always,
            run: update_status,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_ai::Response;
    use filmcraft_ai::transport::FakeTransport;

    #[test]
    fn versions_compare_by_their_numbers() {
        assert_eq!(version("v0.6.1"), Some([0, 6, 1]));
        assert_eq!(version("0.6"), Some([0, 6, 0]));
        assert_eq!(version("V1.2.3-beta.2"), Some([1, 2, 3]));
        assert_eq!(version("1.10.0"), Some([1, 10, 0]));
        assert!(version("1.10.0") > version("1.9.9"));
        for bad in ["", "v", "latest", "x1.2", "1.a.3", "99999999999999999999999"] {
            assert_eq!(version(bad), None, "{bad}");
        }
    }

    fn release(tag: &str, url: &str) -> Vec<u8> {
        json!({"tag_name": tag, "html_url": url, "body": "Fixes.\r\n- Faster export", "published_at": "2026-10-20T10:00:00Z"}).to_string().into_bytes()
    }

    #[test]
    fn a_release_is_read_and_compared_with_this_build() {
        let page = format!("{RELEASES_PAGE}/tag/v0.6.0");
        let c = read_release("0.5.0", 200, &release("v0.6.0", &page));
        assert_eq!((c.latest.as_deref(), c.newer, c.url.as_str(), c.notes.as_str()), (Some("0.6.0"), true, page.as_str(), "Fixes.\n• Faster export"));
        assert_eq!(c.published.as_deref(), Some("2026-10-20T10:00:00Z"));
        assert!(!read_release("0.6.0", 200, &release("v0.6.0", &page)).newer, "the same version");
        assert!(!read_release("0.7.0", 200, &release("v0.6.0", &page)).newer, "an older release");
        // another site's page is never opened: the releases page instead
        assert_eq!(read_release("0.5.0", 200, &release("v0.6.0", "https://evil.example/x")).url, RELEASES_PAGE);
        assert_eq!(read_release("0.5.0", 200, &release("v0.6.0", &format!("{RELEASES_PAGE}/x\"y"))).url, RELEASES_PAGE);
        // long notes are cut
        let long = json!({"tag_name": "v1.0.0", "body": "é".repeat(10_000)}).to_string();
        assert!(read_release("0.5.0", 200, long.as_bytes()).notes.chars().count() <= NOTES_MAX + 1);
    }

    #[test]
    fn notes_lose_their_markdown() {
        let md = "## What's new\n\n- **Faster** export, see [the docs](https://x.y/z)\n  * nested `code`\nPlain line";
        assert_eq!(plain(md), "What's new\n\n• Faster export, see the docs\n  • nested code\nPlain line");
        assert_eq!(plain("[unclosed (x"), "[unclosed (x");
        assert_eq!(plain(""), "");
        assert_eq!(plain("Settings ▸ AI"), "Settings › AI");
        let notes = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/releases/0.6.0.md")).unwrap();
        let p = plain(&notes);
        assert!(!p.contains("##") && !p.contains("**") && p.contains("• Reframe to Face"), "{p}");
    }

    #[test]
    fn no_release_a_limit_or_a_bad_answer_is_said_plainly() {
        let none = read_release("0.5.0", 404, b"{}");
        assert!(none.none && none.error.is_none() && !none.newer);
        assert!(read_release("0.5.0", 403, b"").error.unwrap().contains("limiting"));
        assert!(read_release("0.5.0", 500, b"").error.unwrap().contains("500"));
        for body in [&b"not json"[..], b"[]", b"{\"tag_name\": \"nightly\"}", b"{\"tag_name\": 7}"] {
            let c = read_release("0.5.0", 200, body);
            assert!(c.error.is_some() && !c.newer, "{c:?}");
        }
    }

    #[test]
    fn the_check_runs_in_the_background_and_keeps_its_answer() {
        let fake = Arc::new(FakeTransport::new());
        let page = format!("{RELEASES_PAGE}/tag/v99.0.0");
        let body = release("v99.0.0", &page);
        fake.route(
            |r| r.url == RELEASES_API,
            move |r| {
                assert!(r.headers.iter().any(|(k, v)| k == "User-Agent" && v.starts_with("JiloStudio/")), "GitHub needs a User-Agent");
                Ok(Response::new(200, body.clone()))
            },
        );
        let mut s = Session { ai_transport: Some(fake.clone()), ..Default::default() };
        assert_eq!(s.execute("app.updateStatus", json!({})).unwrap()["checked"], false);
        let r = s.execute("app.checkUpdates", json!({})).unwrap();
        let id = r["job"].as_u64().unwrap();
        for _ in 0..500 {
            if s.jobs.iter().find(|j| j.id == id).is_some_and(|j| j.to_json()["finished"] == true) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let st = s.execute("app.updateStatus", json!({})).unwrap();
        assert_eq!((st["checked"].as_bool(), st["newer"].as_bool(), st["latest"].as_str()), (Some(true), Some(true), Some("99.0.0")), "{st}");
        assert_eq!(st["url"], page);
        assert_eq!(fake.sent().len(), 1);
        // a network failure is an answer too
        let mut s = Session { ai_transport: Some(Arc::new(FakeTransport::new())), ..Default::default() };
        let r = s.execute("app.checkUpdates", json!({"wait": true})).unwrap();
        assert!(r["error"].is_string() && r["newer"] == false, "{r}");
    }
}
