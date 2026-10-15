//! The sound library: sound effects (whooshes, pops, dings, risers, impacts, a shutter, typing…)
//! and music beds (a calm pad, an upbeat groove, lo-fi, tension), synthesised on this computer from
//! code — nothing is downloaded and they are ours to give: free for any use, no attribution
//! (CC0). A sound is rendered once to `<data dir>/sounds/<id>.wav` (beside the preferences; the
//! temporary directory for a session without them), imported into a Sounds bin and placed at the
//! playhead on a free audio track.
//!
//! | Command | |
//! |---|---|
//! | `sounds.list` | `{category?: "effects"\|"music"}`: id, name, category, seconds |
//! | `sounds.add` | `{id, time?}`: on a free audio track at the playhead (one undo step) |
//! | `sounds.render` | `{id}`: the WAV file only (its path) |

use std::f32::consts::TAU;

use filmcraft_project::{ItemId, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, has_seq, str_p};
use crate::{EngineError, Result, Session};

const RATE: u32 = 48_000;

/// (id, name, category, seconds)
pub const SOUNDS: [(&str, &str, &str, f32); 19] = [
    ("whoosh", "Whoosh", "effects", 0.9),
    ("swoosh-up", "Swoosh Up", "effects", 0.7),
    ("swoosh-down", "Swoosh Down", "effects", 0.7),
    ("pop", "Pop", "effects", 0.15),
    ("click", "Click", "effects", 0.05),
    ("ding", "Ding", "effects", 1.6),
    ("bell", "Bell", "effects", 2.6),
    ("riser", "Riser", "effects", 3.0),
    ("impact", "Impact", "effects", 1.4),
    ("shutter", "Camera Shutter", "effects", 0.3),
    ("typing", "Typing", "effects", 2.0),
    ("notification", "Notification", "effects", 0.7),
    ("success", "Success", "effects", 1.0),
    ("error", "Error", "effects", 0.6),
    ("heartbeat", "Heartbeat", "effects", 1.8),
    ("calm-pad", "Calm Pad", "music", 32.0),
    ("upbeat", "Upbeat Groove", "music", 32.0),
    ("lofi", "Lo-fi", "music", 33.9),
    ("tension", "Tension", "music", 24.0),
];

/// A small deterministic noise source.
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// A state-variable band-pass filter.
struct Band {
    low: f32,
    band: f32,
}
impl Band {
    fn new() -> Self {
        Self { low: 0.0, band: 0.0 }
    }
    /// One sample through the filter at `freq` Hz with damping `q` (smaller = narrower).
    fn run(&mut self, x: f32, freq: f32, q: f32) -> f32 {
        let f = (2.0 * (std::f32::consts::PI * freq.clamp(20.0, 18_000.0) / RATE as f32).sin()).min(1.0);
        let high = x - self.low - q * self.band;
        self.band += f * high;
        self.low += f * self.band;
        self.band
    }
}

fn n(seconds: f32) -> usize {
    (seconds * RATE as f32) as usize
}

fn t_of(i: usize) -> f32 {
    i as f32 / RATE as f32
}

/// A note's frequency (A4 = 69).
fn midi(m: f32) -> f32 {
    440.0 * 2f32.powf((m - 69.0) / 12.0)
}

/// Add a decaying sine (partials `(ratio, gain)`) at `at` seconds.
fn tone(out: &mut [f32], at: f32, freq: f32, partials: &[(f32, f32)], decay: f32, gain: f32, len: f32) {
    let s = n(at);
    for i in 0..n(len).min(out.len().saturating_sub(s)) {
        let t = t_of(i);
        let env = (t / 0.004).min(1.0) * (-t / decay).exp();
        let v: f32 = partials.iter().map(|(r, g)| g * (TAU * freq * r * t).sin()).sum();
        out[s + i] += v * env * gain;
    }
}

