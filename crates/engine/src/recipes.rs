//! Recipes: a series of editing steps saved under a name and run again with one click on whatever
//! is selected — "clean a talking head", "captions for shorts". A step is any command with its
//! parameters, so a recipe can do anything the menus, the panels or an agent can.
//!
//! - **Recording** (`recipes.recordStart` / `recipes.recordStop`): the commands run at the top
//!   level while recording become the steps, without navigation (selection, playhead, undo) and
//!   without the ids of the clips, items and captions they were run on, so the recipe acts on the
//!   selection and the open sequence of the next run.
//! - **Running** (`recipes.run`): one step at a time, driven by [`pump`] with the other background
//!   work; a step that starts a background job (recognition, a cloud feature, an export) is waited
//!   for before the next one. Recognition (`transcript.generate`) is recognised in the background
//!   first, as the batch does, so the interface never waits. A failed step stops the run. When the
//!   run ends, its edits are one undo step, "Recipe: <name>".
//! - **Storage**: `<data dir>/recipes/<name>.json` (`format: "jilo-recipe"`); built-in recipes are
//!   defined here and cannot be deleted (a saved recipe of the same name replaces one).

use std::path::PathBuf;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, str_p};
use crate::{EngineError, Result, Session};

/// One step: a command and its parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub command: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// A saved series of steps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    #[serde(default = "format")]
    pub format: String,
    #[serde(default = "version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub steps: Vec<Step>,
    /// Defined in code (not saved).
    #[serde(skip)]
    pub builtin: bool,
}

fn format() -> String {
    "jilo-recipe".into()
}
fn version() -> u32 {
    1
}

fn step(command: &str, params: Value) -> Step {
    Step { command: command.into(), params, enabled: true }
}

/// The recipes that come with Jilo Studio.
pub fn builtin() -> Vec<Recipe> {
    let r = |name: &str, description: &str, steps: Vec<Step>| Recipe {
        format: format(),
        version: version(),
        name: name.into(),
        description: description.into(),
        steps,
        builtin: true,
    };
    vec![
        r(
            "Clean Talking Head",
            "Transcribe the open sequence, then remove retakes and stumbles, filler words and pauses.",
            vec![
                step("transcript.generate", json!({})),
                step("transcript.removeRetakes", json!({})),
                step("transcript.removeFillers", json!({})),
                step("transcript.removePauses", json!({})),
            ],
        ),
        r(
            "Karaoke Captions",
            "Transcribe, make captions and give them the Karaoke Yellow style.",
            vec![
                step("transcript.generate", json!({})),
                step("transcript.createCaptions", json!({})),
                step("captions.applyStylePreset", json!({"preset": "karaoke-yellow", "track": "all"})),
            ],
        ),
        r(
            "Shorts Pack",
            "Transcribe, clean fillers and pauses, add karaoke captions and zoom on the loud moments.",
            vec![
                step("transcript.generate", json!({})),
                step("transcript.removeFillers", json!({})),
                step("transcript.removePauses", json!({})),
                step("transcript.createCaptions", json!({})),
                step("captions.applyStylePreset", json!({"preset": "punch-karaoke", "track": "all"})),
                step("clip.autoZoom", json!({"method": "loudness"})),
            ],
        ),
    ]
}

/// Where saved recipes live (`<data dir>/recipes`).
pub fn dir() -> Option<PathBuf> {
    crate::autosave::default_data_dir().map(|d| d.join("recipes"))
}

/// A file name for a recipe name (letters and digits kept, the rest `-`).
fn file_name(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    format!("{}.json", if s.is_empty() { "recipe".to_string() } else { s })
}

/// Saved recipes, by name.
fn saved() -> Vec<Recipe> {
    dir().map(|d| saved_in(&d)).unwrap_or_default()
}

fn saved_in(d: &std::path::Path) -> Vec<Recipe> {
    let Ok(rd) = std::fs::read_dir(d) else { return Vec::new() };
    let mut v: Vec<Recipe> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Recipe>(&b).ok())
        .filter(|r| r.format == "jilo-recipe" && !r.name.trim().is_empty())
        .collect();
    v.sort_by_key(|r| r.name.to_lowercase());
    v
}

