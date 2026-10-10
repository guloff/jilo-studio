//! YouTube texts and chapters (`youtube.generate`, `youtube.texts`, `youtube.set`), as Jilo Studio
//! writes them: three titles, a description, chapters, tags and hashtags from the sequence
//! transcript, with YouTube's rules checked in code.
//!
//! The transcript goes to the text engine of Settings ▸ AI in blocks of at least 8 s
//! (`[start second] text`). The chapters it proposes start on a cue (the cue begun by then; one
//! closer than 10 s to the previous moves on to the next cue at least 10 s later, not dropped); the
//! first starts at 0:00, one that leaves less than 10 s to the end is left out. They become the
//! sequence's chapter markers (replacing the ones there unless `replaceChapters` is false), so they
//! follow the edit, go into exported MP4/MOV files and can be moved by hand. The texts are kept with
//! the sequence (item metadata `youtube`); `youtube.texts` assembles the description with the
//! chapters of the current chapter markers, as YouTube wants them: at least 3, the first at 0:00,
//! each at least 10 s long.

use filmcraft_project::{ItemId, Label, Marker, MarkerId, MarkerKind};
use filmcraft_time::Tick;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ai_tasks::{Outcome, TaskWork, other, run, started};
use crate::commands::{CommandSpec, bad, bool_p, str_p};
use crate::highlights::{Cue, cues_from_words};
use crate::{EngineError, Result, Session};

pub const MIN_CHAPTERS: usize = 3;
pub const MIN_CHAPTER_SECONDS: f64 = 10.0;
pub const TITLE_LIMIT: usize = 100;
pub const DESCRIPTION_LIMIT: usize = 5000;
/// The tags field, counting the commas between tags and the quotes around tags with a space.
pub const TAGS_LIMIT: usize = 500;
const MAX_HASHTAGS: usize = 3;
/// Where the texts are kept: the sequence item's metadata.
pub const META_KEY: &str = "youtube";

/// The texts of a sequence for YouTube.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Texts {
    pub language: String,
    pub titles: Vec<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub hashtags: Vec<String>,
}

/// One line: no angle brackets (YouTube refuses them), spaces collapsed, at most `limit` characters.
pub fn clean_line(s: &str, limit: usize) -> String {
    let line = s.replace(['<', '>'], "").split_whitespace().collect::<Vec<_>>().join(" ");
    line.chars().take(limit).collect::<String>().trim().to_string()
}

/// Text with paragraphs kept: no angle brackets, spaces collapsed within lines, at most one empty
/// line in a row, at most `limit` characters.
pub fn clean_text(s: &str, limit: usize) -> String {
    let lines: Vec<String> =
        s.replace("\r\n", "\n").replace('\r', "\n").replace(['<', '>'], "").split('\n').map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let mut out: Vec<String> = Vec::new();
    for l in lines {
        if l.is_empty() && out.last().is_some_and(|p: &String| p.is_empty()) {
            continue;
        }
        out.push(l);
    }
    let text = out.join("\n");
    text.trim().chars().take(limit).collect::<String>().trim().to_string()
}

/// The length of the tags field as YouTube counts it.
pub fn tags_length(tags: &[String]) -> usize {
    tags.iter().map(|t| t.chars().count() + if t.contains(' ') { 2 } else { 0 }).sum::<usize>() + tags.len().saturating_sub(1)
}

pub fn normalise_tags<'a>(raw: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let tag = clean_line(item.replace(',', " ").trim_start_matches('#'), 100);
        if tag.is_empty() || out.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
            continue;
        }
        let mut next = out.clone();
        next.push(tag);
        if tags_length(&next) > TAGS_LIMIT {
            continue;
        }
        out = next;
    }
    out
}

pub fn normalise_hashtags<'a>(raw: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let word: String = clean_line(item, 60).trim_start_matches('#').chars().filter(|c| !c.is_whitespace() && *c != '#' && *c != ',').collect();
        if word.is_empty() || out.iter().any(|h| h[1..].to_lowercase() == word.to_lowercase()) {
            continue;
        }
        out.push(format!("#{word}"));
        if out.len() >= MAX_HASHTAGS {
            break;
        }
    }
    out
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

