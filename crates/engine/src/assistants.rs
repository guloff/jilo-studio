//! Settings ▸ AI ▸ AI Assistants: letting Claude (or another MCP client) drive the running app.
//!
//! With `ai.allowAssistants` on, the desktop app keeps its control channel open on
//! [`ASSISTANT_PORT`] from the moment the setting is turned on (no restart), and the app's own
//! executable started with `--mcp` (no window) is the MCP server that bridges to it. These commands
//! show how to connect a client and register the server with one:
//!
//! | Command | |
//! |---|---|
//! | `ai.assistants.status` | allowed or not, the port, the MCP server's command, the setup of each client |
//! | `ai.assistants.connect` | `{client: "claudeDesktop" \| "claudeCode"}`: register the server with that client |
//!
//! Claude Desktop reads `~/Library/Application Support/Claude/claude_desktop_config.json`: the
//! server is added under `mcpServers` (other servers and settings are kept; the previous file is
//! kept as `.bak`). Claude Code is told with its own `claude mcp add --scope user`, run when the
//! `claude` command is found; otherwise the command to paste in Terminal is returned.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{Result, Session};

/// The port the app listens on for AI assistants when Settings ▸ AI ▸ Allow AI assistants is on
/// (`--mcp` and `filmcraft-cli mcp --app` connect to it). Fixed, so a client registered once finds the app
/// after every restart.
pub const ASSISTANT_PORT: u16 = 47821;

/// The name the server is registered under in a client.
pub const SERVER_NAME: &str = "jilo-studio";

/// The program a client runs as the MCP server: the running executable itself (in the `.app`,
/// `Contents/MacOS/Jilo Studio`), which serves MCP when started with [`server_args`].
pub fn server_command() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// The arguments that make `command` serve MCP to the open app: `--mcp` for the app, `mcp --app`
/// for the command-line tool (`filmcraft-cli`).
pub fn server_args(command: &Path) -> Vec<&'static str> {
    if command.file_stem().is_some_and(|n| n == "filmcraft-cli") { vec!["mcp", "--app"] } else { vec!["--mcp"] }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Claude Desktop's configuration file.
pub fn claude_desktop_config() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return home().map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json"));
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Claude/claude_desktop_config.json"));
    }
    home().map(|h| h.join(".config/Claude/claude_desktop_config.json"))
}

/// The `claude` command (Claude Code): on `PATH`, else where its installers put it. An app started
/// from the Dock or Finder has a short `PATH`, so the usual places are looked at too.
pub fn claude_code_command() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join("claude")).collect::<Vec<_>>()).unwrap_or_default();
    let usual = home().map(|h| vec![h.join(".local/bin/claude"), h.join(".claude/local/claude"), h.join(".npm-global/bin/claude")]).unwrap_or_default();
    on_path.into_iter().chain(usual).chain([PathBuf::from("/opt/homebrew/bin/claude"), PathBuf::from("/usr/local/bin/claude")]).find(|p| p.is_file())
}

/// Quote one shell word (for the command shown to paste in Terminal).
fn sh_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The Terminal command that registers the server with Claude Code.
pub fn claude_code_setup(command: &Path) -> String {
    format!("claude mcp add --scope user {SERVER_NAME} -- {} {}", sh_quote(&command.to_string_lossy()), server_args(command).join(" "))
}

/// The `mcpServers` entry a client's JSON configuration takes.
pub fn server_entry(command: &Path) -> Value {
    json!({"command": command.to_string_lossy(), "args": server_args(command)})
}

