//! `cargo xtask licenses [--out FILE]`: the third-party crates the desktop app is built from (the
//! `filmcraft` and `filmcraft-cli` binaries for macOS on Apple silicon), each with its version,
//! licence expression, source and the licence texts it ships, plus what the app downloads on
//! request (speech models, voices, caption fonts) — the `THIRD-PARTY-LICENSES.md` the package
//! puts in the app's Resources/Licenses. Uses `cargo metadata` (from the lock file; nothing is
//! downloaded when the registry already holds the crates).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const TARGET: &str = "aarch64-apple-darwin";
const ROOTS: [&str; 2] = ["filmcraft", "filmcraft-cli"];

/// What the app fetches on request, with its licence (not built into the binaries).
const DOWNLOADS: [(&str, &str, &str); 4] = [
    ("Whisper speech models (whisper.cpp ggml)", "MIT", "https://huggingface.co/ggerganov/whisper.cpp"),
    ("Kokoro-82M natural voices", "Apache-2.0", "https://huggingface.co/hexgrad/Kokoro-82M"),
    ("CMU Pronouncing Dictionary (with the voices)", "BSD-2-Clause", "https://github.com/cmusphinx/cmudict"),
    ("Caption fonts from Google Fonts (Montserrat, Oswald, Fira Sans Condensed and five more)", "OFL-1.1", "https://github.com/google/fonts"),
];

pub fn run(args: &[String], root: &Path) -> Result<(), String> {
    let out = args
        .windows(2)
        .find(|w| w[0] == "--out")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| root.join("target/THIRD-PARTY-LICENSES.md").to_string_lossy().to_string());
    let meta = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--filter-platform", TARGET, "--locked"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !meta.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&meta.stderr)));
    }
    let v: Value = serde_json::from_slice(&meta.stdout).map_err(|e| e.to_string())?;
    let packages: HashMap<&str, &Value> = v["packages"].as_array().into_iter().flatten().filter_map(|p| Some((p["id"].as_str()?, p))).collect();
    let nodes: HashMap<&str, &Value> = v["resolve"]["nodes"].as_array().into_iter().flatten().filter_map(|n| Some((n["id"].as_str()?, n))).collect();
    // normal (not build- or dev-only) dependencies reachable from the binaries
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut todo: Vec<&str> = packages.values().filter(|p| ROOTS.contains(&p["name"].as_str().unwrap_or(""))).filter_map(|p| p["id"].as_str()).collect();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        for d in nodes.get(id).and_then(|n| n["deps"].as_array()).into_iter().flatten() {
            let normal = d["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].is_null());
            if normal && let Some(pid) = d["pkg"].as_str() {
                todo.push(pid);
            }
        }
    }
    // third party: what is not a member of this workspace
    let members: BTreeSet<&str> = v["workspace_members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut by_name: BTreeMap<(String, String), &Value> = BTreeMap::new();
    for id in seen.iter().filter(|id| !members.contains(*id)) {
        if let Some(p) = packages.get(id) {
            by_name.insert((p["name"].as_str().unwrap_or("").to_lowercase(), p["version"].as_str().unwrap_or("").to_string()), p);
        }
    }
    let mut md = String::from(
        "# Third-party software in Jilo Studio\n\nJilo Studio is MIT OR Apache-2.0 (see LICENSE-MIT, LICENSE-APACHE and NOTICE). It is built with the Rust crates below, each under its own licence; their licence texts follow the list.\n\n",
    );
    md.push_str("| Crate | Version | Licence | Source |\n|---|---|---|---|\n");
    for p in by_name.values() {
        let src = p["repository"].as_str().or(p["homepage"].as_str()).unwrap_or("crates.io");
        md.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            p["name"].as_str().unwrap_or(""),
            p["version"].as_str().unwrap_or(""),
            p["license"].as_str().unwrap_or("see its licence file"),
            src
        ));
    }
    md.push_str("\n## Downloaded on request\n\nNot part of the app: fetched only when you ask (Settings ▸ AI, Caption Style), checked by SHA-256.\n\n| What | Licence | Source |\n|---|---|---|\n");
    for (what, lic, src) in DOWNLOADS {
        md.push_str(&format!("| {what} | {lic} | {src} |\n"));
    }
    md.push_str("\nThe sound library (Library ▸ Audio) is synthesised by Jilo Studio's own code: CC0, no attribution needed.\n\n## Licence texts\n");
    let mut texts = 0;
    for p in by_name.values() {
        let Some(dir) = p["manifest_path"].as_str().and_then(|m| Path::new(m).parent()) else { continue };
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        let n = e.file_name().to_string_lossy().to_uppercase();
                        e.path().is_file()
                            && (n.starts_with("LICENSE")
                                || n.starts_with("LICENCE")
                                || n.starts_with("COPYING")
                                || n.starts_with("NOTICE")
                                || n.starts_with("UNLICENSE"))
                    })
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        if files.is_empty() {
            continue;
        }
        md.push_str(&format!("\n### {} {}\n", p["name"].as_str().unwrap_or(""), p["version"].as_str().unwrap_or("")));
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            md.push_str(&format!("\n`{}`\n\n```\n{}\n```\n", f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), text.trim_end()));
            texts += 1;
        }
    }
    if let Some(d) = Path::new(&out).parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(&out, md).map_err(|e| format!("{out}: {e}"))?;
    println!("licenses: {} crates, {texts} licence texts → {out}", by_name.len());
    Ok(())
}
