//! Speech recognition engines for `transcript.generate` / `transcript.recognize`: Whisper on this
//! computer (the default) or a cloud engine (Google Gemini, OpenAI) chosen in Settings ▸ AI or by
//! the `engine` parameter.
//!
//! | command | does |
//! |---|---|
//! | `transcript.recognize` | recognise in a background job (progress, Stop); the transcripts wait in [`Session::recognition_cache`] until `transcript.generate` with the same settings applies them as one undo step |
//! | `transcript.generate` | apply cached transcripts, recognising (in the call) whatever is not cached |
//!
//! **Nothing goes to the cloud without consent.** A cloud engine runs only after the user agreed to
//! send audio to that provider: Settings ▸ AI keeps the agreement (`ai.confirmedProviders`), the
//! interface asks once per provider, and agents pass `confirm: true` (which records it). Otherwise
//! the command fails before any audio is read.
//!
//! Cloud recognition goes window by window ([`filmcraft_speech::chunked`]): WAV windows (16-bit mono
//! 16 kHz, our own writer), 2 minutes for Gemini and 10 for OpenAI, 3 at a time, each checked
//! against the sound and recognised again in halves when incomplete. The cues are tidied, become
//! words, word bounds are tightened by level, and speakers come from the model's labels or, when it
//! gave none, from FilmCraft's own speaker labelling. A retired Gemini model is replaced by the one
//! its error names, and Settings ▸ AI remembers the replacement.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_ai::speech::{Recognised, SpeechRequest};
use filmcraft_ai::{AiError, Provider, Secret, Transport};
use filmcraft_project::{ItemId, ItemKind, Transcript};
use filmcraft_speech::chunked::{ChunkOptions, ChunkReport, WindowAnswer, WindowRecognizer, WindowRequest};
use filmcraft_speech::cues::{Cue, CueWord};
use filmcraft_speech::{Options, SpeechError, Transcriber};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{bad, bool_p, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// The engine a recognition runs on.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineChoice {
    /// A Whisper model of the catalogue (or the host's installed transcriber).
    Local(String),
    Cloud {
        provider: Provider,
        model: String,
    },
}

impl EngineChoice {
    pub fn describe(&self) -> String {
        match self {
            EngineChoice::Local(m) => m.clone(),
            EngineChoice::Cloud { provider, model } => format!("{}:{model}", provider.id()),
        }
    }
}

/// A transcript recognised in the background, waiting to be applied.
#[derive(Clone, Debug)]
pub struct Recognition {
    pub item: ItemId,
    /// The settings it was recognised with ([`plan_key`]).
    pub key: String,
    pub transcript: Transcript,
    pub report: Option<ChunkReport>,
    /// A retired cloud model was replaced by this one.
    pub model_switched: Option<String>,
}

pub type RecognitionCache = Arc<Mutex<Vec<Recognition>>>;

/// A recognition's settings.
pub struct Plan {
    pub engine: EngineChoice,
    pub opts: Options,
    pub transcriber: Arc<dyn Transcriber>,
    /// The cloud transcriber behind `transcriber`, for its report.
    pub cloud: Option<Arc<CloudTranscriber>>,
    pub key: String,
}

pub(crate) fn opts_from(s: &Session, p: &Value) -> Options {
    // Settings ▸ Media Analysis & Transcription: language (or auto-detect) and speaker labelling
    let ma = &s.prefs.media_analysis;
    let default_language = if ma.language_auto_detect { None } else { Some(ma.default_language.clone()) };
    Options {
        language: match str_p(p, "language") {
            Some(l) => Some(l).filter(|l| !l.is_empty() && *l != "auto").map(str::to_string),
            None => default_language,
        },
        diarize: bool_p(p, "diarize").unwrap_or(ma.speaker_labeling != "off"),
        max_speakers: u64_p(p, "maxSpeakers").map(|n| n.clamp(1, 32) as usize).unwrap_or(Options::default().max_speakers),
    }
}

/// The engine `p` asks for (`engine`, `model`), else Settings ▸ AI and Media Analysis.
pub fn engine_choice(s: &Session, p: &Value, cmd: &str) -> Result<EngineChoice> {
    let id = str_p(p, "engine").map(str::to_string).unwrap_or_else(|| s.prefs.ai.speech_engine.clone());
    match id.as_str() {
        "" | "whisper" | "local" => Ok(EngineChoice::Local(str_p(p, "model").unwrap_or(&s.prefs.media_analysis.whisper_model).to_string())),
        other => {
            let provider = Provider::from_id(other).ok_or_else(|| bad(cmd, format!("unknown engine `{other}` (whisper, gemini or openai)")))?;
            let model = str_p(p, "model").map(str::to_string).unwrap_or_else(|| s.prefs.ai.speech_model(provider));
            Ok(EngineChoice::Cloud { provider, model })
        }
    }
}