/// A drum hit at `at`: a sine falling from `f0` to `f1` and a noise burst.
fn drum(out: &mut [f32], noise: &mut Noise, at: f32, f0: f32, f1: f32, decay: f32, body: f32, snap: f32) {
    let s = n(at);
    let mut phase = 0.0f32;
    for i in 0..n(decay * 6.0).min(out.len().saturating_sub(s)) {
        let t = t_of(i);
        let f = f1 + (f0 - f1) * (-t / 0.03).exp();
        phase += TAU * f / RATE as f32;
        let env = (-t / decay).exp();
        out[s + i] += (phase.sin() * body + noise.next() * snap * (-t / (decay * 0.4)).exp()) * env;
    }
}

/// Mono samples of sound `id`.
fn synth(id: &str, seconds: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; n(seconds)];
    let mut noise = Noise(0x9E37_79B9);
    let len = out.len();
    match id {
        "whoosh" | "swoosh-up" | "swoosh-down" => {
            let mut b = Band::new();
            for (i, o) in out.iter_mut().enumerate() {
                let x = i as f32 / len as f32;
                let f = match id {
                    "swoosh-up" => 400.0 * (12.0f32).powf(x),
                    "swoosh-down" => 5000.0 / (12.0f32).powf(x),
                    _ => 300.0 + 2800.0 * (std::f32::consts::PI * x).sin(),
                };
                let env = (std::f32::consts::PI * x).sin().powf(1.6);
                *o = b.run(noise.next(), f, 0.35) * env * 1.4;
            }
        }
        "pop" => {
            let mut phase = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let t = t_of(i);
                phase += TAU * (200.0 + 900.0 * (-t / 0.012).exp()) / RATE as f32;
                *o = phase.sin() * (-t / 0.03).exp();
            }
        }
        "click" => {
            let mut b = Band::new();
            for (i, o) in out.iter_mut().enumerate() {
                *o = b.run(noise.next(), 4000.0, 0.5) * (-t_of(i) / 0.004).exp() * 2.0;
            }
        }
        "ding" => tone(&mut out, 0.0, 1320.0, &[(1.0, 1.0), (2.0, 0.35), (3.0, 0.15)], 0.45, 0.7, seconds),
        "bell" => tone(&mut out, 0.0, 523.25, &[(1.0, 1.0), (2.76, 0.5), (5.4, 0.25), (8.93, 0.12)], 0.8, 0.5, seconds),
        "riser" => {
            let mut b = Band::new();
            let mut phase = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let x = i as f32 / len as f32;
                phase += TAU * (200.0 + 1000.0 * x * x) / RATE as f32;
                let env = x.powf(2.0) * (1.0 - ((x - 0.97) / 0.03).clamp(0.0, 1.0));
                *o = (b.run(noise.next(), 300.0 + 6000.0 * x * x, 0.4) * 0.8 + phase.sin() * 0.35) * env;
            }
        }
        "impact" => {
            drum(&mut out, &mut noise, 0.0, 120.0, 38.0, 0.35, 1.0, 0.5);
            let mut b = Band::new();
            for (i, o) in out.iter_mut().enumerate() {
                *o += b.run(noise.next(), 180.0, 0.8) * (-t_of(i) / 0.25).exp() * 0.6;
            }
        }
        "shutter" => {
            for at in [0.0, 0.09] {
                let mut b = Band::new();
                for i in 0..n(0.05) {
                    out[n(at) + i] += b.run(noise.next(), 2500.0, 0.6) * (-t_of(i) / 0.008).exp() * 1.8;
                }
            }
        }
        "typing" => {
            let mut at = 0.02;
            while at < seconds - 0.06 {
                let mut b = Band::new();
                let f = 1800.0 + 1600.0 * noise.next().abs();
                for i in 0..n(0.03) {
                    out[n(at) + i] += b.run(noise.next(), f, 0.6) * (-t_of(i) / 0.005).exp() * 1.2;
                }
                at += 0.07 + 0.09 * noise.next().abs();
            }
        }
        "notification" => {
            tone(&mut out, 0.0, 880.0, &[(1.0, 1.0), (2.0, 0.2)], 0.15, 0.5, 0.4);
            tone(&mut out, 0.12, 1318.5, &[(1.0, 1.0), (2.0, 0.2)], 0.25, 0.5, 0.55);
        }
        "success" => {
            for (k, m) in [72.0, 76.0, 79.0, 84.0].iter().enumerate() {
                tone(&mut out, k as f32 * 0.09, midi(*m), &[(1.0, 1.0), (2.0, 0.25)], 0.3, 0.35, 0.7);
            }
        }
        "error" => {
            for (k, f) in [220.0, 196.0].iter().enumerate() {
                tone(&mut out, k as f32 * 0.2, *f, &[(1.0, 1.0), (3.0, 0.33), (5.0, 0.2)], 0.12, 0.5, 0.35);
            }
        }
        "heartbeat" => {
            for at in [0.0, 0.22, 0.9, 1.12] {
                drum(&mut out, &mut noise, at, 90.0, 45.0, 0.07, 1.0, 0.0);
            }
        }
        "calm-pad" => pad(&mut out, &[[57.0, 60.0, 64.0], [53.0, 57.0, 60.0], [48.0, 52.0, 55.0], [55.0, 59.0, 62.0]], 4.0, 0.5),
        "upbeat" => groove(&mut out, &mut noise, 120.0, &[45.0, 41.0, 36.0, 43.0], false),
        "lofi" => {
            groove(&mut out, &mut noise, 85.0, &[50.0, 43.0, 48.0, 45.0], true);
            keys(&mut out, 85.0, &[[62.0, 65.0, 69.0, 72.0], [55.0, 59.0, 62.0, 65.0], [60.0, 64.0, 67.0, 71.0], [57.0, 60.0, 64.0, 67.0]]);
        }
        "tension" => {
            // a low drone, a ticking clock and a slow rise
            for (i, o) in out.iter_mut().enumerate() {
                let t = t_of(i);
                let x = i as f32 / len as f32;
                *o = ((TAU * 55.0 * t).sin() * 0.4 + (TAU * 55.6 * t).sin() * 0.3 + (TAU * (110.0 + 30.0 * x) * t).sin() * 0.12 * x) * (t / 2.0).min(1.0);
            }
            let mut at = 0.0;
            while at < seconds - 0.1 {
                let mut b = Band::new();
                for i in 0..n(0.02) {
                    out[n(at) + i] += b.run(noise.next(), 3000.0, 0.5) * (-t_of(i) / 0.004).exp() * 0.8;
                }
                at += 0.5;
            }
        }
        _ => {}
    }
    out
}