/// Add (or replace) the server in a Claude Desktop configuration file, keeping everything else in
/// it. A missing file is created; one that is not a JSON object is left alone and reported. The
/// previous contents go to `<file>.bak`. Written to a temporary file and renamed, so a failure
/// never leaves half a file.
pub fn add_to_json_config(path: &Path, cli: &Path) -> std::result::Result<bool, String> {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut cfg: Value = match old.as_deref().map(str::trim) {
        None | Some("") => json!({}),
        Some(t) => serde_json::from_str(t).map_err(|e| format!("{} is not valid JSON ({e}); it was left as it is", path.display()))?,
    };
    let Some(root) = cfg.as_object_mut() else {
        return Err(format!("{} is not a JSON object; it was left as it is", path.display()));
    };
    let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else {
        return Err(format!("`mcpServers` in {} is not an object; it was left as it is", path.display()));
    };
    let entry = server_entry(cli);
    let changed = servers.get(SERVER_NAME) != Some(&entry);
    servers.insert(SERVER_NAME.into(), entry);
    if !changed {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if let Some(old) = &old {
        let bak = path.with_extension("json.bak");
        std::fs::write(&bak, old).map_err(|e| format!("{}: {e}", bak.display()))?;
    }
    let text = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text + "\n").map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// Whether a JSON configuration file already has the server, pointing at `cli`.
fn json_config_has(path: &Path, cli: Option<&Path>) -> bool {
    let Some(v) = std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { return false };
    let entry = &v["mcpServers"][SERVER_NAME];
    match cli {
        Some(cli) => *entry == server_entry(cli),
        None => entry.is_object(),
    }
}

/// `command` (for tests and scripts) or this executable.
fn command_for(cmd: &str, p: &Value) -> Result<PathBuf> {
    str_p(p, "command").map(PathBuf::from).or_else(server_command).ok_or_else(|| bad(cmd, "the app's own executable could not be found"))
}

fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let cli = str_p(p, "command").map(PathBuf::from).or_else(server_command);
    let desktop = str_p(p, "config").map(PathBuf::from).or_else(claude_desktop_config);
    Ok(json!({
        "allowed": s.prefs.ai.allow_assistants,
        "port": ASSISTANT_PORT,
        "server": SERVER_NAME,
        "command": cli.as_ref().map(|c| c.to_string_lossy().to_string()),
        "args": cli.as_deref().map(server_args),
        "claudeDesktop": {
            "config": desktop.as_ref().map(|d| d.to_string_lossy().to_string()),
            "connected": desktop.as_deref().is_some_and(|d| json_config_has(d, cli.as_deref())),
        },
        "claudeCode": {
            "command": claude_code_command().map(|c| c.to_string_lossy().to_string()),
            "setup": cli.as_deref().map(claude_code_setup),
        },
    }))
}