/// The cache key of a recognition: engine, model, language and speaker settings.
pub fn plan_key(engine: &EngineChoice, opts: &Options) -> String {
    format!("{}|{}|{}|{}", engine.describe(), opts.language.as_deref().unwrap_or("auto"), opts.diarize, opts.max_speakers)
}

/// Seconds of audio of `items` (what a cloud engine would receive, before overlaps).
pub fn audio_seconds(s: &Session, items: &[ItemId]) -> f64 {
    items
        .iter()
        .filter_map(|i| match &s.project.item(*i)?.kind {
            ItemKind::Media(m) => Some(m.duration().seconds()),
            _ => None,
        })
        .sum()
}

/// Seconds of audio `p` would recognise (the clips `transcript.generate` would take).
pub fn target_seconds(s: &Session, p: &Value) -> f64 {
    audio_seconds(s, &crate::transcript::targets(s, p))
}

/// Make sure the user agreed to send audio to `provider`: Settings ▸ AI remembers it; `confirm:
/// true` gives (and records) it.
pub fn ensure_consent(s: &mut Session, provider: Provider, p: &Value, cmd: &str, seconds: f64) -> Result<()> {
    ensure_consent_for(s, provider, p, cmd, &format!("{} of audio", minutes(seconds)))
}

/// [`ensure_consent`] for sending `what` (text of the transcript, captions…).
pub fn ensure_consent_for(s: &mut Session, provider: Provider, p: &Value, cmd: &str, what: &str) -> Result<()> {
    if s.prefs.ai.confirmed(provider) {
        return Ok(());
    }
    if bool_p(p, "confirm") == Some(true) {
        let mut list = s.prefs.ai.confirmed_providers.clone();
        list.push(provider.id().to_string());
        s.execute("prefs.set", json!({"key": "ai.confirmedProviders", "value": list}))?;
        return Ok(());
    }
    Err(bad(cmd, format!("CONFIRM_CLOUD: {} would receive {what}. Ask the user, then pass confirm: true (asked once per provider).", provider.name())))
}

fn minutes(seconds: f64) -> String {
    if seconds < 60.0 { format!("{:.0} s", seconds.max(1.0)) } else { format!("{:.0} min", (seconds / 60.0).ceil()) }
}

/// The recognition `p` asks for. A cloud engine needs a key, the network and the user's consent.
pub fn plan(s: &mut Session, p: &Value, cmd: &str, items: &[ItemId]) -> Result<Plan> {
    let engine = engine_choice(s, p, cmd)?;
    let opts = opts_from(s, p);
    let key = plan_key(&engine, &opts);
    match &engine {
        EngineChoice::Local(_) => {
            let transcriber = crate::transcript::local_transcriber(s, p)?;
            Ok(Plan { engine, opts, transcriber, cloud: None, key })
        }
        EngineChoice::Cloud { provider, model } => {
            let (provider, model) = (*provider, model.clone());
            let (secret, _) = crate::ai::key(s, provider).ok_or_else(|| crate::ai::ai_err(AiError::NoKey(provider)))?;
            let transport = crate::ai::transport(s).map_err(crate::ai::ai_err)?;
            ensure_consent(s, provider, p, cmd, audio_seconds(s, items))?;
            let cloud = Arc::new(CloudTranscriber::new(provider, model, secret, transport, s.ai_sleep.clone()));
            Ok(Plan { engine, opts, transcriber: cloud.clone(), cloud: Some(cloud), key })
        }
    }
}

/// Mono 16 kHz audio of a media source of length `dur` (None: no audio).
pub fn source_audio(src: &filmcraft_media::SharedSource, dur: Tick) -> Option<Vec<f32>> {
    if !src.info().has_audio() {
        return None;
    }
    let sr = filmcraft_speech::SAMPLE_RATE;
    let len = dur.to_units_floor(sr as i64).max(0) as usize;
    let buf = src.audio(0, len, sr).ok()?;
    Some(filmcraft_speech::downmix(&buf.channels))
}

/// A waiting recognition of `item` with settings `key` (taken out of the cache).
pub fn take_cached(s: &Session, item: ItemId, key: &str) -> Option<Recognition> {
    let mut c = s.recognition_cache.lock().unwrap_or_else(PoisonError::into_inner);
    let i = c.iter().position(|r| r.item == item && r.key == key)?;
    Some(c.remove(i))
}

/// The report of a recognition as JSON.
pub fn report_json(r: &ChunkReport) -> Value {
    json!({
        "windows": r.windows,
        "retried": r.retried,
        "uncoveredSec": (r.uncovered_seconds * 10.0).round() / 10.0,
        "secondsSent": r.seconds_sent.round(),
    })
}