/// Chords of soft detuned saws, each `bar` seconds, crossfaded.
fn pad(out: &mut [f32], chords: &[[f32; 3]], bar: f32, gain: f32) {
    let len = out.len();
    let mut lp = 0.0f32;
    for (i, o) in out.iter_mut().enumerate() {
        let t = t_of(i);
        let k = ((t / bar) as usize) % chords.len();
        let within = t % bar;
        let fade = (within / 0.8).min(1.0) * ((bar - within) / 0.8).min(1.0);
        let mut v = 0.0;
        for m in chords[k] {
            for d in [-0.08f32, 0.0, 0.08] {
                let f = midi(m + d);
                v += 2.0 * ((f * t) % 1.0) - 1.0;
            }
        }
        lp += 0.03 * (v - lp);
        *o += lp * fade * gain * 0.12 * ((i as f32 / len as f32) * 40.0).min(1.0);
    }
}

/// Drums and bass at `bpm` over a four-bar root progression (swing for lo-fi).
fn groove(out: &mut [f32], noise: &mut Noise, bpm: f32, roots: &[f32], swing: bool) {
    let beat = 60.0 / bpm;
    let total = out.len() as f32 / RATE as f32;
    let mut b = 0usize;
    while (b as f32) * beat < total - beat {
        let at = b as f32 * beat;
        let in_bar = b % 4;
        if in_bar == 0 || in_bar == 2 || (!swing && in_bar == 3 && b % 8 == 7) {
            drum(out, noise, at, 150.0, 48.0, 0.12, 0.9, 0.1);
        }
        if in_bar == 1 || in_bar == 3 {
            drum(out, noise, at, 220.0, 180.0, 0.06, 0.25, 0.55);
        }
        for half in [0.0, 0.5] {
            let off = if swing && half > 0.0 { 0.62 } else { half };
            let s = n(at + off * beat);
            let mut bp = Band::new();
            for i in 0..n(0.04).min(out.len().saturating_sub(s)) {
                out[s + i] += bp.run(noise.next(), 8000.0, 0.4) * (-t_of(i) / 0.012).exp() * 0.25;
            }
        }
        // bass: the bar's root on every beat
        let root = roots[(b / 4) % roots.len()];
        tone(out, at, midi(root), &[(1.0, 1.0), (2.0, 0.3)], beat * 0.6, 0.35, beat);
        b += 1;
    }
}