impl Texts {
    /// Checked fields of `v` over `base` (the same checks for the engine's answer and for edits).
    pub fn normalised(v: &Value, base: &Texts) -> Texts {
        Texts {
            language: v.get("language").and_then(Value::as_str).map(|l| l.chars().take(8).collect()).unwrap_or_else(|| base.language.clone()),
            titles: match v.get("titles") {
                Some(t) if t.is_array() => strings(t).into_iter().map(|x| clean_line(x, TITLE_LIMIT)).filter(|x| !x.is_empty()).take(5).collect(),
                _ => base.titles.clone(),
            },
            description: v.get("description").and_then(Value::as_str).map_or_else(|| base.description.clone(), |d| clean_text(d, DESCRIPTION_LIMIT)),
            tags: match v.get("tags") {
                Some(t) if t.is_array() => normalise_tags(strings(t)),
                Some(Value::String(t)) => normalise_tags(t.split(',')),
                _ => base.tags.clone(),
            },
            hashtags: match v.get("hashtags") {
                Some(t) if t.is_array() => normalise_hashtags(strings(t)),
                Some(Value::String(t)) => normalise_hashtags(t.split_whitespace()),
                _ => base.hashtags.clone(),
            },
        }
    }
}

/// `0:00` under an hour, `0:00:00` from an hour on (all chapters of a video look the same).
pub fn format_chapter_time(seconds: f64, total: f64) -> String {
    let whole = (seconds + 1e-6).floor().max(0.0) as u64;
    let (h, m, s) = (whole / 3600, (whole % 3600) / 60, whole % 60);
    if total >= 3600.0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Why YouTube would not show chapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChapterProblem {
    TooFew,
    VideoTooShort,
}

impl ChapterProblem {
    pub fn id(self) -> &'static str {
        match self {
            ChapterProblem::TooFew => "tooFew",
            ChapterProblem::VideoTooShort => "videoTooShort",
        }
    }
}

/// Chapters as the description lists them (Jilo Studio's `timedChapters`): whole seconds down, the
/// first at 0:00, one closer than 10 s to the previous left out, and so is a last one with less than
/// 10 s to the end. `marks`: (seconds, title).
pub fn timed_chapters(marks: &[(f64, String)], total: f64) -> (Vec<(f64, String)>, Vec<ChapterProblem>) {
    let mut placed: Vec<(f64, String)> = marks.iter().map(|(t, n)| ((t + 1e-6).floor().max(0.0), n.clone())).collect();
    placed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, String)> = Vec::new();
    for (t, n) in placed {
        match out.last() {
            None => out.push((0.0, n)),
            Some(last) if t - last.0 >= MIN_CHAPTER_SECONDS => out.push((t, n)),
            _ => {}
        }
    }
    while out.len() > 1 && out.last().is_some_and(|l| total - l.0 < MIN_CHAPTER_SECONDS) {
        out.pop();
    }
    let mut problems = Vec::new();
    if total < MIN_CHAPTERS as f64 * MIN_CHAPTER_SECONDS {
        problems.push(ChapterProblem::VideoTooShort);
    } else if out.len() < MIN_CHAPTERS {
        problems.push(ChapterProblem::TooFew);
    }
    (out, problems)
}