/// `transcript.recognize`: recognise `items` in a background job.
pub fn recognize(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "transcript.recognize";
    let items = crate::transcript::targets(s, p);
    if items.is_empty() {
        return Err(bad(cmd, "nothing to transcribe (pass `items`, select clips, or open a sequence with audio)"));
    }
    let plan = plan(s, p, cmd, &items)?;
    let sources: Vec<(ItemId, filmcraft_media::SharedSource, Tick)> = items
        .iter()
        .filter_map(|&i| {
            let dur = match &s.project.item(i)?.kind {
                ItemKind::Media(m) => m.duration(),
                _ => return None,
            };
            Some((i, s.source(i)?, dur))
        })
        .collect();
    if sources.is_empty() {
        return Err(EngineError::Other("none of the clips has audio to transcribe".into()));
    }
    let label = "Transcribing".to_string();
    let engine = plan.engine.describe();
    let job = crate::narration::new_job(s, label);
    job.progress.total.store(1000 * sources.len() as u64, Ordering::Relaxed);
    let (prog, res, cache) = (job.progress.clone(), job.result.clone(), s.recognition_cache.clone());
    let wait = bool_p(p, "wait").unwrap_or(false);
    let id = crate::narration::spawn(s, job, wait, "filmcraft-recognize", move || {
        let t0 = web_time::Instant::now();
        let r = run_job(&plan, &sources, &prog, &cache);
        crate::narration::finish(&prog, &res, r, t0);
    })?;
    Ok(json!({"job": id, "items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "engine": engine}))
}

fn run_job(
    plan: &Plan,
    sources: &[(ItemId, filmcraft_media::SharedSource, Tick)],
    prog: &filmcraft_export::Progress,
    cache: &RecognitionCache,
) -> std::result::Result<String, String> {
    let mut done_words = 0usize;
    for (n, (item, src, dur)) in sources.iter().enumerate() {
        if prog.cancel.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("{}: reading the audio…", plan.engine.describe());
        let Some(audio) = source_audio(src, *dur) else { continue };
        let base = 1000 * n as u64;
        let mut progress = |f: f32, status: &str| {
            if f >= 0.0 {
                prog.done.store(base + (f.clamp(0.0, 1.0) * 1000.0) as u64, Ordering::Relaxed);
            }
            if !status.is_empty() {
                *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = status.to_string();
            }
            !prog.cancel.load(Ordering::Relaxed)
        };
        let mut t = plan.transcriber.transcribe(&audio, &plan.opts, &mut progress).map_err(|e| e.to_string())?;
        t.normalize();
        done_words += t.words.len();
        let (report, model_switched) = match &plan.cloud {
            Some(c) => (c.last_report(), c.switched_model()),
            None => (None, None),
        };
        cache.lock().unwrap_or_else(PoisonError::into_inner).push(Recognition { item: *item, key: plan.key.clone(), transcript: t, report, model_switched });
    }
    prog.done.store(1000 * sources.len() as u64, Ordering::Relaxed);
    Ok(format!("Recognised {done_words} words; Transcribe to add them"))
}

/// A cloud engine as a [`Transcriber`].
pub struct CloudTranscriber {
    provider: Provider,
    model: Mutex<String>,
    key: Secret,
    transport: Arc<dyn Transport>,
    sleep: Option<filmcraft_ai::SleepFn>,
    report: Mutex<Option<ChunkReport>>,
    switched: Mutex<Option<String>>,
}

impl CloudTranscriber {
    pub fn new(provider: Provider, model: String, key: Secret, transport: Arc<dyn Transport>, sleep: Option<filmcraft_ai::SleepFn>) -> Self {
        Self { provider, model: Mutex::new(model), key, transport, sleep, report: Mutex::new(None), switched: Mutex::new(None) }
    }

    fn model(&self) -> String {
        self.model.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn last_report(&self) -> Option<ChunkReport> {
        self.report.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn switched_model(&self) -> Option<String> {
        self.switched.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn chunk_options(&self) -> ChunkOptions {
        ChunkOptions { window_seconds: filmcraft_ai::speech::max_window_seconds(self.provider), ..Default::default() }
    }

    /// Recognise with `model`, window by window, stopping when `stop` is raised.
    fn recognise_with(
        &self,
        audio: &[f32],
        model: &str,
        language: Option<&str>,
        stop: &Arc<AtomicBool>,
        frac: &AtomicU32,
        status: &Mutex<String>,
    ) -> std::result::Result<(Vec<Cue>, Option<String>, ChunkReport), (SpeechError, Option<AiError>)> {
        let stop_c = stop.clone();
        let mut ctx = filmcraft_ai::Ctx::new(self.transport.clone()).with_cancel(move || stop_c.load(Ordering::Relaxed));
        if let Some(sl) = &self.sleep {
            ctx = ctx.with_sleep(sl.clone());
        }
        let win = Windows { ctx, provider: self.provider, key: &self.key, model, language, last_error: Mutex::new(None) };
        let progress = |f: f32, st: &str| {
            if f >= 0.0 {
                frac.store(f.to_bits(), Ordering::Relaxed);
            }
            *status.lock().unwrap_or_else(PoisonError::into_inner) = st.to_string();
            !stop.load(Ordering::Relaxed)
        };
        filmcraft_speech::chunked::recognise(audio, &win, self.chunk_options(), &progress)
            .map_err(|e| (e, win.last_error.lock().unwrap_or_else(PoisonError::into_inner).take()))
    }
}

/// The windows of one recognition, sent to the provider.
struct Windows<'a> {
    ctx: filmcraft_ai::Ctx,
    provider: Provider,
    key: &'a Secret,
    model: &'a str,
    language: Option<&'a str>,
    last_error: Mutex<Option<AiError>>,
}

impl WindowRecognizer for Windows<'_> {
    fn recognise(&self, w: &WindowRequest) -> std::result::Result<WindowAnswer, SpeechError> {
        let wav = filmcraft_media::wav::write_wav16(w.audio, 1, filmcraft_speech::SAMPLE_RATE);
        let req = SpeechRequest { wav: &wav, duration: w.duration, offset: w.offset, language: self.language };
        match filmcraft_ai::speech::transcribe(&self.ctx, self.provider, self.key, self.model, &req) {
            Ok(r) => Ok(answer(r)),
            Err(AiError::Cancelled) => Err(SpeechError::Cancelled),
            Err(e) => {
                let msg = e.to_string();
                *self.last_error.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
                Err(SpeechError::Model(msg))
            }
        }
    }

    fn word_timings(&self) -> bool {
        // OpenAI models other than whisper-1 give no word times
        self.provider == Provider::Gemini || self.model == filmcraft_ai::openai::DEFAULT_SPEECH_MODEL
    }
}

fn answer(r: Recognised) -> WindowAnswer {
    WindowAnswer {
        cues: r
            .segments
            .into_iter()
            .map(|s| Cue {
                start: s.start,
                end: s.end,
                text: s.text,
                speaker: s.speaker,
                words: (!s.words.is_empty()).then(|| s.words.into_iter().map(|w| CueWord::new(w.text, w.start, w.end)).collect()),
            })
            .collect(),
        language: r.language,
        truncated: r.truncated,
    }
}

impl Transcriber for CloudTranscriber {
    fn id(&self) -> String {
        format!("{}:{}", self.provider.id(), self.model())
    }

    fn transcribe(&self, audio: &[f32], opts: &Options, progress: filmcraft_speech::ProgressFn) -> std::result::Result<Transcript, SpeechError> {
        let stop = Arc::new(AtomicBool::new(false));
        let frac = AtomicU32::new(0f32.to_bits());
        let status = Mutex::new(String::new());
        let language = opts.language.clone();
        let model = self.model();
        // the windows run on their own threads; this one reports progress and passes Stop on
        let outcome = std::thread::scope(|s| {
            let h = s.spawn(|| {
                let first = self.recognise_with(audio, &model, language.as_deref(), &stop, &frac, &status);
                match first {
                    Err((_, Some(AiError::ModelRetired { replacement: Some(r), .. }))) if r != model => {
                        *self.switched.lock().unwrap_or_else(PoisonError::into_inner) = Some(r.clone());
                        *self.model.lock().unwrap_or_else(PoisonError::into_inner) = r.clone();
                        self.recognise_with(audio, &r, language.as_deref(), &stop, &frac, &status).map(|x| (x, r))
                    }
                    other => other.map(|x| (x, model.clone())),
                }
            });
            while !h.is_finished() {
                let st = status.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if !progress(f32::from_bits(frac.load(Ordering::Relaxed)), &st) {
                    stop.store(true, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            h.join().unwrap_or(Err((SpeechError::Model("recognition stopped unexpectedly".into()), None)))
        });
        let ((cues, heard, report), model) = outcome.map_err(|(e, _)| e)?;
        let lang = opts.language.clone().or(heard).unwrap_or_else(|| "und".into());
        let mut t = filmcraft_speech::chunked::transcript_from_cues(&cues, &lang, &format!("{}:{model}", self.provider.id()));
        filmcraft_speech::vad::tighten_words(audio, &mut t.words);
        if opts.diarize && t.speakers.is_empty() && !t.words.is_empty() {
            let p = filmcraft_speech::diarize::Params { max_speakers: opts.max_speakers, ..Default::default() };
            filmcraft_speech::diarize::diarize(audio, &mut t, &p);
        }
        *self.report.lock().unwrap_or_else(PoisonError::into_inner) = Some(report);
        let _ = progress(1.0, "Done");
        Ok(t)
    }
}
