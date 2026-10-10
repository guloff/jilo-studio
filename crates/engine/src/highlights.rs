//! Highlights of a recording (`highlights.find`, `highlights.assemble`), as Jilo Studio finds them.
//!
//! The sequence transcript goes to the text engine (Settings ▸ AI) as cues, one per line,
//! `[start-end] text` in sequence seconds, with what to look for: moments that can go viral as a
//! short, the most meaningful complete thoughts, or fact-dense passages. The engine proposes
//! fragments with a title, a score and a reason; the code then checks them (best score first,
//! boundaries snapped to whole cues, cut to the longest allowed length, short and overlapping ones
//! dropped) and adds each as a range marker, named by its title, in one undo step.
//!
//! `highlights.assemble` turns chosen highlight markers into material: a subsequence per
//! highlight, one sequence of all of them in time order, or one export queue item per highlight.

use filmcraft_edit as edit;
use filmcraft_edit::transcript::SeqWord;
use filmcraft_project::{Label, Marker, MarkerId, MarkerKind, TrackItem, TrackKind};
use filmcraft_time::{Tick, TimeRange};
use serde::Serialize;
use serde_json::{Value, json};

use crate::ai_tasks::{Outcome, TaskWork, other, run, started};
use crate::commands::{CommandSpec, bad, f64_p, str_p};
use crate::{EngineError, Result, Session};

/// What a highlight search looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Viral,
    Meaningful,
    Facts,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Viral, Mode::Meaningful, Mode::Facts];

    pub fn id(self) -> &'static str {
        match self {
            Mode::Viral => "viral",
            Mode::Meaningful => "meaningful",
            Mode::Facts => "facts",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id().eq_ignore_ascii_case(id))
    }

    /// What the model is asked to find (Jilo Studio's wording).
    fn brief(self) -> &'static str {
        match self {
            Mode::Viral => {
                "moments with the highest potential to go viral as a standalone short video: a strong hook in the \
                 first two seconds, emotion, surprise, humour, a bold or controversial claim, a relatable pain, a \
                 punchline. The fragment must make sense to a stranger without any context."
            }
            Mode::Meaningful => {
                "the most meaningful self-contained ideas: a complete thought with a setup and a conclusion, useful \
                 advice or insight, the strongest arguments. The viewer should learn something complete."
            }
            Mode::Facts => {
                "fact-dense highlights: concrete numbers, names, dates, definitions, cause and effect, claims that \
                 can be quoted precisely. Prefer informative precision over emotion."
            }
        }
    }

    /// Facts want precision; the other modes some freedom.
    fn temperature(self) -> f32 {
        if self == Mode::Facts { 0.2 } else { 0.5 }
    }

    /// The colour of this mode's markers.
    pub fn color(self) -> Label {
        match self {
            Mode::Viral => Label::Mango,
            Mode::Meaningful => Label::Cerulean,
            Mode::Facts => Label::Forest,
        }
    }
}

/// One line of the transcript as the engine reads it (sequence seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// A new cue after a pause this long.
const CUE_PAUSE: f64 = 0.7;
/// A cue ends at the end of a phrase once it has this many characters.
const CUE_PHRASE_CHARS: usize = 15;
/// No cue is longer.
const CUE_MAX_SECONDS: f64 = 7.0;

/// The sequence transcript as cues: a new cue after a pause, at the end of a phrase once the cue
/// is long enough, and at the latest every seven seconds.
pub fn cues_from_words(words: &[SeqWord]) -> Vec<Cue> {
    cue_spans(words).into_iter().map(|(c, _)| c).collect()
}