/// Which cues start the chapters the engine asked for (Jilo Studio's `pickChapterCues`): the cue
/// begun by then (+0.5 s); one closer than 10 s to the previous chapter moves on to the first cue at
/// least 10 s later; the first chapter is kept (it will start at 0:00); one that leaves less than
/// 10 s to the end is not taken. Returns cue indices.
pub fn pick_chapter_cues(requested: &[f64], cue_starts: &[f64], total: f64) -> Vec<usize> {
    let floors: Vec<f64> = cue_starts.iter().map(|s| (s + 1e-6).floor()).collect();
    let mut want: Vec<f64> = requested.iter().copied().filter(|x| x.is_finite()).collect();
    want.sort_by(f64::total_cmp);
    let mut chosen: Vec<usize> = Vec::new();
    let mut previous = 0.0;
    for w in want {
        let mut index = floors.iter().rposition(|f| *f <= w + 0.5).unwrap_or(0);
        let Some(&last) = chosen.last() else {
            chosen.push(index);
            continue;
        };
        index = index.max(last + 1);
        while index < floors.len() && floors[index] - previous < MIN_CHAPTER_SECONDS {
            index += 1;
        }
        let Some(&f) = floors.get(index) else { continue };
        if total - f < MIN_CHAPTER_SECONDS {
            continue;
        }
        chosen.push(index);
        previous = f;
    }
    chosen
}

/// How many chapters to ask for: YouTube wants at least 3 of at least 10 s.
fn chapter_range(total: f64) -> (usize, usize) {
    let max = ((total / MIN_CHAPTER_SECONDS).floor() as usize).min(((total / 60.0).round() as usize).clamp(3, 30));
    (max.min(3), max)
}

/// Cues joined into blocks of at least 8 s: fewer timestamps for the engine to read.
fn blocks(cues: &[Cue]) -> Vec<(f64, String)> {
    let mut out: Vec<(f64, String)> = Vec::new();
    for c in cues {
        let text = c.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if c.start - last.0 < 8.0 => {
                last.1.push(' ');
                last.1.push_str(&text);
            }
            _ => out.push((c.start, text)),
        }
    }
    out
}

pub fn prompt(total: f64, language: &str) -> String {
    let (min, max) = chapter_range(total);
    let chapters = if max >= 3 {
        format!(
            "3. \"chapters\": between {min} and {max} chapters that follow the structure of the talk. The first starts at 0. Each \"start\" must be one of the block times given in square brackets. Chapters are at least {} seconds apart. Chapter titles are at most 50 characters.",
            MIN_CHAPTER_SECONDS
        )
    } else {
        "3. \"chapters\": return an empty array — the video is too short for chapters.".to_string()
    };
    [
        "You are a YouTube producer who writes metadata that helps a video get found and watched.".to_string(),
        format!("The video lasts {} seconds. Write everything in {language}.", total.round()),
        String::new(),
        "Rules:".into(),
        "1. \"titles\": 3 different title options, each at most 70 characters, specific and honest, no clickbait lies.".into(),
        "2. \"description\": 2-4 short paragraphs, at most 1500 characters. The first sentence must hook the viewer".into(),
        "   (it is shown in search). No timestamps, no links, no angle brackets, no hashtags inside the text.".into(),
        chapters,
        "4. \"tags\": 10-20 search phrases people would type to find this video, without \"#\".".into(),
        "5. \"hashtags\": 3 hashtags without spaces.".into(),
        "6. Never invent facts, names or numbers that are not in the transcript.".into(),
        String::new(),
        "The user message is the transcript in blocks, each line is [start second] text.".into(),
    ]
    .join("\n")
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "titles": {"type": "array", "items": {"type": "string"}},
            "description": {"type": "string"},
            "chapters": {"type": "array", "items": {"type": "object", "properties": {"start": {"type": "number"}, "title": {"type": "string"}}, "required": ["start", "title"]}},
            "tags": {"type": "array", "items": {"type": "string"}},
            "hashtags": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["titles", "description", "chapters", "tags", "hashtags"]
    })
}

/// The texts kept with sequence `seq`.
pub fn texts_of(s: &Session, seq: ItemId) -> Option<Texts> {
    let raw = s.project.item(seq)?.metadata.get(META_KEY)?;
    serde_json::from_str(raw).ok()
}

/// The sequence's chapter markers as (seconds, title).
fn chapter_marks(s: &Session) -> Vec<(f64, String)> {
    s.active_sequence()
        .map(|q| q.markers.iter().filter(|m| m.kind == MarkerKind::Chapter).map(|m| (m.start.seconds(), m.name.clone())).collect())
        .unwrap_or_default()
}

