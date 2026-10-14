//! Sound on autopilot.
//!
//! | Command | |
//! |---|---|
//! | `audio.autoMix` | one click: audio types for untyped clips (speech → Dialogue, music → Music), speech brought to one loudness, music ducked under speech, and the mix set to a platform's loudness (YouTube −14 LUFS, Apple Podcasts −16, EBU R128 −23…) on the Mix fader. One undo step |
//! | `audio.beats` | the beats of a music clip (tempo and times) |
//! | `audio.beatMarkers` | a marker on every `every`-th beat of the music |
//! | `sequence.cutToBeat` | the cuts between the selected clips (one track) roll to the nearest beat of the music (of every `every`-th beat), so the edit keeps its length and every cut lands on the beat. One undo step |
//!
//! The music is the `clip` given, else the selected audio clip, else a Music clip, else a clip on
//! a track named Music, else the longest audio clip without a transcript.

use serde_json::{Value, json};

use filmcraft_audio_dsp::LoudnessMeter;
use filmcraft_project::essential::AudioType;
use filmcraft_project::{ClipId, TrackItem};
use filmcraft_time::Tick;

use crate::commands::{CommandSpec, bad, bool_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Delivery targets: (id, name, integrated loudness LUFS, highest true peak dBTP).
pub const PLATFORMS: [(&str, &str, f64, f64); 6] = [
    ("youtube", "YouTube", -14.0, -1.0),
    ("social", "Shorts, Reels, TikTok", -14.0, -1.0),
    ("spotify", "Spotify", -14.0, -1.0),
    ("podcast", "Apple Podcasts", -16.0, -1.0),
    ("ebu", "Broadcast (EBU R128)", -23.0, -1.0),
    ("atsc", "Broadcast (ATSC A/85)", -24.0, -2.0),
];

/// Longest music analysed for beats, in seconds.
const BEATS_MAX_SECONDS: f64 = 20.0 * 60.0;

/// Rate the music is analysed at.
const BEAT_RATE: u32 = 22_050;

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// The audio type an untyped clip gets: speech (it has a transcript, or its track is named for a
/// voice) is Dialogue; music (its track is named for music, or a long sound-only file without
/// speech) is Music; anything else stays untyped.
fn guess_type(s: &Session, track_name: &str, it: &TrackItem) -> Option<AudioType> {
    let t = lower(track_name);
    let item = s.project.item(it.item)?;
    let speech = s.project.transcripts.get(&it.item).is_some_and(|tr| !tr.words.is_empty());
    if speech || ["voice", "dialog", "mic", "host", "guest", "голос", "речь", "диктор"].iter().any(|w| t.contains(w)) {
        return Some(AudioType::Dialogue);
    }
    if ["music", "score", "song", "музык", "песн"].iter().any(|w| t.contains(w)) || (!item.has_video() && it.duration.seconds() >= 20.0) {
        return Some(AudioType::Music);
    }
    None
}

/// Integrated loudness (LUFS) and sample peak (dBFS) of the sequence mix.
pub fn mix_loudness(s: &Session) -> Result<(f64, f64)> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let sr = q.settings.sample_rate.max(1);
    let total = q.duration().to_units_floor(sr as i64);
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let mut meter = LoudnessMeter::new(f64::from(sr), 2);
    let mut peak = 0f32;
    let mut pos = 0i64;
    while pos < total {
        let n = (total - pos).min(sr as i64) as usize;
        let b = filmcraft_render::audio::mix_sequence(&s.project, q, pos, n, &provider);
        peak = b.peaks().into_iter().fold(peak, f32::max);
        if b.channels.len() >= 2 {
            meter.process(&[&b.channels[0], &b.channels[1]]);
        }
        pos += n as i64;
    }
    let peak_db = if peak > 0.0 { 20.0 * f64::from(peak).log10() } else { f64::NEG_INFINITY };
    Ok((meter.integrated(), peak_db))
}