/// Every recipe: saved ones, and the built-in ones not replaced by a saved recipe of their name.
pub fn all() -> Vec<Recipe> {
    let mut v = saved();
    for b in builtin() {
        if !v.iter().any(|r| r.name.eq_ignore_ascii_case(&b.name)) {
            v.push(b);
        }
    }
    v
}

pub fn find(name: &str) -> Option<Recipe> {
    all().into_iter().find(|r| r.name.eq_ignore_ascii_case(name.trim()))
}

/// Commands a recording leaves out: moving around, selecting, undoing, files, settings and the
/// recipes themselves.
fn recordable(id: &str) -> bool {
    const SKIP: [&str; 18] = [
        "recipes.",
        "playhead.",
        "project.select",
        "captions.select",
        "captions.goTo",
        "captions.next",
        "captions.previous",
        "edit.undo",
        "edit.redo",
        "edit.select",
        "timeline.select",
        "trim.select",
        "edit.deselect",
        "file.",
        "app.",
        "prefs.",
        "history.",
        "transcript.recognize",
    ];
    !SKIP.iter().any(|p| id.starts_with(p))
}

/// The parameters of a recorded step without the ids of what it was run on (the next run uses the
/// selection and the open sequence) and without waits.
fn portable(params: &Value) -> Value {
    let mut p = params.clone();
    if let Some(o) = p.as_object_mut() {
        for k in ["clips", "clip", "items", "item", "caption", "captions", "sequence", "wait"] {
            o.remove(k);
        }
        if o.get("track").is_some_and(Value::is_number) {
            o.remove("track");
        }
    }
    p
}

impl Session {
    /// Note a command run at the top level while a recipe is being recorded.
    pub(crate) fn record_step(&mut self, id: &str, params: &Value) {
        if let Some(rec) = self.recording.as_mut()
            && recordable(id)
        {
            rec.push(step(id, portable(params)));
        }
    }
}

/// A recipe being run.
pub struct RecipeRun {
    job: crate::Job,
    name: String,
    steps: Vec<Step>,
    /// Next step to run.
    i: usize,
    /// A background job of the current step, and the command (with its parameters) that applies
    /// its result when it is done (None: the job is the step).
    waiting: Option<(u64, Option<(String, Value)>)>,
    /// The project before the run (its edits become one undo step).
    before: usize,
    pub finished: bool,
    error: Option<String>,
}

impl RecipeRun {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "steps": self.steps.len(),
            "step": self.i.min(self.steps.len()),
            "finished": self.finished,
            "error": self.error,
            "job": self.job.id,
        })
    }
}

fn label(command: &str) -> String {
    crate::find_command(command).map_or_else(|| command.to_string(), |c| c.label.trim_end_matches('…').to_string())
}