/// The description as it goes into YouTube: the text, the chapters (when YouTube will take them)
/// and the hashtags; past the limit the text is shortened, not the chapters.
pub fn full_description(t: &Texts, chapters: &[(f64, String)], problems: &[ChapterProblem], total: f64) -> String {
    let list = if problems.is_empty() {
        chapters.iter().map(|(s, n)| format!("{} {n}", format_chapter_time(*s, total))).collect::<Vec<_>>().join("\n")
    } else {
        String::new()
    };
    let tail = [list, t.hashtags.join(" ")].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n\n");
    let room = DESCRIPTION_LIMIT.saturating_sub(if tail.is_empty() { 0 } else { tail.chars().count() + 2 });
    let body: String = t.description.chars().take(room).collect::<String>().trim().to_string();
    [body, tail].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join("\n\n")
}

fn texts_json(s: &Session, seq: ItemId, t: &Texts) -> Value {
    let total = s.project.sequence(seq).map_or(0.0, |q| q.duration().seconds());
    let (chapters, problems) = timed_chapters(&chapter_marks(s), total);
    let description = full_description(t, &chapters, &problems, total);
    let tags = t.tags.join(", ");
    let all = [t.titles.first().cloned().unwrap_or_default(), description.clone(), tags.clone()]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    json!({
        "language": t.language,
        "titles": t.titles,
        "text": t.description,
        "tags": t.tags,
        "hashtags": t.hashtags,
        "chapters": chapters.iter().map(|(sec, n)| json!({"time": sec, "label": format_chapter_time(*sec, total), "title": n})).collect::<Vec<_>>(),
        "problems": problems.iter().map(|p| p.id()).collect::<Vec<_>>(),
        "description": description,
        "tagsText": tags,
        "all": all,
    })
}

fn generate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "youtube.generate";
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let words = crate::transcript::sequence_words(s);
    let cues = cues_from_words(&words);
    if cues.is_empty() {
        return Err(bad(cmd, "the sequence has no transcript (Transcribe first)"));
    }
    let spoken = words.first().and_then(|w| s.project.transcripts.get(&w.item)).map(|t| t.language.clone()).filter(|l| !l.is_empty());
    let language = str_p(p, "language").map(str::to_string).or(spoken).unwrap_or_else(|| "en".into());
    let language_name = filmcraft_ai::speech::language_name(&language);
    let language_name = language_name.split(" (").next().unwrap_or(&language).to_string();
    let total = s.active_sequence().map_or(0.0, |q| q.duration().seconds());
    let transcript = blocks(&cues).iter().map(|(t, x)| format!("[{}] {x}", t.floor())).collect::<Vec<_>>().join("\n");
    let inputs = json!({"language": language, "total": total.round(), "transcript": transcript});
    let system = prompt(total, &language_name);
    let work: TaskWork = Box::new(move |engine, ctx, progress| {
        if !progress(0.1, "Reading the transcript") {
            return Err(filmcraft_ai::AiError::Cancelled);
        }
        let v = engine.json(ctx, &system, &transcript, Some(&schema()), 0.4)?;
        let _ = progress(1.0, "Texts written");
        Ok(v)
    });
    let answer = match run(s, p, cmd, "Writing YouTube texts", &inputs, "the transcript", work)? {
        Outcome::Started(id) => return Ok(started(id)),
        Outcome::Ready(v) => v,
    };
    let texts = Texts::normalised(&answer, &Texts { language: language.clone(), ..Texts::default() });
    if texts.description.is_empty() && texts.titles.is_empty() {
        return Err(other("the engine wrote no description; try again or another engine in Settings ▸ AI"));
    }
    // chapters on cues: the title of the requested chapter nearest to each chosen cue
    let mut requested: Vec<(f64, String)> = answer
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let start = c.get("start").and_then(Value::as_f64).filter(|x| x.is_finite())?;
            let title = clean_line(c.get("title").and_then(Value::as_str).unwrap_or_default(), TITLE_LIMIT);
            (!title.is_empty()).then_some((start, title))
        })
        .collect();
    requested.sort_by(|a, b| a.0.total_cmp(&b.0));
    let starts: Vec<f64> = cues.iter().map(|c| c.start).collect();
    let picked = pick_chapter_cues(&requested.iter().map(|r| r.0).collect::<Vec<_>>(), &starts, total);
    let mut chapters: Vec<(Tick, String)> = Vec::new();
    let mut cursor = 0;
    for (k, ci) in picked.iter().enumerate() {
        let Some(cue) = cues.get(*ci) else { continue };
        let Some(mut best) = (cursor < requested.len()).then_some(cursor) else { break };
        for i in cursor..requested.len() {
            if (requested[i].0 - cue.start).abs() < (requested[best].0 - cue.start).abs() {
                best = i;
            }
        }
        cursor = best + 1;
        let at = if k == 0 { Tick::ZERO } else { Tick::from_seconds_f64(cue.start.floor()) };
        chapters.push((at, requested[best].1.clone()));
    }
    let replace = bool_p(p, "replaceChapters").unwrap_or(true);
    let stored = serde_json::to_string(&texts).map_err(|e| other(e.to_string()))?;
    s.edit("YouTube Texts", |pr, _| {
        if let Some(it) = pr.items.get_mut(&seq_id) {
            it.metadata.insert(META_KEY.into(), stored);
        }
        let ids: Vec<MarkerId> = chapters.iter().map(|_| MarkerId(pr.alloc_id())).collect();
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        if !chapters.is_empty() {
            if replace {
                q.markers.retain(|m| m.kind != MarkerKind::Chapter);
            }
            for ((at, title), id) in chapters.iter().zip(ids) {
                q.markers.push(Marker {
                    id,
                    start: *at,
                    duration: Tick::ZERO,
                    name: title.clone(),
                    comment: String::new(),
                    kind: MarkerKind::Chapter,
                    color: Label::Green,
                });
            }
            q.markers.sort_by_key(|m| m.start);
        }
        Ok(())
    })?;
    Ok(texts_json(s, seq_id, &texts))
}