fn connect(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ai.assistants.connect";
    let client = str_p(p, "client").ok_or_else(|| bad(cmd, "`client`: claudeDesktop or claudeCode"))?;
    let cli = command_for(cmd, p)?;
    let r = match client {
        "claudeDesktop" => {
            let path = str_p(p, "config").map(PathBuf::from).or_else(claude_desktop_config).ok_or_else(|| bad(cmd, "no home folder"))?;
            let changed = add_to_json_config(&path, &cli).map_err(|e| bad(cmd, e))?;
            json!({"client": client, "config": path.to_string_lossy(), "changed": changed, "restart": true})
        }
        "claudeCode" => {
            let setup = claude_code_setup(&cli);
            let claude = str_p(p, "claude").map(PathBuf::from).or_else(claude_code_command);
            let Some(claude) = claude else {
                return Err(bad(cmd, format!("Claude Code (the `claude` command) was not found. Run this in Terminal: {setup}")));
            };
            // an entry from an earlier install (another path) is replaced
            let _ = std::process::Command::new(&claude).args(["mcp", "remove", "--scope", "user", SERVER_NAME]).output();
            let out = std::process::Command::new(&claude)
                .args(["mcp", "add", "--scope", "user", SERVER_NAME, "--"])
                .arg(&cli)
                .args(server_args(&cli))
                .output()
                .map_err(|e| bad(cmd, format!("{}: {e}. Run this in Terminal: {setup}", claude.display())))?;
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                return Err(bad(cmd, format!("`claude mcp add` failed: {}. Run this in Terminal: {setup}", err.trim())));
            }
            json!({"client": client, "claude": claude.to_string_lossy(), "setup": setup, "changed": true, "restart": false})
        }
        other => return Err(bad(cmd, format!("unknown client `{other}`: claudeDesktop or claudeCode"))),
    };
    // connecting a client is asking for it to work: the channel opens with it
    if !s.prefs.ai.allow_assistants {
        s.execute("prefs.set", json!({"key": "ai.allowAssistants", "value": true}))?;
    }
    Ok(r)
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec("ai.assistants.status", "AI Assistants Status", r#"{"command":str?,"config":str?}"#, status as fn(&mut Session, &Value) -> Result<Value>),
        spec(
            "ai.assistants.connect",
            "Connect an AI Assistant",
            r#"{"client":"claudeDesktop|claudeCode","command":str?,"config":str?,"claude":str?}"#,
            connect,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("filmcraft-assistants-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_desktop_config_gets_the_server_and_keeps_the_rest() {
        let d = tmp("desktop");
        let cfg = d.join("Claude/claude_desktop_config.json");
        let cli = PathBuf::from("/Applications/Jilo Studio.app/Contents/MacOS/Jilo Studio");
        // a missing file is created
        assert_eq!(add_to_json_config(&cfg, &cli), Ok(true));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["mcpServers"][SERVER_NAME], json!({"command": cli.to_string_lossy(), "args": ["--mcp"]}));
        // other servers and settings stay; the old file is kept
        std::fs::write(&cfg, r#"{"globalShortcut":"Cmd+Space","mcpServers":{"other":{"command":"x"}}}"#).unwrap();
        assert_eq!(add_to_json_config(&cfg, &cli), Ok(true));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["globalShortcut"], "Cmd+Space");
        assert_eq!(v["mcpServers"]["other"]["command"], "x");
        assert!(json_config_has(&cfg, Some(&cli)));
        assert!(std::fs::read_to_string(cfg.with_extension("json.bak")).unwrap().contains("globalShortcut"));
        // a second time changes nothing
        assert_eq!(add_to_json_config(&cfg, &cli), Ok(false));
        // a file that is not an object is not touched
        std::fs::write(&cfg, "[1, 2]").unwrap();
        assert!(add_to_json_config(&cfg, &cli).is_err());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "[1, 2]");
        std::fs::write(&cfg, "{not json").unwrap();
        assert!(add_to_json_config(&cfg, &cli).unwrap_err().contains("left as it is"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn connecting_a_client_turns_the_channel_on_and_status_says_so() {
        let d = tmp("connect");
        let cfg = d.join("claude_desktop_config.json");
        let cli = d.join("filmcraft-cli");
        let mut s = Session::default();
        assert!(!s.prefs.ai.allow_assistants, "off until the user turns it on");
        let p = json!({"command": cli.to_string_lossy(), "config": cfg.to_string_lossy()});
        let st = s.execute("ai.assistants.status", p.clone()).unwrap();
        assert_eq!((st["allowed"].clone(), st["port"].clone(), st["claudeDesktop"]["connected"].clone()), (json!(false), json!(47821), json!(false)));
        assert!(st["claudeCode"]["setup"].as_str().unwrap().ends_with("filmcraft-cli mcp --app"), "{st}");
        let r =
            s.execute("ai.assistants.connect", json!({"client": "claudeDesktop", "command": cli.to_string_lossy(), "config": cfg.to_string_lossy()})).unwrap();
        assert_eq!((r["changed"].clone(), r["restart"].clone()), (json!(true), json!(true)));
        assert!(s.prefs.ai.allow_assistants);
        let st = s.execute("ai.assistants.status", p).unwrap();
        assert_eq!(st["claudeDesktop"]["connected"], true);
        // Claude Code without its command: the error carries the command to paste
        let e = s
            .execute(
                "ai.assistants.connect",
                json!({"client": "claudeCode", "command": cli.to_string_lossy(), "claude": d.join("no-claude").to_string_lossy()}),
            )
            .unwrap_err();
        assert!(e.to_string().contains("claude mcp add --scope user jilo-studio --"), "{e}");
        assert!(s.execute("ai.assistants.connect", json!({"client": "cursor", "command": "x"})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn paths_with_spaces_are_quoted_for_terminal() {
        let c = claude_code_setup(Path::new("/Applications/Jilo Studio.app/Contents/MacOS/Jilo Studio"));
        assert_eq!(c, "claude mcp add --scope user jilo-studio -- '/Applications/Jilo Studio.app/Contents/MacOS/Jilo Studio' --mcp");
        // the command-line tool serves MCP with its own subcommand
        assert_eq!(
            claude_code_setup(Path::new("/usr/local/bin/filmcraft-cli")),
            "claude mcp add --scope user jilo-studio -- /usr/local/bin/filmcraft-cli mcp --app"
        );
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }
}
