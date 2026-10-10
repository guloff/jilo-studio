//! API keys: `<data dir>/ai.json`, else the environment.
//!
//! The file is the user's own (mode 0600 on Unix), written atomically (a temporary file in the same
//! directory, then a rename) so a crash never leaves half a key. Keys are not kept in the system
//! keychain: a development build's signature changes on every rebuild, and macOS would ask for the
//! login password each time. They never go into projects, logs or command journals; [`Secret`]
//! prints as `***`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Provider;

/// File name of the key file in the data directory.
pub const FILE_NAME: &str = "ai.json";

/// A key. `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The key itself, for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// Where a key came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// `ai.json`.
    File,
    /// This environment variable.
    Env(&'static str),
}

impl KeySource {
    /// `file` or `env:NAME` (for `ai.status`).
    pub fn describe(&self) -> String {
        match self {
            KeySource::File => "file".into(),
            KeySource::Env(v) => format!("env:{v}"),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gemini_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    openai_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anthropic_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elevenlabs_api_key: Option<String>,
    /// Entries this version does not know are kept as they are.
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

impl KeyFile {
    fn slot(&mut self, p: Provider) -> &mut Option<String> {
        match p {
            Provider::Gemini => &mut self.gemini_api_key,
            Provider::OpenAi => &mut self.openai_api_key,
            Provider::Anthropic => &mut self.anthropic_api_key,
            Provider::ElevenLabs => &mut self.elevenlabs_api_key,
        }
    }
}

/// The key file of data directory `data_dir`.
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

fn read(path: &Path) -> std::io::Result<KeyFile> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(KeyFile::default()),
        Err(e) => Err(e),
    }
}

/// The key for `provider`: from the file at `path` (when given and set), else from the provider's
/// environment variables. An unreadable file counts as no file.
pub fn key(path: Option<&Path>, provider: Provider) -> Option<(Secret, KeySource)> {
    key_with_env(path, provider, |v| std::env::var(v).ok())
}

/// [`key`] with the environment supplied (tests).
pub fn key_with_env(path: Option<&Path>, provider: Provider, env: impl Fn(&str) -> Option<String>) -> Option<(Secret, KeySource)> {
    if let Some(path) = path
        && let Ok(mut f) = read(path)
        && let Some(k) = f.slot(provider).take().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
    {
        return Some((Secret(k), KeySource::File));
    }
    provider.env_vars().iter().find_map(|v| env(v).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).map(|k| (Secret(k), KeySource::Env(v))))
}

/// Store (`Some`) or remove (`None` or blank) the key for `provider` in the file at `path`. Other
/// entries stay. The file is owner-only and replaced atomically.
pub fn set_key(path: &Path, provider: Provider, key: Option<&str>) -> std::io::Result<()> {
    let mut f = read(path)?;
    *f.slot(provider) = key.map(str::trim).filter(|k| !k.is_empty()).map(str::to_string);
    let mut text = serde_json::to_vec_pretty(&f).map_err(std::io::Error::other)?;
    text.push(b'\n');
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    let r = write_private(&tmp, &text).and_then(|()| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// Write `data` to a new owner-only file (the mode is set when it is created, before any byte of
/// the key is written).
fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut file = o.open(path)?;
    #[cfg(unix)]
    {
        // an existing temporary file keeps its old mode through `open`
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(data)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("filmcraft-ai-keys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn file_first_then_environment() {
        let d = tmp("order");
        let p = path_in(&d);
        let env = |v: &str| (v == "GOOGLE_API_KEY").then(|| "env-key".to_string());
        assert_eq!(key_with_env(Some(&p), Provider::Gemini, env), Some((Secret::new("env-key"), KeySource::Env("GOOGLE_API_KEY"))));
        assert_eq!(key_with_env(Some(&p), Provider::OpenAi, env), None);
        set_key(&p, Provider::Gemini, Some("  file-key \n")).unwrap();
        assert_eq!(key_with_env(Some(&p), Provider::Gemini, env), Some((Secret::new("file-key"), KeySource::File)));
        // GEMINI_API_KEY comes before GOOGLE_API_KEY
        let both = |v: &str| Some(format!("{v}-value"));
        assert_eq!(key_with_env(None, Provider::Gemini, both).map(|k| k.1), Some(KeySource::Env("GEMINI_API_KEY")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn set_and_remove_keep_other_entries_and_the_file_is_private() {
        let d = tmp("set");
        let p = path_in(&d);
        std::fs::write(&p, r#"{"futureSetting": 7, "openaiApiKey": "sk-old"}"#).unwrap();
        set_key(&p, Provider::Gemini, Some("g-key")).unwrap();
        set_key(&p, Provider::OpenAi, Some("")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"futureSetting": 7, "geminiApiKey": "g-key"}));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        set_key(&p, Provider::Gemini, None).unwrap();
        assert_eq!(key_with_env(Some(&p), Provider::Gemini, |_| None), None);
        // no temporary files left behind
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_file_is_not_overwritten() {
        let d = tmp("bad");
        let p = path_in(&d);
        std::fs::write(&p, "{not json").unwrap();
        assert!(set_key(&p, Provider::Gemini, Some("k")).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{not json");
        assert_eq!(key_with_env(Some(&p), Provider::Gemini, |_| None), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn secrets_do_not_print() {
        let s = Secret::new("sk-very-secret");
        assert_eq!(format!("{s:?} {s}"), "Secret(***) ***");
    }
}