fn texts(s: &mut Session, _: &Value) -> Result<Value> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let t = texts_of(s, seq_id).ok_or_else(|| bad("youtube.texts", "this sequence has no YouTube texts yet (youtube.generate)"))?;
    Ok(texts_json(s, seq_id, &t))
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let base = texts_of(s, seq_id).unwrap_or_default();
    let mut v = p.clone();
    // `text` is the description without chapters and hashtags, as youtube.texts returns it
    if let (Some(t), Some(o)) = (p.get("text").cloned(), v.as_object_mut()) {
        o.insert("description".into(), t);
    }
    let t = Texts::normalised(&v, &base);
    if t == base {
        return Ok(texts_json(s, seq_id, &t));
    }
    let stored = serde_json::to_string(&t).map_err(|e| other(e.to_string()))?;
    s.edit("Edit YouTube Texts", |pr, _| {
        let it = pr.items.get_mut(&seq_id).ok_or(EngineError::NoSequence)?;
        it.metadata.insert(META_KEY.into(), stored);
        Ok(())
    })?;
    Ok(texts_json(s, seq_id, &t))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "youtube.generate",
            label: "YouTube Title, Description and Chapters…",
            menu: &["Sequence"],
            shortcut: None,
            params: r#"{"language":"ru|tg|en|…"?,"replaceChapters":bool=true,"engine":"gemini|openai|anthropic"?,"model":str?,"confirm":bool?,"background":bool?}"#,
            enabled: crate::transcript::has_transcript,
            run: generate,
            journal: true,
        },
        CommandSpec {
            id: "youtube.texts",
            label: "YouTube Texts",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: crate::commands::has_seq,
            run: texts,
            journal: false,
        },
        CommandSpec {
            id: "youtube.set",
            label: "Edit YouTube Texts",
            menu: &[],
            shortcut: None,
            params: r##"{"titles":[str]?,"text":str?,"tags":[str]|"a, b"?,"hashtags":[str]|"#a #b"?}"##,
            enabled: crate::commands::has_seq,
            run: set,
            journal: true,
        },
    ]
}
