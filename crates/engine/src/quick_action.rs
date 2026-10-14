//! Finder Quick Actions: right-click video or audio files in Finder ▸ Quick Actions ▸ "<name>", and
//! Jilo Studio processes them without opening a window (`batch.processFiles` with the steps kept
//! in the action: a recipe, captions, a brand kit, formats, export…). A Quick Action is an
//! Automator service, `<name>.workflow` in `~/Library/Services`, whose one step runs
//! `"<Jilo Studio>" --process '<steps as JSON>' <files>`.
//!
//! | Command | |
//! |---|---|
//! | `system.quickAction.install` | `{name, params, exe?, dir?}`: write the action (replacing one of that name) |
//! | `system.quickAction.list` | `{dir?}`: the Jilo Studio actions installed |
//! | `system.quickAction.remove` | `{name, dir?}` |
//!
//! The same command line works in a Shortcuts "Run Shell Script" action (docs/automation.md).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// Marks a workflow as made by Jilo Studio (in its Info.plist), so `list` and `remove` only touch ours.
const MARK: &str = "JiloStudioQuickAction";

/// Where Quick Actions live: `~/Library/Services`.
pub fn services_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join("Library/Services"))
}

fn dir_p(p: &Value) -> Result<PathBuf> {
    match str_p(p, "dir") {
        Some(d) => Ok(PathBuf::from(crate::export_tools::expand_home(d))),
        None => services_dir().ok_or_else(|| EngineError::Other("no home folder for Quick Actions".into())),
    }
}

/// A name Finder can show and a file system can hold.
fn valid_name(n: &str) -> bool {
    let n = n.trim();
    !n.is_empty() && n.chars().count() <= 60 && !n.contains(['/', ':', '\\']) && !n.starts_with('.')
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `s` as one single-quoted shell word.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The shell script of the action's one step: the files come as arguments.
pub fn script(exe: &str, params: &Value) -> String {
    format!("exec {} --process {} \"$@\"\n", shell_quote(exe), shell_quote(&params.to_string()))
}

const PLIST_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n";

/// `Contents/Info.plist`: a Finder service for movies, sound and pictures.
pub fn info_plist(name: &str) -> String {
    format!(
        "{PLIST_HEAD}<dict>\n\t<key>{MARK}</key>\n\t<true/>\n\t<key>NSServices</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>NSBackgroundColorName</key>\n\t\t\t<string>background</string>\n\t\t\t<key>NSIconName</key>\n\t\t\t<string>NSActionTemplate</string>\n\t\t\t<key>NSMenuItem</key>\n\t\t\t<dict>\n\t\t\t\t<key>default</key>\n\t\t\t\t<string>{}</string>\n\t\t\t</dict>\n\t\t\t<key>NSMessage</key>\n\t\t\t<string>runWorkflowAsService</string>\n\t\t\t<key>NSRequiredContext</key>\n\t\t\t<dict>\n\t\t\t\t<key>NSApplicationIdentifier</key>\n\t\t\t\t<string>com.apple.finder</string>\n\t\t\t</dict>\n\t\t\t<key>NSSendFileTypes</key>\n\t\t\t<array>\n\t\t\t\t<string>public.movie</string>\n\t\t\t\t<string>public.audio</string>\n\t\t\t\t<string>public.image</string>\n\t\t\t</array>\n\t\t</dict>\n\t</array>\n</dict>\n</plist>\n",
        xml(name)
    )
}

/// `Contents/document.wflow`: one "Run Shell Script" step, input as arguments.
pub fn document_wflow(command: &str) -> String {
    let uuid = |k: u32| format!("4A1E5C2B-7D3F-4E8A-9B6C-{:012X}", 0x51_10_00_00_00_00u64 + u64::from(k));
    format!(
        r#"{PLIST_HEAD}<dict>
	<key>AMApplicationBuild</key>
	<string>523</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/zsh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>{}</string>
				<key>Keywords</key>
				<array>
					<string>Shell</string>
					<string>Script</string>
				</array>
				<key>OutputUUID</key>
				<string>{}</string>
				<key>UUID</key>
				<string>{}</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict/>
				<key>isViewVisible</key>
				<integer>1</integer>
				<key>location</key>
				<string>309.000000:253.000000</string>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleIDsByPath</key>
		<dict/>
		<key>applicationPaths</key>
		<array/>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<false/>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<false/>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<false/>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#,
        xml(command),
        uuid(1),
        uuid(2),
        uuid(3)
    )
}

/// Write `<dir>/<name>.workflow`, replacing one of that name.
pub fn install_in(dir: &Path, name: &str, exe: &str, params: &Value) -> std::io::Result<PathBuf> {
    let wf = dir.join(format!("{}.workflow", name.trim()));
    let contents = wf.join("Contents");
    if wf.exists() {
        std::fs::remove_dir_all(&wf)?;
    }
    std::fs::create_dir_all(&contents)?;
    std::fs::write(contents.join("Info.plist"), info_plist(name.trim()))?;
    std::fs::write(contents.join("document.wflow"), document_wflow(&script(exe, params)))?;
    Ok(wf)
}

/// The Jilo Studio actions in `dir`, by name.
pub fn list_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "workflow"))
                .filter(|e| std::fs::read_to_string(e.path().join("Contents/Info.plist")).is_ok_and(|t| t.contains(MARK)))
                .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn install(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "system.quickAction.install";
    let name = str_p(p, "name").filter(|n| valid_name(n)).ok_or_else(|| bad(cmd, "`name`: what Finder shows (no / or :)"))?;
    let params = p.get("params").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({"export": true}));
    let exe = match str_p(p, "exe") {
        Some(e) => e.to_string(),
        None => std::env::current_exe().map_err(|e| EngineError::Other(format!("the app's path: {e}")))?.to_string_lossy().to_string(),
    };
    let dir = dir_p(p)?;
    let wf = install_in(&dir, name, &exe, &params).map_err(|e| EngineError::Other(format!("{}: {e}", dir.display())))?;
    Ok(json!({"name": name.trim(), "path": wf.to_string_lossy(), "script": script(&exe, &params)}))
}