/// [`cues_from_words`] with the range of `words` each cue holds.
pub fn cue_spans(words: &[SeqWord]) -> Vec<(Cue, std::ops::Range<usize>)> {
    let mut out: Vec<(Cue, std::ops::Range<usize>)> = Vec::new();
    let mut cur: Option<(Cue, std::ops::Range<usize>)> = None;
    for (i, w) in words.iter().enumerate() {
        let (a, b) = (w.start.seconds(), w.end.seconds().max(w.start.seconds()));
        let text = w.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some((c, r)) = cur.as_mut() {
            if a - c.end > CUE_PAUSE || b - c.start > CUE_MAX_SECONDS {
                out.extend(cur.take());
            } else {
                c.text.push(' ');
                c.text.push_str(text);
                c.end = b;
                r.end = i + 1;
                if c.text.chars().count() >= CUE_PHRASE_CHARS && text.ends_with(['.', '!', '?', '…']) {
                    out.extend(cur.take());
                }
                continue;
            }
        }
        cur = Some((Cue { start: a, end: b, text: text.to_string() }, i..i + 1));
    }
    out.extend(cur);
    out
}

/// The limits of a search, checked and clamped as Jilo Studio does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub count: usize,
    pub min_seconds: f64,
    pub max_seconds: f64,
}

impl Limits {
    /// `count` 1–20 (5), `minSeconds` 5–180 (15), `maxSeconds` at least 5 s more, at most 600 (60).
    pub fn from_params(p: &Value) -> Self {
        let count = p.get("count").and_then(Value::as_f64).filter(|c| c.is_finite()).unwrap_or(5.0).round().clamp(1.0, 20.0) as usize;
        let min_seconds = f64_p(p, "minSeconds").filter(|x| x.is_finite()).unwrap_or(15.0).clamp(5.0, 180.0);
        let max_seconds = f64_p(p, "maxSeconds").filter(|x| x.is_finite()).unwrap_or(60.0).clamp(min_seconds + 5.0, 600.0);
        Limits { count, min_seconds, max_seconds }
    }
}

/// The instructions (system prompt) of a search; the transcript is the user message.
pub fn prompt(mode: Mode, limits: Limits, language: &str) -> String {
    [
        "You are a senior video editor who cuts long recordings into short clips for social media and YouTube.".to_string(),
        format!("Find up to {} {}", limits.count, mode.brief()),
        String::new(),
        "Rules:".into(),
        format!("1. Each fragment lasts between {} and {} seconds.", limits.min_seconds, limits.max_seconds),
        "2. A fragment starts at the beginning of a cue and ends at the end of a cue; use the timestamps given.".into(),
        "3. Fragments must not overlap. Never invent content that is not in the transcript.".into(),
        "4. score is 0-100: how well the fragment fits the goal. Sort by score, best first.".into(),
        format!("5. Write \"title\" (max 8 words, catchy) and \"reason\" (one sentence) in {language}."),
        String::new(),
        "The user message is the transcript, one cue per line as [start-end] text, times in seconds.".into(),
    ]
    .join("\n")
}

/// The transcript as the engine reads it.
pub fn transcript_text(cues: &[Cue]) -> String {
    cues.iter().map(|c| format!("[{:.1}-{:.1}] {}", c.start, c.end, c.text.split_whitespace().collect::<Vec<_>>().join(" "))).collect::<Vec<_>>().join("\n")
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"fragments": {"type": "array", "items": {"type": "object", "properties": {
            "start": {"type": "number"}, "end": {"type": "number"}, "title": {"type": "string"},
            "score": {"type": "number"}, "reason": {"type": "string"}
        }, "required": ["start", "end", "title", "score", "reason"]}}},
        "required": ["fragments"]
    })
}

/// A checked highlight (sequence seconds).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fragment {
    pub start: f64,
    pub end: f64,
    pub title: String,
    pub score: u32,
    pub reason: String,
}

/// How far a proposed boundary may be from the cue it snaps to.
const SNAP: f64 = 0.5;