fn auto_mix(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "audio.autoMix";
    let platform = str_p(p, "platform").unwrap_or("youtube");
    let &(pid, _, target, max_peak) =
        PLATFORMS.iter().find(|x| x.0 == platform).ok_or_else(|| bad(cmd, format!("`platform`: {}", PLATFORMS.map(|x| x.0).join(", "))))?;
    let step = |k: &str| bool_p(p, k).unwrap_or(true);
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let selection = s.state.selection.clone();
    let r = (|| -> Result<Value> {
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let mut out = json!({"platform": pid, "targetLufs": target});
        // 1. audio types for the clips that have none
        if step("tag") {
            let mut by: Vec<(AudioType, Vec<u64>)> = vec![(AudioType::Dialogue, Vec::new()), (AudioType::Music, Vec::new())];
            for t in &q.audio_tracks {
                for it in t.items.iter().filter(|i| i.essential.is_none()) {
                    if let Some(k) = guess_type(s, &t.name, it)
                        && let Some(e) = by.iter_mut().find(|e| e.0 == k)
                    {
                        e.1.push(it.id.0);
                    }
                }
            }
            for (k, clips) in &by {
                if !clips.is_empty() {
                    let name = if *k == AudioType::Dialogue { "dialogue" } else { "music" };
                    s.execute("essentialSound.setType", json!({"clips": clips, "type": name}))?;
                }
            }
            out["tagged"] = json!({"dialogue": by[0].1.len(), "music": by[1].1.len()});
        }
        let typed = |s: &Session, k: AudioType| -> Vec<u64> {
            s.active_sequence()
                .map(|q| {
                    q.audio_tracks.iter().flat_map(|t| t.items.iter()).filter(|i| i.essential.as_ref().is_some_and(|e| e.kind == k)).map(|i| i.id.0).collect()
                })
                .unwrap_or_default()
        };
        // 2. speech at one loudness
        let speech = typed(s, AudioType::Dialogue);
        if step("match") && !speech.is_empty() {
            let r = s.execute("essentialSound.autoMatch", json!({"clips": speech}));
            out["matched"] = json!(r.map(|v| v["clips"].as_array().map_or(0, Vec::len)).unwrap_or(0));
        }
        // 3. music under speech
        let music = typed(s, AudioType::Music);
        if step("duck") && !music.is_empty() && !speech.is_empty() {
            let r = s.execute("essentialSound.generateDucking", json!({"clips": music}));
            out["ducked"] = json!(r.map(|v| v["clips"].as_array().map_or(0, Vec::len)).unwrap_or(0));
        }
        // 4. the mix at the platform's loudness, on the Mix fader
        if step("normalize") {
            let (lufs, peak) = mix_loudness(s)?;
            if lufs.is_finite() {
                let gain = filmcraft_audio_dsp::loudness::normalize_gain_db_peak_limited(lufs, target, peak, max_peak).clamp(-30.0, 30.0);
                s.edit_sequence("Mix Loudness", |q, _, _| {
                    q.master_volume_db = (q.master_volume_db + gain).clamp(-96.0, 48.0);
                    Ok(())
                })?;
                out["measuredLufs"] = json!(lufs);
                out["gainDb"] = json!(gain);
            } else {
                out["measuredLufs"] = Value::Null;
            }
        }
        Ok(out)
    })();
    s.state.selection = selection;
    match r {
        Ok(v) => {
            crate::recipes::collapse_undo(s, before, "Auto Mix");
            Ok(v)
        }
        Err(e) => {
            s.revert_to(mark);
            Err(e)
        }
    }
}

/// The music clip: see the module notes.
fn music_clip(s: &Session, p: &Value, exclude: &[ClipId]) -> Option<TrackItem> {
    let q = s.active_sequence()?;
    let all: Vec<(&str, &TrackItem)> =
        q.audio_tracks.iter().flat_map(|t| t.items.iter().map(move |i| (t.name.as_str(), i))).filter(|x| !exclude.contains(&x.1.id)).collect();
    if let Some(c) = u64_p(p, "clip") {
        return all.iter().find(|x| x.1.id.0 == c).map(|x| x.1.clone());
    }
    let pick = all
        .iter()
        .find(|x| s.state.selection.contains(&x.1.id))
        .or_else(|| all.iter().find(|x| x.1.essential.as_ref().is_some_and(|e| e.kind == AudioType::Music)))
        .or_else(|| all.iter().find(|x| ["music", "музык", "score"].iter().any(|w| lower(x.0).contains(w))))
        .or_else(|| all.iter().filter(|x| !s.project.transcripts.contains_key(&x.1.item)).max_by_key(|x| x.1.duration));
    pick.map(|x| x.1.clone())
}

