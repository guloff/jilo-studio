//! Batch processing (`batch.start`, `batch.status`, `batch.cancel`), as Jilo Studio runs batches:
//! the same steps over several sequences, one after another, in the background.
//!
//! The items are sequences, or clips of the Project panel (a sequence is made from each, as File ▸
//! New Sequence From Clip). The steps, in this order, each optional:
//! `transcribe` (only the media without a transcript; the engine of the batch, local Whisper by
//! default), `fillers` (remove hesitations), `pauses` (remove silent pauses by sound), `autoZoom`
//! (by loudness), `highlights` and `youtube` (the text engine), `export` (an export queue item per
//! sequence, started at the end). The batch is one job in the status bar and the Progress panel
//! (Cancel stops it after the current step); `batch.status` reports every item. A failed step is
//! noted and the item goes on with its next step; one item's failure never stops the others. The
//! cloud is used only for the steps that need it, after the consent given with `batch.start`
//! (`confirm`). The steps run through the same commands as by hand, so each change is an undo step.
//!
//! Driven by [`pump`] (called with the other background work, `Session::poll_persistence`): one
//! step at a time, so the interface never waits for a long batch.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;

use filmcraft_project::{ItemId, ItemKind};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p};
use crate::{EngineError, Result, Session};

/// At most this many items in a batch.
pub const BATCH_LIMIT: usize = 50;

/// The steps of a batch, in the order they run.
pub const STEPS: [&str; 7] = ["transcribe", "fillers", "pauses", "autoZoom", "highlights", "youtube", "export"];

/// What happened to one item.
#[derive(Clone, Debug, Default)]
pub struct ItemReport {
    pub sequence: ItemId,
    pub name: String,
    /// `queued`, `running`, `done`, `failed`, `cancelled`.
    pub status: String,
    pub done: Vec<String>,
    /// (step, error) of the steps that failed.
    pub errors: Vec<(String, String)>,
}