/// Check the engine's fragments against the cues (Jilo Studio's `snapFragments`): best score
/// first; the start snaps to the last cue starting by it (+0.5 s), the end to the first cue ending
/// by it (−0.5 s); a fragment longer than the limit ends at the last cue that fits (else is cut);
/// reversed fragments, fragments outside the transcript, fragments shorter than
/// `min(minSeconds, 3)` and fragments overlapping a better one are dropped.
pub fn snap_fragments(raw: &[Value], cues: &[Cue], limits: Limits) -> Vec<Fragment> {
    let (Some(first), Some(last)) = (cues.first(), cues.last()) else { return Vec::new() };
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
    let mut proposed: Vec<&Value> = raw
        .iter()
        .filter(|v| num(v, "start").is_some() && num(v, "end").is_some() && v.get("title").and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty()))
        .collect();
    proposed.sort_by(|a, b| num(b, "score").unwrap_or(0.0).total_cmp(&num(a, "score").unwrap_or(0.0)));
    let mut out: Vec<Fragment> = Vec::new();
    for v in proposed {
        let (want_start, want_end) = (num(v, "start").unwrap_or(0.0), num(v, "end").unwrap_or(0.0));
        // a reversed range, or one outside the transcript, is invented: snapping would make it real
        if want_end <= want_start || want_start > last.end + SNAP || want_end < first.start - SNAP {
            continue;
        }
        let start_cue = cues.iter().rev().find(|c| c.start <= want_start + SNAP).unwrap_or(first);
        let end_cue = cues.iter().find(|c| c.end >= want_end - SNAP).unwrap_or(last);
        let start = start_cue.start;
        let mut end = end_cue.end.max(start + 1.0);
        if end - start > limits.max_seconds {
            end = cues.iter().rev().find(|c| c.end <= start + limits.max_seconds && c.end > start).map_or(start + limits.max_seconds, |c| c.end);
        }
        if end - start < limits.min_seconds.min(3.0) || out.iter().any(|o| start < o.end && end > o.start) {
            continue;
        }
        out.push(Fragment {
            start,
            end,
            title: v.get("title").and_then(Value::as_str).unwrap_or_default().trim().to_string(),
            score: num(v, "score").unwrap_or(0.0).round().clamp(0.0, 100.0) as u32,
            reason: v.get("reason").and_then(Value::as_str).unwrap_or_default().trim().to_string(),
        });
        if out.len() >= limits.count {
            break;
        }
    }
    out
}

/// A marker comment: the score and the reason.
pub fn marker_comment(f: &Fragment) -> String {
    if f.reason.is_empty() { format!("{}/100", f.score) } else { format!("{}/100 · {}", f.score, f.reason) }
}