/// Advance the running recipe by one step (or wait for the step's background job).
pub fn pump(s: &mut Session) {
    let Some(mut r) = s.recipe.take() else { return };
    if r.finished {
        s.recipe = Some(r);
        return;
    }
    if r.job.progress.cancel.load(Ordering::Relaxed) {
        if let Some((id, _)) = r.waiting.take()
            && let Some(j) = s.jobs.iter().find(|j| j.id == id)
        {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
        finish(s, &mut r, Err("stopped".into()));
        s.recipe = Some(r);
        return;
    }
    let n = r.steps.len();
    if let Some((id, then)) = r.waiting.take() {
        match s.jobs.iter().find(|j| j.id == id).map(|j| j.to_json()) {
            Some(v) if v["finished"] == true => {
                if let Some(e) = v["result"]["error"].as_str() {
                    let e = format!("{}: {e}", label(&r.steps[r.i].command));
                    finish(s, &mut r, Err(e));
                    s.recipe = Some(r);
                    return;
                }
                if let Some((cmd, p)) = then
                    && let Err(e) = s.execute(&cmd, p)
                {
                    let e = format!("{}: {e}", label(&cmd));
                    finish(s, &mut r, Err(e));
                    s.recipe = Some(r);
                    return;
                }
                r.i += 1;
            }
            Some(v) => {
                let f = v["progress"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
                r.job.progress.done.store((r.i as u64) * 1000 + (f * 1000.0) as u64, Ordering::Relaxed);
                r.waiting = Some((id, then));
                s.recipe = Some(r);
                return;
            }
            None => r.i += 1,
        }
    }
    // skip the steps that are off
    while r.i < n && !r.steps[r.i].enabled {
        r.i += 1;
    }
    r.job.progress.done.store((r.i as u64) * 1000, Ordering::Relaxed);
    if r.i >= n {
        let msg = format!("Recipe {} done", r.name);
        finish(s, &mut r, Ok(msg));
        s.recipe = Some(r);
        return;
    }
    let st = r.steps[r.i].clone();
    *r.job.progress.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("{} of {n} · {}", r.i + 1, label(&st.command));
    let params = if st.params.is_object() { st.params.clone() } else { json!({}) };
    // recognition: in the background first, then applied
    let started = if st.command == "transcript.generate" {
        let mut bg = params.clone();
        bg["wait"] = json!(false);
        s.execute("transcript.recognize", bg).map(|v| v["job"].as_u64().map(|id| (id, Some((st.command.clone(), params.clone())))))
    } else {
        s.execute(&st.command, params).map(|v| v["job"].as_u64().filter(|id| s.jobs.iter().any(|j| j.id == *id)).map(|id| (id, None)))
    };
    match started {
        Ok(Some(w)) => r.waiting = Some(w),
        Ok(None) => r.i += 1,
        Err(e) => {
            let e = format!("{}: {e}", label(&st.command));
            finish(s, &mut r, Err(e));
        }
    }
    s.recipe = Some(r);
}

/// Make the edits since the project `before` (an `Arc::as_ptr` of `Session::project`) one undo
/// step named `label`. Nothing happens when that project is not in the undo history (no edits).
pub(crate) fn collapse_undo(s: &mut Session, before: usize, label: &str) {
    let h = &mut s.history.undo;
    if let Some(k) = h.iter().position(|u| std::sync::Arc::as_ptr(&u.1) as usize == before) {
        h.truncate(k + 1);
        h[k].0 = label.to_string();
    }
}

fn finish(s: &mut Session, r: &mut RecipeRun, res: std::result::Result<String, String>) {
    r.finished = true;
    r.error = res.as_ref().err().cloned();
    // the run's edits become one undo step
    collapse_undo(s, r.before, &format!("Recipe: {}", r.name));
    crate::narration::finish(&r.job.progress, &r.job.result, res, web_time::Instant::now());
}

fn list(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(all().iter().map(|r| json!({"name": r.name, "description": r.description, "steps": r.steps.len(), "builtin": r.builtin})).collect::<Vec<_>>()))
}

fn get(_: &mut Session, p: &Value) -> Result<Value> {
    let name = str_p(p, "name").ok_or_else(|| bad("recipes.get", "`name` is required"))?;
    let r = find(name).ok_or_else(|| bad("recipes.get", format!("no recipe `{name}`")))?;
    let mut v = serde_json::to_value(&r).map_err(|e| EngineError::Other(e.to_string()))?;
    v["builtin"] = json!(r.builtin);
    Ok(v)
}

fn parse_steps(p: &Value, cmd: &str) -> Result<Vec<Step>> {
    let steps: Vec<Step> = serde_json::from_value(p.get("steps").cloned().unwrap_or(Value::Null)).map_err(|e| bad(cmd, format!("`steps`: {e}")))?;
    if steps.is_empty() {
        return Err(bad(cmd, "a recipe needs at least one step"));
    }
    if let Some(s) = steps.iter().find(|s| crate::find_command(&s.command).is_none()) {
        return Err(bad(cmd, format!("unknown command `{}`", s.command)));
    }
    Ok(steps)
}

fn write(r: &Recipe, cmd: &str) -> Result<Value> {
    let d = dir().ok_or_else(|| bad(cmd, "no data directory for recipes"))?;
    write_in(&d, r)
}

fn write_in(d: &std::path::Path, r: &Recipe) -> Result<Value> {
    std::fs::create_dir_all(d).map_err(|e| EngineError::Other(e.to_string()))?;
    let path = d.join(file_name(&r.name));
    let body = serde_json::to_vec_pretty(r).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::write(&path, body).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(json!({"name": r.name, "steps": r.steps.len(), "path": path.display().to_string()}))
}

fn save(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "recipes.save";
    let name = str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` is required"))?;
    let steps = parse_steps(p, cmd)?;
    let description = str_p(p, "description").unwrap_or("").to_string();
    write(&Recipe { format: format(), version: version(), name: name.into(), description, steps, builtin: false }, cmd)
}

fn delete(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "recipes.delete";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "`name` is required"))?;
    let r = saved()
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(name.trim()))
        .ok_or_else(|| bad(cmd, format!("no saved recipe `{name}` (built-in recipes stay)")))?;
    let path = dir().map(|d| d.join(file_name(&r.name))).ok_or_else(|| bad(cmd, "no data directory for recipes"))?;
    std::fs::remove_file(&path).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(json!({"deleted": r.name}))
}

fn record_start(s: &mut Session, _: &Value) -> Result<Value> {
    if s.recording.is_some() {
        return Err(bad("recipes.recordStart", "a recipe is already being recorded"));
    }
    s.recording = Some(Vec::new());
    Ok(json!({"recording": true}))
}

fn record_stop(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "recipes.recordStop";
    let steps = s.recording.take().ok_or_else(|| bad(cmd, "no recipe is being recorded"))?;
    match str_p(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) if !steps.is_empty() => {
            let r = Recipe {
                format: format(),
                version: version(),
                name: name.into(),
                description: str_p(p, "description").unwrap_or("").into(),
                steps,
                builtin: false,
            };
            write(&r, cmd)
        }
        Some(_) => Err(bad(cmd, "nothing was recorded")),
        None => Ok(json!({"steps": steps})),
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "recipes.run";
    if s.recipe.as_ref().is_some_and(|r| !r.finished) {
        return Err(bad(cmd, "a recipe is already running (recipes.status; recipes.cancel stops it)"));
    }
    let (name, steps) = match (str_p(p, "name"), p.get("steps")) {
        (_, Some(_)) => (str_p(p, "name").unwrap_or("Recipe").to_string(), parse_steps(p, cmd)?),
        (Some(n), None) => {
            let r = find(n).ok_or_else(|| bad(cmd, format!("no recipe `{n}`")))?;
            (r.name, r.steps)
        }
        (None, None) => return Err(bad(cmd, "`name` of a recipe, or `steps`")),
    };
    let job = crate::narration::new_job(s, format!("Recipe: {name}"));
    job.progress.total.store(steps.len() as u64 * 1000, Ordering::Relaxed);
    s.jobs.push(job.clone());
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    s.recipe = Some(RecipeRun { job, name, steps, i: 0, waiting: None, before, finished: false, error: None });
    if bool_p(p, "wait").unwrap_or(false) {
        while s.recipe.as_ref().is_some_and(|r| !r.finished) {
            pump(s);
            if s.recipe.as_ref().is_some_and(|r| r.waiting.is_some()) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    status(s, &json!({}))
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    let mut v = s.recipe.as_ref().map(RecipeRun::to_json).unwrap_or(json!({"running": false}));
    v["recording"] = json!(s.recording.as_ref().map(Vec::len));
    Ok(v)
}

fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    let r = s.recipe.as_ref().filter(|r| !r.finished).ok_or_else(|| bad("recipes.cancel", "no recipe is running"))?;
    r.job.progress.cancel.store(true, Ordering::Relaxed);
    Ok(json!({"cancelled": true}))
}

fn spec(id: &'static str, label: &'static str, params: &'static str, run: fn(&mut Session, &Value) -> Result<Value>) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec("recipes.list", "List Recipes", "{}", list),
        spec("recipes.get", "Get Recipe", r#"{"name":str}"#, get),
        spec("recipes.save", "Save Recipe", r#"{"name":str,"description":str?,"steps":[{"command":id,"params":{}?,"enabled":bool?}]}"#, save),
        spec("recipes.delete", "Delete Recipe", r#"{"name":str}"#, delete),
        spec("recipes.recordStart", "Record Recipe", "{}", record_start),
        spec(
            "recipes.recordStop",
            "Stop Recording Recipe",
            r#"{"name":str? (save under this name; without it the steps are returned),"description":str?}"#,
            record_stop,
        ),
        spec("recipes.run", "Run Recipe", r#"{"name":str? | "steps":[…]?,"wait":bool?}"#, run),
        spec("recipes.status", "Recipe Status", "{}", status),
        spec("recipes.cancel", "Stop Recipe", "{}", cancel),
    ]
}

#[cfg(test)]
#[path = "recipes_tests.rs"]
mod tests;
