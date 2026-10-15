//! Dubbing (`audio.dub`): a caption track — usually the translation `captions.translate` made —
//! spoken phrase by phrase in a voice of Text to Speech (the natural voices on this computer, or
//! ElevenLabs in the cloud for other languages), each phrase at its caption's time on a new audio
//! track, a little faster when it would not fit before the next one (and cut there when it still
//! does not), and the original voice turned down under it (or muted). One undo step. With
//! `background: true` the phrases are spoken in a job (progress, Stop) and the same call
//! afterwards puts them in.

use filmcraft_project::{ClipId, TrackId};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, has_seq, str_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "audio.dub";

/// Characters spoken in a second at pace 1 (an average across languages).
const CHARS_PER_SECOND: f64 = 15.0;

/// One phrase: `tts.create` parameters and the time it may last.
fn plan(s: &Session, p: &Value) -> Result<(TrackId, Vec<(Value, Tick)>)> {
    let which = p.get("captions").cloned().unwrap_or(Value::Null);
    let tid =
        crate::captions::track_param(s, &json!({"track": which})).ok_or_else(|| bad(CMD, "`captions`: the caption track to speak (C1, C2… or its id)"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let t = q.caption_track(tid).ok_or_else(|| bad(CMD, "no such caption track"))?;
    if t.captions.is_empty() {
        return Err(bad(CMD, "the caption track has no captions"));
    }
    let voice = str_p(p, "voice");
    let mut out = Vec::new();
    for (i, c) in t.captions.iter().enumerate() {
        let text = c.plain_lines().join(" ");
        if text.trim().is_empty() {
            continue;
        }
        let next = t.captions.get(i + 1).map_or(c.end() + Tick::from_seconds_f64(1.0), |n| n.start);
        let slot = (next - c.start).max(c.duration);
        let need = text.chars().count() as f64 / CHARS_PER_SECOND;
        let pace = (need / slot.seconds().max(0.2)).clamp(1.0, 1.6);
        let mut q = json!({"text": text, "pace": (pace * 100.0).round() / 100.0, "time": c.start.0});
        if let Some(v) = voice {
            q["voice"] = json!(v);
        }
        for k in ["confirm", "dir"] {
            if let Some(v) = p.get(k) {
                q[k] = v.clone();
            }
        }
        out.push((q, slot));
    }
    Ok((tid, out))
}

fn dub(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, phrases) = plan(s, p)?;
    if bool_p(p, "background").unwrap_or(false) {
        let list: Vec<Value> = phrases.iter().map(|x| x.0.clone()).collect();
        if let Some(job) = crate::narration::render_many(s, "tts.create", &list, "Dubbing: speaking the phrases", false)? {
            return Ok(json!({"job": job, "background": true}));
        }
    }
    let duck = f64_p(p, "originalDb").unwrap_or(-20.0).clamp(-96.0, 0.0);
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let selection = s.state.selection.clone();
    let r = (|| -> Result<Value> {
        // the original speech: audio clips of transcribed media under the captions
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let (from, to) = (phrases.first().map_or(Tick::ZERO, |x| Tick(x.0["time"].as_i64().unwrap_or(0))), q.duration());
        let original: Vec<u64> = q
            .audio_tracks
            .iter()
            .flat_map(|t| t.items.iter())
            .filter(|it| it.end() > from && it.start < to && s.project.transcripts.contains_key(&it.item))
            .map(|it| it.id.0)
            .collect();
        let tracks: Vec<TrackId> = q.audio_tracks.iter().map(|t| t.id).collect();
        s.execute("sequence.addTracks", json!({"video": 0, "audio": 1, "audioAfter": "last"}))?;
        let dub_track = s
            .active_sequence()
            .and_then(|q| q.audio_tracks.iter().map(|t| t.id).find(|id| !tracks.contains(id)))
            .ok_or_else(|| EngineError::Other("could not add an audio track".into()))?;
        let lang = str_p(p, "language").map_or_else(|| "Dub".to_string(), |l| format!("Dub {l}"));
        let _ = s.execute("timeline.setTrack", json!({"track": dub_track.0, "name": lang}));
        let mut clips = Vec::new();
        let mut cut = 0;
        for (q, slot) in &phrases {
            let mut q = q.clone();
            q["track"] = json!(dub_track.0);
            let r = s.execute("tts.create", q)?;
            let Some(c) = r["clip"].as_u64().map(ClipId) else { continue };
            let dur = s.active_sequence().and_then(|q| q.find_item(c)).map(|x| x.1.duration).unwrap_or_default();
            if dur > *slot {
                s.execute("timeline.trim", json!({"clip": c.0, "edge": "out", "delta": (*slot - dur).0}))?;
                cut += 1;
            }
            clips.push(c.0);
        }
        if !original.is_empty() {
            s.execute("clip.audioGain", json!({"clips": original, "mode": "adjust", "db": duck}))?;
        }
        Ok(json!({"track": dub_track.0, "clips": clips, "phrases": phrases.len(), "shortened": cut, "originalClips": original.len()}))
    })();
    match r {
        Ok(v) => {
            crate::recipes::collapse_undo(s, before, "Dub");
            s.state.selection = selection;
            Ok(v)
        }
        Err(e) => {
            s.revert_to(mark);
            s.state.selection = selection;
            Err(e)
        }
    }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Dub",
        menu: &[],
        shortcut: None,
        params: r#"{"captions":"C2"|id (the translated captions),"voice":str? (tts.voices),"language":str? (names the track),"originalDb":f64=-20 (-96: muted),"confirm":bool? (cloud voices),"background":bool?,"dir":path?}"#,
        enabled: has_seq,
        run: dub,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_are_spoken_on_a_new_track_and_the_original_turned_down() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.execute("captions.newTrack", json!({})).unwrap();
        for (t, text) in [(1.0, "Hello there."), (4.0, "This is a much longer phrase that will not fit in its short time at all, really.")] {
            s.set_playhead(Tick::from_seconds_f64(t));
            s.execute("captions.add", json!({"text": text, "durationSeconds": 1.5})).unwrap();
        }
        let (_, phrases) = plan(&s, &json!({"captions": "C1"})).unwrap();
        assert_eq!(phrases.len(), 2);
        assert_eq!(phrases[0].0["pace"], 1.0, "a short phrase keeps its pace");
        assert!(phrases[1].0["pace"].as_f64().unwrap() > 1.0, "a long one is faster");
        assert!(s.execute(CMD, json!({"captions": "C9"})).is_err());
        // without voices installed the speaking fails and nothing is left behind
        let tracks = s.active_sequence().unwrap().audio_tracks.len();
        let undo = s.history.undo.len();
        if s.execute(CMD, json!({"captions": "C1", "voice": "no-such-voice"})).is_err() {
            assert_eq!(s.active_sequence().unwrap().audio_tracks.len(), tracks, "reverted");
            assert_eq!(s.history.undo.len(), undo);
        }
    }
}