fn find(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "highlights.find";
    let mode = match str_p(p, "mode") {
        Some(m) => Mode::from_id(m).ok_or_else(|| bad(cmd, format!("unknown mode `{m}` (viral, meaningful or facts)")))?,
        None => Mode::Viral,
    };
    let limits = Limits::from_params(p);
    let words = crate::transcript::sequence_words(s);
    let cues = cues_from_words(&words);
    if cues.is_empty() {
        return Err(bad(cmd, "the sequence has no transcript (Transcribe first)"));
    }
    // titles in the language spoken (else the one asked for)
    let spoken = words.first().and_then(|w| s.project.transcripts.get(&w.item)).map(|t| t.language.clone()).filter(|l| !l.is_empty());
    let language = str_p(p, "language").map(str::to_string).or(spoken).unwrap_or_else(|| "en".into());
    let language_name = filmcraft_ai::speech::language_name(&language);
    let language_name = language_name.split(" (").next().unwrap_or(&language).to_string();
    let transcript = transcript_text(&cues);
    let inputs =
        json!({"mode": mode.id(), "count": limits.count, "min": limits.min_seconds, "max": limits.max_seconds, "language": language, "transcript": transcript});
    let system = prompt(mode, limits, &language_name);
    let work: TaskWork = Box::new(move |engine, ctx, progress| {
        if !progress(0.1, "Reading the transcript") {
            return Err(filmcraft_ai::AiError::Cancelled);
        }
        let v = engine.json(ctx, &system, &transcript, Some(&schema()), mode.temperature())?;
        let _ = progress(1.0, "Highlights found");
        Ok(v)
    });
    let answer = match run(s, p, cmd, "Finding highlights", &inputs, "the transcript", work)? {
        Outcome::Started(id) => return Ok(started(id)),
        Outcome::Ready(v) => v,
    };
    // an answer of the wrong shape is a failure, not "nothing interesting"
    let Some(raw) = answer.get("fragments").and_then(Value::as_array) else {
        return Err(other("the engine did not answer with a list of fragments; try again or another engine in Settings ▸ AI"));
    };
    let fragments = snap_fragments(raw, &cues, limits);
    if fragments.is_empty() {
        return Err(other(if raw.is_empty() {
            "the engine found no highlights in this transcript".to_string()
        } else {
            format!("the engine proposed {} fragments, but none matches the transcript's timing", raw.len())
        }));
    }
    let color = mode.color();
    let marks: Vec<Fragment> = fragments.clone();
    let ids = s.edit_sequence("Find Highlights", |q, ctx, _| {
        let mut ids = Vec::new();
        for f in &marks {
            let id = MarkerId(ctx.alloc());
            let start = Tick::from_seconds_f64(f.start);
            let duration = Tick::from_seconds_f64(f.end) - start;
            q.markers.push(Marker { id, start, duration, name: f.title.clone(), comment: marker_comment(f), kind: MarkerKind::Comment, color });
            ids.push(id);
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(ids)
    })?;
    let list: Vec<Value> = fragments
        .iter()
        .zip(&ids)
        .map(|(f, id)| json!({"marker": id.0, "start": f.start, "end": f.end, "title": f.title, "score": f.score, "reason": f.reason}))
        .collect();
    Ok(json!({"mode": mode.id(), "language": language, "highlights": list}))
}

/// What `highlights.assemble` makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assemble {
    Subsequences,
    Sequence,
    Queue,
}

/// The track items of `q` within `ranges`, laid end to end from time zero in the given order
/// (each range keeps its own links).
fn clips_in_ranges(q: &filmcraft_project::Sequence, ranges: &[TimeRange], mut alloc: impl FnMut() -> u64) -> Vec<(TrackKind, usize, TrackItem)> {
    let mut out = Vec::new();
    let mut at = Tick::ZERO;
    for r in ranges {
        let mut links = std::collections::HashMap::new();
        for (kind, tracks) in [(TrackKind::Video, &q.video_tracks), (TrackKind::Audio, &q.audio_tracks)] {
            for (ti, tr) in tracks.iter().enumerate() {
                for it in &tr.items {
                    if let Some(mut c) = edit::through::clip_item_to(it, *r) {
                        c.id = filmcraft_project::ClipId(alloc());
                        c.start = c.start - r.start + at;
                        if let Some(l) = c.link {
                            c.link = Some(*links.entry(l).or_insert_with(&mut alloc));
                        }
                        out.push((kind, ti, c));
                    }
                }
            }
        }
        at += r.duration;
    }
    out
}