/// The beats of `it` in sequence time, and the tempo.
fn beats_of(s: &Session, it: &TrackItem) -> Result<(Vec<Tick>, f64)> {
    if it.duration.seconds() > BEATS_MAX_SECONDS {
        return Err(EngineError::Other("the music is longer than 20 minutes".into()));
    }
    let provider = s.media.full_res_provider(s.project.clone(), s.services.clone());
    let (a0, a1) = (it.start.to_units_floor(i64::from(BEAT_RATE)), it.end().to_units_floor(i64::from(BEAT_RATE)));
    let [l, r] = filmcraft_render::audio::clip_signal(it, a0, (a1 - a0).max(0) as usize, BEAT_RATE, &provider)
        .ok_or_else(|| EngineError::Other("the music clip has no sound".into()))?;
    let a = filmcraft_audio_dsp::remix::analyze(&[&l, &r], BEAT_RATE).map_err(|e| EngineError::Other(format!("no beat found: {e:?}")))?;
    let beats = a.beats.iter().map(|b| it.start + Tick::from_seconds_f64(*b as f64 / f64::from(BEAT_RATE))).filter(|t| *t < it.end()).collect();
    Ok((beats, a.bpm))
}

fn beats(s: &mut Session, p: &Value) -> Result<Value> {
    let it = music_clip(s, p, &[]).ok_or_else(|| bad("audio.beats", "no music clip in the sequence"))?;
    let (b, bpm) = beats_of(s, &it)?;
    Ok(json!({"clip": it.id.0, "bpm": bpm, "beats": b.iter().map(|t| t.0).collect::<Vec<_>>(), "seconds": b.iter().map(|t| t.seconds()).collect::<Vec<_>>()}))
}

fn beat_markers(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "audio.beatMarkers";
    let it = music_clip(s, p, &[]).ok_or_else(|| bad(cmd, "no music clip in the sequence"))?;
    let every = u64_p(p, "every").unwrap_or(4).clamp(1, 64) as usize;
    let (b, bpm) = beats_of(s, &it)?;
    let marks: Vec<Tick> = b.into_iter().step_by(every).collect();
    let n = marks.len();
    let rate = s.active_sequence().ok_or(EngineError::NoSequence)?.settings.frame_rate;
    s.edit_sequence("Beat Markers", |q, ctx, _| {
        for t in marks {
            let t = rate.snap_nearest(t);
            if q.markers.iter().any(|m| m.start == t) {
                continue;
            }
            q.markers.push(filmcraft_project::Marker {
                id: filmcraft_project::MarkerId(ctx.alloc()),
                start: t,
                duration: Tick::ZERO,
                name: "Beat".into(),
                comment: String::new(),
                kind: filmcraft_project::MarkerKind::Comment,
                color: filmcraft_project::Label::Cerulean,
            });
        }
        q.markers.sort_by_key(|m| m.start);
        Ok(())
    })?;
    Ok(json!({"clip": it.id.0, "bpm": bpm, "markers": n}))
}

/// Where each cut goes: the beat of `grid` nearest to it, kept in order (a cut never passes the
/// one before it) and inside its two clips (`bounds`: the left clip's start and the right clip's
/// end). None: no beat fits.
pub fn beat_targets(cuts: &[(Tick, Tick, Tick)], grid: &[Tick]) -> Vec<Option<Tick>> {
    let mut last = Tick::MIN;
    cuts.iter()
        .map(|&(cut, lo, hi)| {
            let t = grid.iter().copied().filter(|b| *b > lo && *b < hi && *b > last).min_by_key(|b| (*b - cut).0.abs())?;
            last = t;
            Some(t)
        })
        .collect()
}