fn list(_: &mut Session, p: &Value) -> Result<Value> {
    Ok(json!(list_in(&dir_p(p)?)))
}

fn remove(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "system.quickAction.remove";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "`name`"))?;
    let dir = dir_p(p)?;
    if !list_in(&dir).iter().any(|n| n == name) {
        return Err(bad(cmd, format!("no Jilo Studio Quick Action `{name}`")));
    }
    std::fs::remove_dir_all(dir.join(format!("{name}.workflow"))).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"removed": name}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, params, run| CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false };
    vec![
        spec(
            "system.quickAction.install",
            "Make Finder Quick Action",
            r#"{"name":str,"params":{batch.processFiles steps}?,"exe":path?,"dir":path? (default ~/Library/Services)}"#,
            install as fn(&mut Session, &Value) -> Result<Value>,
        ),
        spec("system.quickAction.list", "List Finder Quick Actions", r#"{"dir":path?}"#, list),
        spec("system.quickAction.remove", "Remove Finder Quick Action", r#"{"name":str,"dir":path?}"#, remove),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_passes_the_steps_and_files_safely() {
        let p = json!({"recipeName": "Bob's \"cut\"", "recipe": true});
        let s = script("/Applications/Jilo Studio.app/Contents/MacOS/Jilo Studio", &p);
        assert!(s.starts_with("exec '/Applications/Jilo Studio.app/Contents/MacOS/Jilo Studio' --process '"), "{s}");
        assert!(s.contains("Bob'\\''s"), "a quote inside the JSON is closed and reopened: {s}");
        assert!(s.ends_with(" \"$@\"\n"));
        assert!(valid_name("Shorts with captions") && !valid_name("a/b") && !valid_name(" ") && !valid_name(".hidden"));
    }

    #[test]
    fn an_action_is_installed_listed_and_removed() {
        let d = std::env::temp_dir().join(format!("jilo-qa-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // someone else's workflow is left alone
        std::fs::create_dir_all(d.join("Other.workflow/Contents")).unwrap();
        std::fs::write(d.join("Other.workflow/Contents/Info.plist"), "<plist/>").unwrap();
        let mut s = Session::default();
        let dir = d.to_string_lossy().to_string();
        let r = s
            .execute("system.quickAction.install", json!({"name": "Shorts & Captions", "params": {"formats": true}, "exe": "/x/Jilo Studio", "dir": dir}))
            .unwrap();
        assert!(r["path"].as_str().unwrap().ends_with("Shorts & Captions.workflow"));
        let info = std::fs::read_to_string(d.join("Shorts & Captions.workflow/Contents/Info.plist")).unwrap();
        assert!(info.contains("<string>Shorts &amp; Captions</string>") && info.contains("public.movie"));
        let doc = std::fs::read_to_string(d.join("Shorts & Captions.workflow/Contents/document.wflow")).unwrap();
        assert!(doc.contains("--process") && doc.contains("{&quot;formats&quot;:true}"), "{doc}");
        assert_eq!(s.execute("system.quickAction.list", json!({"dir": dir})).unwrap(), json!(["Shorts & Captions"]));
        assert!(s.execute("system.quickAction.remove", json!({"name": "Other", "dir": dir})).is_err(), "not ours");
        s.execute("system.quickAction.remove", json!({"name": "Shorts & Captions", "dir": dir})).unwrap();
        assert_eq!(s.execute("system.quickAction.list", json!({"dir": dir})).unwrap(), json!([]));
        assert!(d.join("Other.workflow").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
