//! Silence and speech by level: the stretches of a recording quieter than a threshold for at least
//! a minimum time (what ffmpeg's `silencedetect` reports, without ffmpeg).
//!
//! The signal is measured in 10 ms frames (RMS, dBFS). A silence is a run of frames below the
//! threshold lasting at least `min_seconds`; speech is everything between silences. Measuring frames
//! instead of single samples keeps a zero crossing inside a word from counting as quiet.
//!
//! Defaults used by FilmCraft: pauses −35 dB / 0.6 s; checking that recognised text covers all the
//! speech −45 dB / 0.6 s (quiet speech averages about −38 dBFS, real pauses are below −85 dBFS).

/// A stretch of time in seconds, `start < end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

impl Span {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Frame length of the level measurement, seconds.
pub const FRAME_SECONDS: f64 = 0.01;

/// RMS level of each 10 ms frame of mono `samples` at `rate`, dBFS (−120 for digital silence).
pub fn frame_levels_db(samples: &[f32], rate: u32) -> Vec<f32> {
    let frame = ((rate as f64 * FRAME_SECONDS).round() as usize).max(1);
    samples
        .chunks(frame)
        .map(|c| {
            let e = c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32;
            (10.0 * e.max(1e-12).log10()).max(-120.0)
        })
        .collect()
}

/// The silences of mono `samples` at `rate`: runs below `threshold_db` (dBFS) lasting at least
/// `min_seconds`, in time order. A silence that reaches the end of the recording counts.
pub fn silences(samples: &[f32], rate: u32, threshold_db: f32, min_seconds: f64) -> Vec<Span> {
    if samples.is_empty() || rate == 0 {
        return Vec::new();
    }
    silences_in_levels(&frame_levels_db(samples, rate), samples.len() as f64 / rate as f64, threshold_db, min_seconds)
}

/// [`silences`] from 10 ms frame levels (see [`frame_levels_db`]) of a recording `duration`
/// seconds long, for long recordings measured block by block.
pub fn silences_in_levels(levels: &[f32], duration: f64, threshold_db: f32, min_seconds: f64) -> Vec<Span> {
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    for (i, &db) in levels.iter().chain(std::iter::once(&f32::INFINITY)).enumerate() {
        match (db < threshold_db, run) {
            (true, None) => run = Some(i),
            (false, Some(from)) => {
                let span = Span { start: from as f64 * FRAME_SECONDS, end: (i as f64 * FRAME_SECONDS).min(duration) };
                if span.len() + 1e-9 >= min_seconds {
                    out.push(span);
                }
                run = None;
            }
            _ => {}
        }
    }
    out
}

/// Speech: the parts of `[0, duration]` between `silences` (sorted, as [`silences`] returns them),
/// dropping pieces shorter than a frame.
pub fn speech_between(silences: &[Span], duration: f64) -> Vec<Span> {
    let mut out = Vec::new();
    let mut cursor = 0.0;
    for s in silences {
        if s.start > cursor + FRAME_SECONDS / 2.0 {
            out.push(Span { start: cursor, end: s.start.min(duration) });
        }
        cursor = cursor.max(s.end);
        if cursor >= duration {
            break;
        }
    }
    if duration > cursor + FRAME_SECONDS / 2.0 {
        out.push(Span { start: cursor, end: duration });
    }
    out
}

/// Speech of mono `samples`: [`speech_between`] the [`silences`].
pub fn speech(samples: &[f32], rate: u32, threshold_db: f32, min_silence_seconds: f64) -> Vec<Span> {
    if rate == 0 {
        return Vec::new();
    }
    let duration = samples.len() as f64 / rate as f64;
    speech_between(&silences(samples, rate, threshold_db, min_silence_seconds), duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// `parts` of (seconds, amplitude) as a sine at 220 Hz.
    fn signal(parts: &[(f64, f32)]) -> Vec<f32> {
        let mut out = Vec::new();
        for &(secs, amp) in parts {
            let n = (secs * RATE as f64) as usize;
            out.extend((0..n).map(|i| amp * (i as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin()));
        }
        out
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.011
    }

    #[test]
    fn finds_pauses_longer_than_the_minimum() {
        // speech 1 s, pause 0.8 s, speech 1 s, pause 0.3 s (too short), speech 0.5 s, pause 1 s to the end
        let s = signal(&[(1.0, 0.3), (0.8, 0.0), (1.0, 0.3), (0.3, 0.0), (0.5, 0.3), (1.0, 0.0)]);
        let found = silences(&s, RATE, -35.0, 0.6);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(close(found[0].start, 1.0) && close(found[0].end, 1.8), "{:?}", found[0]);
        assert!(close(found[1].start, 3.6) && close(found[1].end, 4.6), "{:?}", found[1]);
        let sp = speech_between(&found, 4.6);
        assert_eq!(sp.len(), 2);
        assert!(close(sp[0].end, 1.0) && close(sp[1].start, 1.8) && close(sp[1].end, 3.6));
    }

    #[test]
    fn the_threshold_decides_what_is_quiet() {
        // −40 dBFS tone (RMS of a 0.0141 sine ≈ −40 dB): silent at −35, speech at −45
        let s = signal(&[(1.0, 0.3), (1.0, 0.0141), (1.0, 0.3)]);
        assert_eq!(silences(&s, RATE, -35.0, 0.6).len(), 1);
        assert!(silences(&s, RATE, -45.0, 0.6).is_empty());
    }

    #[test]
    fn edge_cases_never_panic() {
        assert!(silences(&[], RATE, -35.0, 0.6).is_empty());
        assert!(silences(&[0.0; 10], 0, -35.0, 0.6).is_empty());
        let quiet = vec![0.0; RATE as usize];
        let all = silences(&quiet, RATE, -35.0, 0.6);
        assert_eq!(all.len(), 1);
        assert!(close(all[0].start, 0.0) && close(all[0].end, 1.0));
        assert!(speech(&quiet, RATE, -35.0, 0.6).is_empty());
        assert_eq!(speech(&signal(&[(0.5, 0.3)]), RATE, -35.0, 0.6).len(), 1);
    }
}