fn cut_to_beat(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sequence.cutToBeat";
    let every = u64_p(p, "every").unwrap_or(1).clamp(1, 64) as usize;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let rate = q.settings.frame_rate;
    let wanted: Vec<ClipId> = match p.get("clips").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(ClipId).collect(),
        None => s.state.selection.clone(),
    };
    // the music: not one of the clips being cut (nor their linked sound)
    let linked = crate::commands::with_links(s, &wanted);
    let music =
        music_clip(s, &json!({"clip": p.get("music").cloned().unwrap_or(Value::Null)}), &linked).ok_or_else(|| bad(cmd, "no music clip in the sequence"))?;
    let track = q
        .video_tracks
        .iter()
        .chain(q.audio_tracks.iter())
        .find(|t| t.items.iter().any(|i| wanted.contains(&i.id) && i.id != music.id))
        .ok_or_else(|| bad(cmd, "select the clips to cut to the beat"))?;
    let mut clips: Vec<&TrackItem> = track.items.iter().filter(|i| wanted.contains(&i.id)).collect();
    clips.sort_by_key(|i| i.start);
    // the cuts between selected clips that touch
    let pairs: Vec<(ClipId, ClipId, Tick, Tick, Tick)> =
        clips.windows(2).filter(|w| w[0].end() == w[1].start).map(|w| (w[0].id, w[1].id, w[0].end(), w[0].start, w[1].end())).collect();
    if pairs.is_empty() {
        return Err(bad(cmd, "select two or more clips that follow each other"));
    }
    let (beats, bpm) = beats_of(s, &music)?;
    let grid: Vec<Tick> = beats.iter().step_by(every).map(|b| rate.snap_nearest(*b)).collect();
    let targets = beat_targets(&pairs.iter().map(|x| (x.2, x.3, x.4)).collect::<Vec<_>>(), &grid);
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let (mut moved, mut kept) = (0, 0);
    for ((l, r, cut, _, _), t) in pairs.iter().zip(targets) {
        let Some(t) = t else {
            kept += 1;
            continue;
        };
        if t == *cut {
            continue;
        }
        // a cut that cannot roll so far (no media past the clip's end) stays where it is
        match s.execute("timeline.roll", json!({"left": l.0, "right": r.0, "delta": (t - *cut).0})) {
            Ok(_) => moved += 1,
            Err(_) => kept += 1,
        }
    }
    crate::recipes::collapse_undo(s, before, "Cut to the Beat");
    Ok(json!({"bpm": bpm, "moved": moved, "kept": kept, "cuts": pairs.len()}))
}

