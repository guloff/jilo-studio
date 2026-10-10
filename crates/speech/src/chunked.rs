//! Recognising a long recording window by window (cloud engines).
//!
//! The recording is cut into windows (2 minutes for Gemini: in Jilo Studio's measurements on an
//! 18-minute video a 2-minute window kept word times for 14 cues of 14, an 8.75-minute one for 0 of
//! 61, and a 10-minute one lost its last 106 seconds of speech). Each window is sent with 3 s of
//! audio on each side so a phrase on the cut is not lost; a cue belongs to the window it starts in
//! (±0.2 s). Three windows are recognised at a time.
//!
//! Every answer is checked against the sound: it was not cut off (`truncated`), all the window's
//! speech (by level, −45 dB / 0.6 s) is covered by words (pad 0.6 s, at most 4 s uncovered), and —
//! for engines that give word times — at most a quarter of the cues failed to align to the model's
//! words. Otherwise the window is split in half and both halves are recognised again (windows
//! longer than 30 s, at most 3 levels deep). The windows are then tidied together
//! ([`crate::cues::tidy`], [`crate::cues::normalise_cues`]). Ported from Jilo Studio
//! (`server/src/plugins/transcription/index.ts`, `runChunked`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use filmcraft_audio_dsp::silence;

use crate::cues::{CUE_LIMITS, Cue, align_cue_words, letter_count, normalise_cues, tidy, uncovered_speech};
use crate::{SAMPLE_RATE, SpeechError};

/// One window to recognise: mono 16 kHz `audio` starting `offset` seconds into the recording.
#[derive(Clone, Copy, Debug)]
pub struct WindowRequest<'a> {
    pub audio: &'a [f32],
    pub offset: f64,
    pub duration: f64,
}

/// A window's answer; cue and word times are relative to the window's first sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowAnswer {
    pub cues: Vec<Cue>,
    /// The language the engine heard (ISO 639-1 or a name such as `"Russian"`).
    pub language: Option<String>,
    /// The answer stopped at the engine's output limit.
    pub truncated: bool,
}

/// An engine that recognises one window.
pub trait WindowRecognizer: Sync {
    fn recognise(&self, w: &WindowRequest) -> Result<WindowAnswer, SpeechError>;
    /// Whether the engine's cues carry word times (Gemini, OpenAI `whisper-1`).
    fn word_timings(&self) -> bool {
        true
    }
}

/// How windows are made and checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChunkOptions {
    pub window_seconds: f64,
    pub overlap_seconds: f64,
    pub parallel: usize,
    /// Windows shorter than this are not split again.
    pub min_split_seconds: f64,
    pub max_depth: u32,
    /// Speech not covered by words above this marks a window incomplete.
    pub max_uncovered_seconds: f64,
    /// Level below which the sound is a pause (checking coverage).
    pub silence_db: f32,
    pub silence_seconds: f64,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            window_seconds: 120.0,
            overlap_seconds: 3.0,
            parallel: 3,
            min_split_seconds: 30.0,
            max_depth: 3,
            max_uncovered_seconds: 4.0,
            silence_db: -45.0,
            silence_seconds: 0.6,
        }
    }
}

/// What the recognition did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChunkReport {
    pub windows: usize,
    /// Windows split in half and recognised again.
    pub retried: usize,
    /// Seconds of speech left without words.
    pub uncovered_seconds: f64,
    /// Seconds of audio sent to the engine (overlaps and retries included).
    pub seconds_sent: f64,
    /// Letters in the accepted window cues and in the final cues (they should match).
    pub letters_in: usize,
    pub letters_out: usize,
}

/// Progress: `(fraction, status)`; return false to stop.
pub type ChunkProgress<'a> = &'a (dyn Fn(f32, &str) -> bool + Sync);

struct Run<'a> {
    audio: &'a [f32],
    total: f64,
    rec: &'a dyn WindowRecognizer,
    opts: ChunkOptions,
    stop: AtomicBool,
    report: Mutex<ChunkReport>,
    language: Mutex<Option<String>>,
    progress: ChunkProgress<'a>,
}

fn sample(t: f64) -> usize {
    (t.max(0.0) * SAMPLE_RATE as f64).round() as usize
}