/// A running batch.
pub struct BatchRun {
    job: crate::Job,
    items: Vec<ItemReport>,
    steps: Vec<&'static str>,
    params: Value,
    /// Item and step being run.
    i: usize,
    k: usize,
    /// A background job of the current step and the command (with its parameters) that applies
    /// its result when it is done.
    child: Option<(u64, &'static str, Value)>,
    queued_exports: usize,
    finished: bool,
}

impl BatchRun {
    pub fn to_json(&self) -> Value {
        json!({
            "job": self.job.id,
            "finished": self.finished,
            "steps": self.steps,
            "items": self.items.iter().map(|r| json!({
                "sequence": r.sequence.0,
                "name": r.name,
                "status": r.status,
                "done": r.done,
                "errors": r.errors.iter().map(|(s, e)| json!({"step": s, "error": e})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// Run `cmd` with sequence `seq` active, then give the editor its sequence back.
fn on_sequence(s: &mut Session, seq: ItemId, cmd: &str, p: Value) -> Result<Value> {
    let (active, open) = (s.state.active_sequence, s.state.open_sequences.clone());
    s.state.active_sequence = Some(seq);
    let r = s.execute(cmd, p);
    s.state.active_sequence = active;
    s.state.open_sequences = open;
    r
}

/// The media on `seq`'s audio tracks that has no transcript.
fn untranscribed(s: &Session, seq: ItemId) -> Vec<u64> {
    let Some(q) = s.project.sequence(seq) else { return Vec::new() };
    let mut out: Vec<u64> = Vec::new();
    for it in q.audio_tracks.iter().flat_map(|t| t.items.iter()) {
        let media = matches!(s.project.item(it.item).map(|i| &i.kind), Some(ItemKind::Media(_)));
        if media && !s.project.transcripts.contains_key(&it.item) && !out.contains(&it.item.0) {
            out.push(it.item.0);
        }
    }
    out
}

/// Parameters of a cloud or speech step: the batch's engine and language, and its consent.
fn with_engine(params: &Value, key: &str) -> Value {
    let mut p = json!({"confirm": bool_p(params, "confirm").unwrap_or(false)});
    if let Some(e) = params.get(key).and_then(Value::as_str) {
        p["engine"] = json!(e);
    }
    if let Some(l) = params.get("language").and_then(Value::as_str).filter(|l| !l.is_empty() && *l != "auto") {
        p["language"] = json!(l);
    }
    p
}

/// Start one step of the current item. Ok(Some(..)): a background job to wait for.
fn start_step(s: &mut Session, b: &BatchRun, seq: ItemId, step: &str) -> Result<Option<(u64, &'static str, Value)>> {
    let p = &b.params;
    match step {
        "transcribe" => {
            let items = untranscribed(s, seq);
            if items.is_empty() {
                return Ok(None);
            }
            let mut q = with_engine(p, "speechEngine");
            q["items"] = json!(items);
            let mut start = q.clone();
            start["wait"] = json!(false);
            let r = on_sequence(s, seq, "transcript.recognize", start)?;
            Ok(r["job"].as_u64().map(|id| (id, "transcript.generate", q)))
        }
        "fillers" => on_sequence(s, seq, "transcript.removeFillers", json!({})).map(|_| None),
        "pauses" => on_sequence(s, seq, "audio.removeSilences", json!({})).map(|_| None),
        "autoZoom" => on_sequence(s, seq, "clip.autoZoom", json!({"method": "loudness"})).map(|_| None),
        "highlights" | "youtube" => {
            let cmd: &'static str = if step == "highlights" { "highlights.find" } else { "youtube.generate" };
            let q = with_engine(p, "textEngine");
            let mut start = q.clone();
            start["background"] = json!(true);
            let r = on_sequence(s, seq, cmd, start)?;
            Ok(r["job"].as_u64().map(|id| (id, cmd, q)))
        }
        "export" => {
            let mut q = json!({"sequence": seq.0});
            for k in ["preset", "path"] {
                if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
                    q[k] = v.clone();
                }
            }
            s.execute("export.queue.add", q).map(|_| None)
        }
        other => Err(EngineError::Other(format!("unknown step `{other}`"))),
    }
}

/// The text of an error for the summary: the reason, without the command's name around it.
fn short(e: &EngineError) -> String {
    let m = e.to_string();
    match m.split_once(" is not available right now: ") {
        Some((_, why)) => why.to_string(),
        None => m,
    }
}

/// Advance the running batch by one step (or wait for the step's background job).
pub fn pump(s: &mut Session) {
    let Some(mut b) = s.batch.take() else { return };
    if b.finished {
        s.batch = Some(b);
        return;
    }
    let cancel = b.job.progress.cancel.load(Ordering::Relaxed);
    if cancel {
        if let Some((id, _, _)) = b.child.take()
            && let Some(j) = s.jobs.iter().find(|j| j.id == id)
        {
            j.progress.cancel.store(true, Ordering::Relaxed);
        }
        for r in b.items.iter_mut().filter(|r| matches!(r.status.as_str(), "queued" | "running")) {
            r.status = "cancelled".into();
        }
        finish(s, &mut b, Err("stopped".into()));
        s.batch = Some(b);
        return;
    }
    let total_steps = (b.items.len() * b.steps.len()).max(1) as u64;
    if b.i >= b.items.len() {
        if b.queued_exports > 0 {
            let _ = s.execute("export.queue.start", json!({}));
        }
        let done = b.items.iter().filter(|r| r.status == "done").count();
        let status = format!("Batch done: {done} of {}", b.items.len());
        finish(s, &mut b, Ok(status));
        s.batch = Some(b);
        return;
    }
    let seq = b.items[b.i].sequence;
    let name = b.items[b.i].name.clone();
    let step = b.steps[b.k];
    let base = (b.i * b.steps.len() + b.k) as u64;
    let label = format!("{} of {} · {name}: {step}", b.i + 1, b.items.len());
    *b.job.progress.status.lock().unwrap_or_else(PoisonError::into_inner) = label;
    b.items[b.i].status = "running".into();
    let outcome: std::result::Result<bool, String> = match b.child.take() {
        // waiting for the step's background job
        Some((id, cmd, q)) => match s.jobs.iter().find(|j| j.id == id).map(|j| j.to_json()) {
            Some(v) if v["finished"] == true => match v["result"]["error"].as_str() {
                Some(e) => Err(e.to_string()),
                None => on_sequence(s, seq, cmd, q).map(|_| true).map_err(|e| short(&e)),
            },
            Some(v) => {
                let f = v["progress"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
                b.job.progress.done.store(base * 1000 + (f * 1000.0) as u64, Ordering::Relaxed);
                b.child = Some((id, cmd, q));
                s.batch = Some(b);
                return;
            }
            None => Err("the step's job is gone".into()),
        },
        None => match start_step(s, &b, seq, step) {
            Ok(Some(child)) => {
                b.child = Some(child);
                s.batch = Some(b);
                return;
            }
            Ok(None) => Ok(true),
            Err(e) => Err(short(&e)),
        },
    };
    let r = &mut b.items[b.i];
    match outcome {
        Ok(_) => {
            r.done.push(step.to_string());
            if step == "export" {
                b.queued_exports += 1;
            }
        }
        Err(e) => r.errors.push((step.to_string(), e)),
    }
    b.k += 1;
    if b.k >= b.steps.len() {
        let r = &mut b.items[b.i];
        r.status = if r.errors.is_empty() { "done".into() } else { "failed".into() };
        b.k = 0;
        b.i += 1;
    }
    b.job.progress.done.store(((base + 1) * 1000).min(total_steps * 1000), Ordering::Relaxed);
    s.batch = Some(b);
}

fn finish(s: &mut Session, b: &mut BatchRun, r: std::result::Result<String, String>) {
    b.finished = true;
    crate::narration::finish(&b.job.progress, &b.job.result, r, web_time::Instant::now());
    let failed: Vec<String> = b.items.iter().filter(|r| !r.errors.is_empty()).map(|r| r.name.clone()).collect();
    if !failed.is_empty() {
        s.log.push(crate::panels::Level::Warning, "batch", format!("Steps failed in: {}", failed.join(", ")));
    }
}

fn start(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "batch.start";
    if s.batch.as_ref().is_some_and(|b| !b.finished) {
        return Err(bad(cmd, "a batch is already running (batch.status; batch.cancel stops it)"));
    }
    let wanted: Vec<ItemId> = p.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ItemId).collect()).unwrap_or_default();
    let mut ids: Vec<ItemId> = Vec::new();
    for id in wanted {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Err(bad(cmd, "`items`: the sequences or clips to process"));
    }
    if ids.len() > BATCH_LIMIT {
        return Err(bad(cmd, format!("at most {BATCH_LIMIT} items in a batch")));
    }
    let steps: Vec<&'static str> = STEPS.iter().copied().filter(|k| bool_p(p, k).unwrap_or(false)).collect();
    if steps.is_empty() {
        return Err(bad(cmd, format!("choose at least one step: {}", STEPS.join(", "))));
    }
    for id in &ids {
        match s.project.item(*id).map(|i| &i.kind) {
            Some(ItemKind::Sequence(_) | ItemKind::Media(_) | ItemKind::Subclip { .. }) => {}
            Some(_) => return Err(bad(cmd, format!("item {} is neither a sequence nor a clip", id.0))),
            None => return Err(bad(cmd, format!("no item {}", id.0))),
        }
    }
    // the engines are checked now, not at the first item: keys, network and consent
    let needs_text = steps.iter().any(|k| matches!(*k, "highlights" | "youtube"));
    if needs_text {
        let probe = with_engine(p, "textEngine");
        crate::ai_tasks::TextEngine::for_task(s, &probe, cmd, "the transcripts")?;
    }
    if steps.contains(&"transcribe") {
        match crate::recognition::engine_choice(s, &with_engine(p, "speechEngine"), cmd)? {
            crate::recognition::EngineChoice::Cloud { provider, .. } => {
                crate::ai::key(s, provider).ok_or_else(|| crate::ai::ai_err(filmcraft_ai::AiError::NoKey(provider)))?;
                crate::recognition::ensure_consent_for(s, provider, p, cmd, "the audio of the sequences")?;
            }
            crate::recognition::EngineChoice::Local(_) => crate::transcript::can_transcribe(s).map_err(|e| bad(cmd, e))?,
        }
    }
    // clips become sequences (as New Sequence From Clip); the editor keeps its sequence
    let (active, open, events) = (s.state.active_sequence, s.state.open_sequences.clone(), s.events.len());
    let mut seqs: Vec<(ItemId, String)> = Vec::new();
    for id in ids {
        let name = s.project.item(id).map(|i| i.name.clone()).unwrap_or_default();
        if matches!(s.project.item(id).map(|i| &i.kind), Some(ItemKind::Sequence(_))) {
            seqs.push((id, name));
            continue;
        }
        match s.execute("file.newSequenceFromClip", json!({"items": [id.0]})) {
            Ok(r) => seqs.extend(r["sequence"].as_u64().map(|q| (ItemId(q), name))),
            Err(e) => s.log.push(crate::panels::Level::Warning, "batch", format!("{name}: {e}")),
        }
    }
    s.state.active_sequence = active;
    s.state.open_sequences = open;
    s.events.truncate(events);
    if seqs.is_empty() {
        return Err(EngineError::Other("none of the items can be made into a sequence".into()));
    }
    let job = crate::narration::new_job(s, "Batch".into());
    job.progress.total.store((seqs.len() * steps.len()) as u64 * 1000, Ordering::Relaxed);
    let id = job.id;
    s.jobs.push(crate::Job { id, label: job.label.clone(), progress: job.progress.clone(), result: job.result.clone() });
    let items: Vec<ItemReport> =
        seqs.into_iter().map(|(sequence, name)| ItemReport { sequence, name, status: "queued".into(), ..Default::default() }).collect();
    s.batch = Some(BatchRun { job, items, steps, params: p.clone(), i: 0, k: 0, child: None, queued_exports: 0, finished: false });
    if bool_p(p, "wait").unwrap_or(false) {
        while s.batch.as_ref().is_some_and(|b| !b.finished) {
            pump(s);
            if s.batch.as_ref().is_some_and(|b| b.child.is_some()) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    status(s, &json!({}))
}

fn status(s: &mut Session, _: &Value) -> Result<Value> {
    s.batch.as_ref().map(BatchRun::to_json).ok_or_else(|| bad("batch.status", "no batch has run in this session"))
}

fn cancel(s: &mut Session, _: &Value) -> Result<Value> {
    let b = s.batch.as_ref().filter(|b| !b.finished).ok_or_else(|| bad("batch.cancel", "no batch is running"))?;
    b.job.progress.cancel.store(true, Ordering::Relaxed);
    pump(s);
    status(s, &json!({}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "batch.start",
            label: "Batch Process…",
            menu: &["File"],
            shortcut: None,
            params: r#"{"items":[id],"transcribe":bool?,"fillers":bool?,"pauses":bool?,"autoZoom":bool?,"highlights":bool?,"youtube":bool?,"export":bool?,"speechEngine":"whisper|gemini|openai"?,"textEngine":"gemini|openai"?,"language":"ru|tg|en|auto"?,"preset":str?,"path":dir?,"confirm":bool?,"wait":bool?}"#,
            enabled: crate::commands::always,
            run: start,
            journal: true,
        },
        CommandSpec {
            id: "batch.status",
            label: "Batch Status",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: crate::commands::always,
            run: status,
            journal: false,
        },
        CommandSpec {
            id: "batch.cancel",
            label: "Stop Batch",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: crate::commands::always,
            run: cancel,
            journal: true,
        },
    ]
}