/// Soft electric-piano chords, one a bar.
fn keys(out: &mut [f32], bpm: f32, chords: &[[f32; 4]]) {
    let bar = 4.0 * 60.0 / bpm;
    let total = out.len() as f32 / RATE as f32;
    let mut k = 0usize;
    while (k as f32) * bar < total - 0.5 {
        for m in chords[k % chords.len()] {
            tone(out, k as f32 * bar, midi(m), &[(1.0, 1.0), (2.0, 0.1)], 1.2, 0.09, bar);
        }
        k += 1;
    }
}

/// Stereo interleaved with a short fade out: effects peak-normalised to −3 dB; music brought to an
/// RMS of −19 dBFS (about −18 LUFS, a bed under speech) through a soft limiter at −1 dB.
pub fn render(id: &str) -> Option<Vec<f32>> {
    let (_, _, cat, seconds) = SOUNDS.iter().find(|s| s.0 == id)?;
    let mut mono = synth(id, *seconds);
    let fade = n(0.03).min(mono.len());
    let len = mono.len();
    let gain = if *cat == "music" {
        let rms = (mono.iter().map(|v| v * v).sum::<f32>() / len.max(1) as f32).sqrt().max(1e-6);
        0.112 / rms
    } else {
        0.708 / mono.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6)
    };
    for (i, v) in mono.iter_mut().enumerate() {
        let x = *v * gain;
        let x = if *cat == "music" { 0.89 * (x / 0.89).tanh() } else { x };
        *v = x * ((len - i) as f32 / fade as f32).min(1.0);
    }
    Some(mono.iter().flat_map(|v| [*v, *v]).collect())
}

fn sounds_dir(s: &Session) -> std::path::PathBuf {
    s.prefs_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("sounds"))
        .unwrap_or_else(|| crate::temp_dir().join(format!("jilo-sounds-{}", std::process::id())))
}

/// The WAV of sound `id`, rendered once and kept.
pub fn path_of(s: &Session, id: &str) -> Result<std::path::PathBuf> {
    let cmd = "sounds.add";
    let dir = sounds_dir(s);
    let path = dir.join(format!("{id}.wav"));
    if path.is_file() {
        return Ok(path);
    }
    let pcm = render(id).ok_or_else(|| bad(cmd, format!("no sound `{id}` (sounds.list)")))?;
    let wav = filmcraft_export::write_wav(&pcm, 2, RATE, 16).map_err(EngineError::Other)?;
    std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::write(&path, wav).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(path)
}

fn list(_: &mut Session, p: &Value) -> Result<Value> {
    let cat = str_p(p, "category");
    Ok(json!({
        "sounds": SOUNDS.iter().filter(|x| cat.is_none_or(|c| c == x.2)).map(|(id, name, c, sec)| json!({"id": id, "name": name, "category": c, "seconds": sec})).collect::<Vec<_>>(),
        "license": "CC0 (synthesised by Jilo Studio)",
    }))
}