impl Run<'_> {
    fn check(&self) -> Result<(), SpeechError> {
        if self.stop.load(Ordering::Relaxed) { Err(SpeechError::Cancelled) } else { Ok(()) }
    }

    /// Cues starting in `[from, to)` of the recording (absolute times).
    fn window(&self, from: f64, to: f64, depth: u32) -> Result<Vec<Cue>, SpeechError> {
        self.check()?;
        let clip_start = (from - self.opts.overlap_seconds).max(0.0);
        let clip_end = (to + self.opts.overlap_seconds).min(self.total);
        let (a, b) = (sample(clip_start).min(self.audio.len()), sample(clip_end).min(self.audio.len()));
        let clip = &self.audio[a..b.max(a)];
        let len = clip_end - clip_start;
        let answer = self.rec.recognise(&WindowRequest { audio: clip, offset: clip_start, duration: len })?;
        {
            let mut r = self.report.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            r.seconds_sent += len;
        }
        if let Some(l) = answer.language.as_deref().filter(|l| !l.trim().is_empty()) {
            let mut lang = self.language.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if lang.is_none() {
                *lang = Some(normalise_language(l));
            }
        }
        let clamp = |t: f64| clip_start + t.clamp(0.0, len);
        let mut cues = Vec::new();
        for c in &answer.cues {
            let (start, end) = (clamp(c.start), clamp(c.end));
            if end <= start || !start.is_finite() {
                continue;
            }
            // a cue belongs to the window it starts in: the overlap is the neighbour's
            if (from > 0.0 && start < from - 0.2) || (to < self.total && start >= to - 0.2) {
                continue;
            }
            let words = c.words.as_ref().map(|ws| {
                ws.iter().map(|w| crate::cues::CueWord::new(w.text.clone(), clamp(w.start), clamp(w.end))).filter(|w| w.end > w.start).collect::<Vec<_>>()
            });
            cues.push(Cue { start, end: end.min(self.total), text: c.text.clone(), speaker: c.speaker.clone(), words: words.filter(|w| !w.is_empty()) });
        }
        // the window's speech, by level, must be covered by words (or by cues without words)
        let pauses = silence::silences(clip, SAMPLE_RATE, self.opts.silence_db, self.opts.silence_seconds);
        let speech: Vec<(f64, f64)> = silence::speech_between(&pauses, len)
            .into_iter()
            .map(|s| ((clip_start + s.start).max(from), (clip_start + s.end).min(to)))
            .filter(|(s, e)| e > s)
            .collect();
        let spoken: Vec<(f64, f64)> = cues
            .iter()
            .flat_map(|c| match &c.words {
                Some(ws) => ws.iter().map(|w| (w.start, w.end)).collect::<Vec<_>>(),
                None => vec![(c.start, c.end)],
            })
            .collect();
        let uncovered = uncovered_speech(&speech, &spoken, 0.6);
        let unaligned = if self.rec.word_timings() {
            let mut sorted = cues.clone();
            sorted.sort_by(|a, b| a.start.total_cmp(&b.start));
            align_cue_words(&sorted).iter().filter(|w| w.is_none()).count()
        } else {
            0
        };
        let incomplete = answer.truncated || uncovered > self.opts.max_uncovered_seconds || (!cues.is_empty() && unaligned as f64 > cues.len() as f64 * 0.25);
        if incomplete && to - from > self.opts.min_split_seconds && depth < self.opts.max_depth {
            self.report.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retried += 1;
            let _ = (self.progress)(-1.0, &format!("Part {}–{} was not recognised completely; recognising its halves", clock(from), clock(to)));
            let middle = (from + to) / 2.0;
            let (left, right) = std::thread::scope(|s| {
                let l = s.spawn(|| self.window(from, middle, depth + 1));
                let r = self.window(middle, to, depth + 1);
                (l.join().unwrap_or(Err(SpeechError::Model("a recognition worker stopped".into()))), r)
            });
            let mut out = left?;
            out.extend(right?);
            return Ok(out);
        }
        self.report.lock().unwrap_or_else(std::sync::PoisonError::into_inner).uncovered_seconds += uncovered;
        Ok(cues)
    }
}

fn clock(s: f64) -> String {
    let w = s.max(0.0).round() as u64;
    format!("{}:{:02}", w / 60, w % 60)
}

/// `"Russian"` → `"ru"`; ISO codes pass through (lower case).
pub fn normalise_language(l: &str) -> String {
    let k = l.trim().to_lowercase();
    match k.as_str() {
        "russian" => "ru".into(),
        "tajik" | "tadjik" => "tg".into(),
        "english" => "en".into(),
        "persian" | "farsi" => "fa".into(),
        "uzbek" => "uz".into(),
        _ => k.chars().take(5).collect(),
    }
}