fn assemble(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "highlights.assemble";
    let how = match str_p(p, "as").unwrap_or("subsequences") {
        "subsequences" => Assemble::Subsequences,
        "sequence" => Assemble::Sequence,
        "queue" => Assemble::Queue,
        other => return Err(bad(cmd, format!("`as`: subsequences, sequence or queue, not `{other}`"))),
    };
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let wanted: Vec<u64> = p.get("markers").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if wanted.is_empty() {
        return Err(bad(cmd, "`markers`: the ids of the highlight markers to use"));
    }
    let mut chosen: Vec<&Marker> = Vec::new();
    for id in &wanted {
        let m = q.markers.iter().find(|m| m.id.0 == *id).ok_or_else(|| bad(cmd, format!("no marker {id} in the active sequence")))?;
        if m.duration <= Tick::ZERO {
            return Err(bad(cmd, format!("marker {id} has no duration")));
        }
        chosen.push(m);
    }
    chosen.sort_by_key(|m| m.start);
    chosen.dedup_by_key(|m| m.id);
    let seq_name = s.project.item(seq_id).map(|i| i.name.clone()).unwrap_or_default();
    if how == Assemble::Queue {
        let dir = match str_p(p, "path") {
            Some(d) => std::path::PathBuf::from(crate::export_tools::expand_home(d)),
            None => crate::export_tools::default_export_dir(s),
        };
        let mut added = Vec::new();
        for (n, m) in chosen.iter().enumerate() {
            let name = crate::export_tools::file_safe(&format!("{seq_name} {:02} {}", n + 1, m.name));
            let mut params = json!({
                "sequence": seq_id.0,
                "range": {"mode": "custom", "startTime": m.start.0, "endTime": (m.start + m.duration).0},
                "path": dir.join(name).to_string_lossy(),
            });
            if let Some(preset) = p.get("preset") {
                params["preset"] = preset.clone();
            }
            let r = s.execute("export.queue.add", params)?;
            added.extend(r["added"].as_array().cloned().unwrap_or_default());
        }
        return Ok(json!({"queued": added}));
    }
    let pieces: Vec<(String, Vec<TimeRange>)> = match how {
        Assemble::Subsequences => chosen.iter().map(|m| (format!("{seq_name} · {}", m.name), vec![TimeRange::new(m.start, m.duration)])).collect(),
        _ => vec![(
            str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("{seq_name} · Highlights")),
            chosen.iter().map(|m| TimeRange::new(m.start, m.duration)).collect(),
        )],
    };
    let label = if how == Assemble::Sequence { "Assemble Highlights" } else { "Highlights as Subsequences" };
    let made = s.edit(label, |pr, st| {
        let bin = pr.root.parent_of(seq_id);
        let mut made = Vec::new();
        for (name, ranges) in &pieces {
            let placed = clips_in_ranges(&q, ranges, || pr.alloc_id());
            if placed.is_empty() {
                continue;
            }
            let nid = pr.new_sequence(name, q.settings.clone(), q.video_tracks.len(), q.audio_tracks.len(), bin);
            let nq = pr.sequence_mut(nid).ok_or(EngineError::NoSequence)?;
            crate::sequence_tools::lay_out_tracks_like(nq, &q);
            for (kind, ti, it) in placed {
                if let Some(tr) = nq.tracks_mut(kind).get_mut(ti) {
                    tr.items.push(it);
                }
            }
            for tr in nq.all_tracks_mut() {
                tr.sort();
            }
            nq.check().map_err(EngineError::Other)?;
            made.push(nid);
        }
        if made.is_empty() {
            return Err(EngineError::Other("there are no clips under the highlights".into()));
        }
        st.project_selection = made.clone();
        Ok(made)
    })?;
    Ok(json!({"sequences": made.iter().map(|i| i.0).collect::<Vec<_>>()}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "highlights.find",
            label: "Find Highlights…",
            menu: &["Markers"],
            shortcut: None,
            params: r#"{"mode":"viral|meaningful|facts"?,"count":1-20?,"minSeconds":5-180?,"maxSeconds":≤600?,"language":str?,"engine":"gemini|openai|anthropic"?,"model":str?,"confirm":bool?,"background":bool?}"#,
            enabled: crate::transcript::has_transcript,
            run: find,
            journal: true,
        },
        CommandSpec {
            id: "highlights.assemble",
            label: "Assemble Highlights",
            menu: &[],
            shortcut: None,
            params: r#"{"markers":[id],"as":"subsequences|sequence|queue"?,"name":str?,"path":dir?,"preset":str?}"#,
            enabled: crate::commands::has_seq,
            run: assemble,
            journal: true,
        },
    ]
}