fn render_cmd(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "id").ok_or_else(|| bad("sounds.render", "`id`"))?;
    Ok(json!({"path": path_of(s, id)?.to_string_lossy()}))
}

/// The Sounds bin, made if needed.
fn bin(s: &mut Session) -> Option<u64> {
    let find = |s: &Session| {
        s.project.root.children.iter().find_map(|e| match e {
            filmcraft_project::BinEntry::Bin(b) if b.name == "Sounds" => Some(b.id.0),
            _ => None,
        })
    };
    if find(s).is_none() {
        let _ = s.execute("file.newBin", json!({"name": "Sounds"}));
    }
    find(s)
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sounds.add";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "`id`: a sound of sounds.list"))?.to_string();
    let path = path_of(s, &id)?;
    let at = p.get("time").and_then(Value::as_i64).map(Tick).unwrap_or(s.playhead());
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let r = (|| -> Result<Value> {
        let b = bin(s);
        let r = s.execute("file.import", json!({"paths": [path.to_string_lossy()], "bin": b}))?;
        let item = r["items"][0]
            .as_u64()
            .or_else(|| r["duplicates"][0]["item"].as_u64())
            .map(ItemId)
            .ok_or_else(|| EngineError::Other("the sound could not be imported".into()))?;
        let dur = s.project.item(item).map(|i| i.duration()).filter(|d| *d > Tick::ZERO).unwrap_or(Tick::from_seconds_f64(1.0));
        let track = crate::paste_media::free_track(s, TrackKind::Audio, at, at + dur)?;
        let placed = s.execute("timeline.place", json!({"item": item.0, "track": track.0, "time": at.0, "video": false}))?;
        Ok(json!({"item": item.0, "clips": placed["clips"], "path": path.to_string_lossy()}))
    })();
    match r {
        Ok(v) => {
            crate::recipes::collapse_undo(s, before, "Add Sound");
            Ok(v)
        }
        Err(e) => {
            s.revert_to(mark);
            Err(e)
        }
    }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "sounds.list",
            label: "List Sounds",
            menu: &[],
            shortcut: None,
            params: r#"{"category":"effects|music"?}"#,
            enabled: always,
            run: list,
            journal: false,
        },
        CommandSpec {
            id: "sounds.add",
            label: "Add Sound",
            menu: &[],
            shortcut: None,
            params: r#"{"id":str,"time":ticks?}"#,
            enabled: has_seq,
            run: add,
            journal: true,
        },
        CommandSpec {
            id: "sounds.render",
            label: "Render Sound",
            menu: &[],
            shortcut: None,
            params: r#"{"id":str}"#,
            enabled: always,
            run: render_cmd,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_renders_audible_and_clean() {
        for (id, _, cat, seconds) in SOUNDS {
            let pcm = render(id).unwrap();
            assert_eq!(pcm.len(), n(seconds) * 2, "{id}");
            let peak = pcm.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(pcm.iter().all(|v| v.is_finite()), "{id}");
            if cat == "music" {
                assert!(peak <= 0.9, "{id}: peak {peak}");
            } else {
                assert!((peak - 0.708).abs() < 0.01, "{id}: peak {peak}");
            }
            let rms = (pcm.iter().map(|v| v * v).sum::<f32>() / pcm.len() as f32).sqrt();
            assert!(rms > 0.003, "{id} is audible: rms {rms}");
        }
    }

    #[test]
    fn a_sound_goes_on_a_free_audio_track() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let undo = s.history.undo.len();
        let r = s.execute("sounds.add", json!({"id": "whoosh"})).unwrap();
        assert!(r["clips"].as_array().is_some_and(|c| !c.is_empty()), "{r}");
        assert_eq!(s.history.undo.len(), undo + 1);
        let path = std::path::PathBuf::from(r["path"].as_str().unwrap());
        assert!(path.is_file());
        assert!(s.execute("sounds.add", json!({"id": "nope"})).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
