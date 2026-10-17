//! The desktop control channel's access token (#163).
//!
//! `filmcraft --control <port>` listens on loopback, which keeps other machines out but not other
//! programs on this one (a web page, another user's process). The app therefore requires a token
//! on each connection's first line: `{"method": "auth", "params": {"token": "…"}}`.
//!
//! The token comes from `FILMCRAFT_CONTROL_TOKEN` when that is set (an agent harness that starts
//! both the app and the client), otherwise the app makes a random one and writes it to
//! `<data dir>/control-<port>.token`, readable by the user only. Clients on the same account
//! (`filmcraft-cli --bridge`, `filmcraft-cli mcp --bridge`) read it from there.

use std::path::{Path, PathBuf};

/// The port the app listens on for AI assistants when Settings ▸ AI ▸ Allow AI assistants is on
/// (`filmcraft-cli mcp --app` connects to it). Fixed, so a client registered once finds the app
/// after every restart.
pub const ASSISTANT_PORT: u16 = 47821;

/// Environment variable that sets the token for both the app and its clients.
pub const TOKEN_ENV: &str = "FILMCRAFT_CONTROL_TOKEN";
/// Environment variable that turns authentication off (`1`), for old scripts on a trusted machine.
pub const NO_AUTH_ENV: &str = "FILMCRAFT_CONTROL_NO_AUTH";

/// Where the app writes the token for a port.
pub fn token_path(data_dir: &Path, port: u16) -> PathBuf {
    data_dir.join(format!("control-{port}.token"))
}

/// The token from [`TOKEN_ENV`], if set and not empty.
pub fn token_from_env() -> Option<String> {
    std::env::var(TOKEN_ENV).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Whether [`NO_AUTH_ENV`] asks for no authentication.
pub fn auth_disabled() -> bool {
    std::env::var(NO_AUTH_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// A new random token: 32 bytes from the OS generator, as 64 hex digits.
pub fn generate() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers for the control token: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Write `token` to `path`, readable and writable by the user only (on Unix; on Windows the
/// per-user data directory's permissions apply). Replaces an older token file.
pub fn write(path: &Path, token: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Remove first: a file left by an older run may have wider permissions, and `mode` only
    // applies when the file is created.
    let _ = std::fs::remove_file(path);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(token.as_bytes()).and_then(|()| f.flush()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The token stored at `path`.
pub fn read(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Compare a presented token with the expected one in time independent of where they differ.
pub fn matches(expected: &str, presented: &str) -> bool {
    let (a, b) = (expected.as_bytes(), presented.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The token a client should present to the app on `port`: [`TOKEN_ENV`], else the app's token
/// file in `data_dir` (or the default data directory).
pub fn for_client(port: u16, data_dir: Option<&Path>) -> Option<String> {
    if let Some(t) = token_from_env() {
        return Some(t);
    }
    let dir = data_dir.map(Path::to_path_buf).or_else(crate::autosave::default_data_dir)?;
    read(&token_path(&dir, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex_and_compare_exactly() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(matches(&a, &a.clone()));
        assert!(!matches(&a, &b));
        assert!(!matches(&a, &a[..63]));
        assert!(!matches(&a, ""));
    }

    #[test]
    fn token_file_is_private_and_replaced() {
        let dir = std::env::temp_dir().join(format!("fc-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = token_path(&dir, 9876);
        write(&path, "first").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // a stale world-readable file is replaced by a private one
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write(&path, "second").unwrap();
        assert_eq!(read(&path).as_deref(), Some("second"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        if token_from_env().is_none() {
            assert_eq!(for_client(9876, Some(&dir)).as_deref(), Some("second"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