/// Recognise mono 16 kHz `audio` window by window with `rec`. Returns the tidied cues (seconds from
/// the first sample), the language heard first, and the report.
pub fn recognise(
    audio: &[f32],
    rec: &dyn WindowRecognizer,
    opts: ChunkOptions,
    progress: ChunkProgress,
) -> Result<(Vec<Cue>, Option<String>, ChunkReport), SpeechError> {
    let total = (audio.len() as f64 / SAMPLE_RATE as f64).max(0.1);
    let step = opts.window_seconds.max(1.0);
    let mut windows = Vec::new();
    let mut start = 0.0;
    while start < total {
        windows.push((start, (start + step).min(total)));
        start += step;
    }
    let run = Run {
        audio,
        total,
        rec,
        opts,
        stop: AtomicBool::new(false),
        report: Mutex::new(ChunkReport { windows: windows.len(), ..Default::default() }),
        language: Mutex::new(None),
        progress,
    };
    if !progress(0.0, &if windows.len() > 1 { format!("Recognising {} parts", windows.len()) } else { "Recognising speech".to_string() }) {
        return Err(SpeechError::Cancelled);
    }
    let results: Mutex<Vec<Option<Result<Vec<Cue>, SpeechError>>>> = Mutex::new(vec![None; windows.len()]);
    let next = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..opts.parallel.clamp(1, 8).min(windows.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(from, to)) = windows.get(i) else { break };
                    let r = if run.stop.load(Ordering::Relaxed) { Err(SpeechError::Cancelled) } else { run.window(from, to, 0) };
                    let failed = r.is_err();
                    results.lock().unwrap_or_else(std::sync::PoisonError::into_inner)[i] = Some(r);
                    if failed {
                        // one failed window fails the whole recognition: stop the others
                        run.stop.store(true, Ordering::Relaxed);
                        break;
                    }
                    let done = finished.fetch_add(1, Ordering::Relaxed) + 1;
                    let status = if windows.len() > 1 { format!("Recognised {done} of {} parts", windows.len()) } else { "Speech recognised".to_string() };
                    if !progress(done as f32 / windows.len() as f32, &status) {
                        run.stop.store(true, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let mut merged = Vec::new();
    let mut first_err = None;
    for r in results.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner) {
        match r {
            Some(Ok(c)) => merged.extend(c),
            // a real failure outranks the "stopped" of the windows it stopped
            Some(Err(e)) if first_err.is_none() || first_err == Some(SpeechError::Cancelled) => first_err = Some(e),
            Some(Err(_)) => {}
            None => first_err = first_err.or(Some(SpeechError::Cancelled)),
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    let cues = normalise_cues(&tidy(&merged), CUE_LIMITS);
    let mut report = run.report.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    report.letters_in = letter_count(merged.iter().map(|c| c.text.as_str()));
    report.letters_out = letter_count(cues.iter().map(|c| c.text.as_str()));
    let language = run.language.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok((cues, language, report))
}

/// A transcript from tidied cues: their words (see [`crate::cues::cue_words`]) with the model's
/// speaker labels as speakers ("S1" → "Speaker 1"), well formed ([`Transcript::normalize`]).
pub fn transcript_from_cues(cues: &[Cue], language: &str, source: &str) -> filmcraft_project::Transcript {
    use filmcraft_project::{Speaker, Transcript, Word};
    let mut t = Transcript { language: language.to_string(), source: source.to_string(), ..Default::default() };
    let mut labels: Vec<String> = Vec::new();
    for (w, label) in crate::cues::cue_words(cues) {
        let mut word = Word::new(w.text, crate::seconds_tick(w.start), crate::seconds_tick(w.end));
        if let Some(l) = label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()) {
            let i = labels.iter().position(|x| *x == l).unwrap_or_else(|| {
                labels.push(l.clone());
                labels.len() - 1
            });
            word.speaker = Some(i as u32);
        }
        t.words.push(word);
    }
    t.speakers = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let generic = l.len() <= 3 && l.starts_with(['S', 's']) && l[1..].chars().all(|c| c.is_ascii_digit());
            Speaker { name: if generic { format!("Speaker {}", i + 1) } else { l.clone() } }
        })
        .collect();
    t.normalize();
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cues::CueWord;

    /// Speech (a 220 Hz tone) in `parts` of the recording, silence elsewhere.
    fn audio(total: f64, parts: &[(f64, f64)]) -> Vec<f32> {
        let n = (total * SAMPLE_RATE as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / SAMPLE_RATE as f64;
                if parts.iter().any(|&(s, e)| t >= s && t < e) { 0.3 * (t as f32 * 220.0 * std::f32::consts::TAU).sin() } else { 0.0 }
            })
            .collect()
    }

    /// A fake engine: knows the words of the whole recording (absolute times) and answers each window
    /// with the cues inside it, one cue per word; windows longer than `limit` come back truncated.
    struct Fake {
        words: Vec<(f64, f64, String)>,
        limit: f64,
        calls: AtomicUsize,
    }

    impl WindowRecognizer for Fake {
        fn recognise(&self, w: &WindowRequest) -> Result<WindowAnswer, SpeechError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let truncated = w.duration > self.limit;
            let cues = self
                .words
                .iter()
                .filter(|(s, _, _)| *s >= w.offset && *s < w.offset + w.duration)
                .filter(|(s, _, _)| !truncated || *s < w.offset + self.limit)
                .map(|(s, e, t)| {
                    let (s, e) = (s - w.offset, e - w.offset);
                    Cue { start: s, end: e, text: t.clone(), speaker: None, words: Some(vec![CueWord::new(t.clone(), s, e)]) }
                })
                .collect();
            Ok(WindowAnswer { cues, language: Some("Russian".into()), truncated })
        }
    }

    fn words(n: usize, every: f64) -> Vec<(f64, f64, String)> {
        (0..n).map(|i| (i as f64 * every + 0.5, i as f64 * every + 1.0, format!("слово{i}."))).collect()
    }

    #[test]
    fn windows_overlap_and_every_cue_is_kept_once() {
        let ws = words(30, 10.0); // a word every 10 s over 5 minutes
        let a = audio(300.0, &ws.iter().map(|w| (w.0, w.1)).collect::<Vec<_>>());
        let fake = Fake { words: ws.clone(), limit: f64::INFINITY, calls: AtomicUsize::new(0) };
        let (cues, lang, report) = recognise(&a, &fake, ChunkOptions::default(), &|_, _| true).unwrap();
        assert_eq!(report.windows, 3);
        assert_eq!(report.retried, 0);
        assert_eq!(lang.as_deref(), Some("ru"));
        let texts: Vec<&str> = cues.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts.len(), 30, "{texts:?}");
        for (i, w) in ws.iter().enumerate() {
            assert_eq!(texts[i], w.2);
            assert!((cues[i].start - w.0).abs() < 1e-6);
        }
        assert_eq!(report.letters_in, report.letters_out);
        // three windows of 2 min with 3 s overlaps: 123 + 126 + 63 s
        assert!((report.seconds_sent - 312.0).abs() < 1e-6, "{}", report.seconds_sent);
    }

    #[test]
    fn a_cut_off_answer_is_recognised_again_in_halves() {
        let ws = words(12, 10.0);
        let a = audio(120.0, &ws.iter().map(|w| (w.0, w.1)).collect::<Vec<_>>());
        // the engine cuts off anything longer than 40 s
        let fake = Fake { words: ws, limit: 40.0, calls: AtomicUsize::new(0) };
        let (cues, _, report) = recognise(&a, &fake, ChunkOptions::default(), &|_, _| true).unwrap();
        assert_eq!(cues.len(), 12, "{cues:#?}");
        assert!(report.retried >= 3, "{report:?}");
    }

    #[test]
    fn speech_without_words_marks_a_window_incomplete() {
        // the engine hears nothing of a long stretch of speech at 30–60 s
        let a = audio(90.0, &[(5.0, 6.0), (30.0, 60.0)]);
        let fake = Fake { words: vec![(5.0, 6.0, "одно.".into())], limit: f64::INFINITY, calls: AtomicUsize::new(0) };
        let opts = ChunkOptions { window_seconds: 90.0, ..Default::default() };
        let (_, _, report) = recognise(&a, &fake, opts, &|_, _| true).unwrap();
        assert!(report.retried >= 1);
        assert!(report.uncovered_seconds > 25.0, "{report:?}");
    }

    #[test]
    fn stop_and_errors_end_the_run() {
        let a = audio(400.0, &[]);
        let fake = Fake { words: vec![], limit: f64::INFINITY, calls: AtomicUsize::new(0) };
        assert_eq!(recognise(&a, &fake, ChunkOptions::default(), &|f, _| f <= 0.0).unwrap_err(), SpeechError::Cancelled);
        struct Broken;
        impl WindowRecognizer for Broken {
            fn recognise(&self, _: &WindowRequest) -> Result<WindowAnswer, SpeechError> {
                Err(SpeechError::Model("BAD_API_KEY".into()))
            }
        }
        assert_eq!(recognise(&a, &Broken, ChunkOptions::default(), &|_, _| true).unwrap_err(), SpeechError::Model("BAD_API_KEY".into()));
        assert_eq!(normalise_language("Tajik"), "tg");
        assert_eq!(normalise_language("EN"), "en");
    }

    #[test]
    fn cues_become_a_transcript_with_speakers() {
        let cues = vec![
            Cue { start: 0.0, end: 1.0, text: "Салом дӯстон".into(), speaker: Some("S1".into()), words: None },
            Cue {
                start: 1.5, end: 2.0, text: "Привет".into(), speaker: Some("Рустам".into()), words: Some(vec![CueWord::new("Привет", 1.5, 2.0)])
            },
        ];
        let t = transcript_from_cues(&cues, "tg", "gemini:gemini-flash-latest");
        assert_eq!(t.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["Салом", "дӯстон", "Привет"]);
        assert_eq!(t.words[2].start, crate::seconds_tick(1.5));
        assert_eq!(t.speakers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Speaker 1", "Рустам"]);
        assert_eq!((t.words[0].speaker, t.words[2].speaker), (Some(0), Some(1)));
        assert!(t.check().is_ok());
    }
}