pub fn commands() -> Vec<CommandSpec> {
    let spec = |id, label, menu, params, run, journal| CommandSpec { id, label, menu, shortcut: None, params, enabled: has_seq, run, journal };
    vec![
        spec(
            "audio.autoMix",
            "Auto Mix…",
            crate::ai_menu::SOUND,
            r#"{"platform":"youtube|social|spotify|podcast|ebu|atsc"="youtube","tag":bool=true,"match":bool=true,"duck":bool=true,"normalize":bool=true}"#,
            auto_mix as fn(&mut Session, &Value) -> Result<Value>,
            true,
        ),
        spec("audio.beats", "Find Beats", &[], r#"{"clip":id?}"#, beats, false),
        spec("audio.beatMarkers", "Mark Beats", &[], r#"{"clip":id?,"every":n=4}"#, beat_markers, true),
        spec("sequence.cutToBeat", "Cut to the Beat", &[], r#"{"clips":[id]?,"every":n=1,"music":id?}"#, cut_to_beat, true),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_cut_goes_to_the_nearest_beat_in_order() {
        let t = Tick::from_seconds_f64;
        let grid: Vec<Tick> = (0..20).map(|k| t(0.5 * k as f64)).collect();
        // cuts at 1.2 s and 1.3 s: the first takes 1.0, the second the next beat after it (1.5)
        let cuts = [(t(1.2), t(0.0), t(1.3)), (t(1.3), t(1.2), t(4.0))];
        assert_eq!(beat_targets(&cuts, &grid), [Some(t(1.0)), Some(t(1.5))]);
        // no beat strictly inside the clips: the cut stays
        assert_eq!(beat_targets(&[(t(1.2), t(1.1), t(1.4))], &grid), [None]);
    }

    #[test]
    fn a_music_bed_is_found_and_its_beats_marked() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        // the demo's Ambient_Score on A2: the longest clip without speech (it has no beat)
        let m = music_clip(&s, &json!({}), &[]).unwrap();
        assert!(s.project.item(m.item).unwrap().name.starts_with("Ambient_Score"));
        assert!(s.execute("audio.beats", json!({})).is_err(), "a pad without drums has no beat");
    }

    /// 120 BPM music on its own track: a beat every 0.5 s from its start.
    fn with_music(s: &mut Session, at: f64) -> u64 {
        let sr = 48_000;
        let (ch, _) = filmcraft_audio_dsp::remix::test_music(sr, 120.0, 12, 7);
        let pcm: Vec<f32> = ch[0].iter().zip(&ch[1]).flat_map(|(l, r)| [*l, *r]).collect();
        let item = crate::commands::import_bytes(s, "/beat.wav", crate::previews::write_wav_f32(&pcm, sr).into(), None).unwrap();
        s.execute("sequence.addTracks", json!({"video": 0, "audio": 1})).unwrap();
        let n = s.active_sequence().unwrap().audio_tracks.len();
        let r = s.execute("timeline.place", json!({"item": item.0, "audioTrack": format!("A{n}"), "seconds": at})).unwrap();
        r["clips"][0].as_u64().unwrap()
    }

    #[test]
    fn beats_are_marked_and_clips_cut_on_them() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let music = with_music(&mut s, 0.0);
        let r = s.execute("audio.beats", json!({"clip": music})).unwrap();
        assert!((r["bpm"].as_f64().unwrap() - 120.0).abs() < 3.0, "{}", r["bpm"]);
        let secs: Vec<f64> = r["seconds"].as_array().unwrap().iter().filter_map(Value::as_f64).collect();
        assert!(secs.windows(2).all(|w| (w[1] - w[0] - 0.5).abs() < 0.05), "a beat every half second");
        let before = s.active_sequence().unwrap().markers.len();
        s.execute("audio.beatMarkers", json!({"clip": music, "every": 4})).unwrap();
        assert_eq!(s.active_sequence().unwrap().markers.len(), before + secs.len().div_ceil(4));
        // the first four clips of V1 (cut to cut): every cut between them rolls to a beat
        let q = s.active_sequence().unwrap();
        let clips: Vec<u64> = q.video_tracks[0].items.iter().take(4).map(|i| i.id.0).collect();
        let total = q.duration();
        let undo = s.history.undo.len();
        let r = s.execute("sequence.cutToBeat", json!({"clips": clips, "music": music})).unwrap();
        assert!(r["moved"].as_u64().unwrap() >= 1, "{r}");
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step: {r}");
        let q = s.active_sequence().unwrap();
        assert_eq!(q.duration(), total, "the edit keeps its length");
        let rate = q.settings.frame_rate;
        let on_beat = |t: f64| secs.iter().any(|b| (b - t).abs() <= rate.frame_duration().seconds() + 0.03);
        let moved: Vec<f64> = clips
            .windows(2)
            .filter_map(|w| {
                let (l, r) = (q.find_item(ClipId(w[0])).unwrap().1, q.find_item(ClipId(w[1])).unwrap().1);
                (l.end() == r.start).then(|| l.end().seconds())
            })
            .collect();
        assert_eq!(moved.iter().filter(|t| on_beat(**t)).count() as u64, r["moved"].as_u64().unwrap(), "{moved:?} {r}");
    }

    #[test]
    fn auto_mix_types_clips_and_sets_the_mix_loudness() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let undo = s.history.undo.len();
        let r = s.execute("audio.autoMix", json!({"platform": "podcast"})).unwrap();
        assert_eq!(r["targetLufs"], -16.0);
        assert_eq!(r["tagged"]["music"], 1, "{r}");
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
        let q = s.active_sequence().unwrap();
        assert!(q.audio_tracks[1].items[0].essential.as_ref().is_some_and(|e| e.kind == AudioType::Music));
        let (lufs, peak) = mix_loudness(&s).unwrap();
        // at the target, unless the peak limit held it lower
        assert!((lufs - -16.0).abs() < 0.6 || peak > -1.6, "{lufs} LUFS, peak {peak}");
        assert!(s.execute("audio.autoMix", json!({"platform": "radio"})).is_err());
    }
}
