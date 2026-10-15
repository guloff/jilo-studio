//! Background removal in video (`clip.removeBackground`): every frame the selected clip shows is
//! matted on this computer (Apple Vision's foreground mask, installed by the host as
//! [`Session::matte`]) into a numbered PNG sequence with alpha beside the project's media
//! (Scratch Disks ▸ Captured, else next to the project, in "Background Removed"), and the clip
//! then shows it in place: same place, length, effects and linked sound, one undo step. The
//! frames are made in a job (progress, Stop); with `background: true` the command returns the job
//! and the same call afterwards puts the result in.

use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};

use filmcraft_project::{ClipId, ItemId};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, has_seq, u64_p};
use crate::{EngineError, Result, Session};

/// The picture with its background made transparent, at its own size (straight RGBA).
pub type Matte = Arc<dyn Fn(&[u8], usize, usize) -> std::result::Result<Vec<u8>, String> + Send + Sync>;

const CMD: &str = "clip.removeBackground";

fn target(s: &Session, p: &Value) -> Result<ClipId> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let video = |c: ClipId| q.video_tracks.iter().any(|t| t.item(c).is_some_and(|it| s.project.item(it.item).is_some_and(|i| i.has_video())));
    match u64_p(p, "clip").map(ClipId) {
        Some(c) if video(c) => Ok(c),
        Some(_) => Err(bad(CMD, "`clip` is not a video clip of the open sequence")),
        None => s.state.selection.iter().copied().find(|c| video(*c)).ok_or_else(|| bad(CMD, "select the video clip to remove the background from")),
    }
}

fn remove(s: &mut Session, p: &Value) -> Result<Value> {
    let clip = target(s, p)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let it = q.find_item(clip).map(|x| x.1.clone()).ok_or_else(|| bad(CMD, "no such clip"))?;
    let rate = q.settings.frame_rate;
    let frames = rate.frame_at(it.duration).max(1);
    let key = crate::ai_tasks::task_key(
        CMD,
        &json!({"item": it.item.0, "in": it.source_in.0, "dur": it.duration.0, "speed": it.speed, "rev": it.reverse, "rate": rate.frame_duration().0}),
    );
    let ready = {
        let mut c = s.ai_task_cache.lock().unwrap_or_else(PoisonError::into_inner);
        c.iter().position(|(k, _)| *k == key).map(|i| c.remove(i).1)
    };
    if let Some(v) = ready {
        return apply(s, clip, v["first"].as_str().unwrap_or_default());
    }
    let matte = s.matte.clone().ok_or_else(|| EngineError::Other("removing the background needs Apple Vision (macOS 14 or later)".into()))?;
    if frames > 18_000 {
        return Err(bad(CMD, "the clip is longer than ten minutes at 30 fps: trim it first"));
    }
    let src = s.source(it.item).ok_or_else(|| bad(CMD, "the clip's media is offline"))?;
    let times: Vec<Tick> = (0..frames).map(|k| it.source_time_at(it.start + rate.tick_of(k))).collect();
    let stamp = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let base = it.name.rsplit_once('.').map_or(it.name.as_str(), |x| x.0).to_string();
    let safe: String = base.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
    let dir = std::path::Path::new(&crate::voiceover::media_dir(s, p, "Background Removed")).join(format!("{} {stamp}", safe.trim()));
    let background = bool_p(p, "background").unwrap_or(false);
    let job = crate::narration::new_job(s, format!("Removing the background of {}", it.name));
    job.progress.total.store(frames as u64, Ordering::Relaxed);
    let (prog, res, cache) = (job.progress.clone(), job.result.clone(), s.ai_task_cache.clone());
    let id = crate::narration::spawn(s, job, !background, "filmcraft-matte", move || {
        let t0 = web_time::Instant::now();
        let r = (|| -> std::result::Result<String, String> {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            for (k, t) in times.iter().enumerate() {
                if prog.cancel.load(Ordering::Relaxed) {
                    return Err("stopped".into());
                }
                let f = src.video_frame(filmcraft_media::FrameRequest { time: *t, scale: 1.0 }).map_err(|e| e.to_string())?;
                let (w, h) = (f.width as usize, f.height as usize);
                let rgba = matte(&f.to_rgba8(), w, h)?;
                let png = filmcraft_export::encode_png(rgba, w as u32, h as u32).map_err(|e| e.to_string())?;
                let path = dir.join(format!("frame_{:06}.png", k + 1));
                std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
                prog.done.store(k as u64 + 1, Ordering::Relaxed);
                *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("Frame {} of {}", k + 1, times.len());
            }
            let first = dir.join("frame_000001.png");
            cache.lock().unwrap_or_else(PoisonError::into_inner).push((key, json!({"first": first.to_string_lossy()})));
            Ok(format!("{} frames without background", times.len()))
        })();
        crate::narration::finish(&prog, &res, r, t0);
    })?;
    if background {
        return Ok(json!({"job": id, "background": true}));
    }
    if let Some(Err(e)) = s.jobs.iter().find(|j| j.id == id).and_then(|j| j.result.lock().unwrap_or_else(PoisonError::into_inner).clone()) {
        return Err(EngineError::Other(e));
    }
    remove(s, &json!({"clip": clip.0}))
}

/// The clip shows the matted frames (an image sequence at the sequence's rate) from their start.
fn apply(s: &mut Session, clip: ClipId, first: &str) -> Result<Value> {
    let rate = s.sequence_rate();
    let before = Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let r = (|| -> Result<ItemId> {
        let (item, _, _) = crate::commands::import_image_sequence_at(s, first, None, Some(rate))?;
        s.edit_sequence("Remove Background", |q, _, _| {
            let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
            it.item = item;
            it.source_in = Tick::ZERO;
            (it.speed, it.reverse, it.frame_hold) = (1.0, false, None);
            Ok(())
        })?;
        Ok(item)
    })();
    match r {
        Ok(item) => {
            crate::recipes::collapse_undo(s, before, "Remove Background");
            Ok(json!({"clip": clip.0, "item": item.0, "frames": first}))
        }
        Err(e) => {
            s.revert_to(mark);
            Err(e)
        }
    }
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Remove Background",
        menu: crate::ai_menu::EDITING,
        shortcut: None,
        params: r#"{"clip":id? (default: the selected video clip),"background":bool? (in a job; call again to put it in),"dir":path?}"#,
        enabled: has_seq,
        run: remove,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_shows_its_frames_without_background() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let dir = std::env::temp_dir().join(format!("fc-matte-{}", std::process::id()));
        // the "subject" is the left half: the right half is made transparent
        s.matte = Some(Arc::new(|rgba: &[u8], w: usize, _h: usize| {
            Ok(rgba.as_chunks::<4>().0.iter().enumerate().flat_map(|(i, p)| [p[0], p[1], p[2], if i % w < w / 2 { 255 } else { 0 }]).collect())
        }));
        // a short clip: the first second of the first V1 clip
        let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
        let start = s.active_sequence().unwrap().video_tracks[0].items[0].start;
        s.execute(
            "timeline.trim",
            json!({"clip": clip.0, "edge": "out", "delta": -(s.active_sequence().unwrap().video_tracks[0].items[0].duration - Tick::from_seconds_f64(0.5)).0}),
        )
        .unwrap();
        let before = s.active_sequence().unwrap().find_item(clip).unwrap().1.clone();
        let undo = s.history.undo.len();
        let r = s.execute(CMD, json!({"clip": clip.0, "dir": dir.to_string_lossy()})).unwrap();
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step: {r}");
        let after = s.active_sequence().unwrap().find_item(clip).unwrap().1.clone();
        assert_ne!(after.item, before.item, "a new picture");
        assert_eq!((after.start, after.duration, after.link), (start, before.duration, before.link), "same place, length and sound");
        let f = s.source(after.item).unwrap().video_frame(filmcraft_media::FrameRequest { time: Tick::ZERO, scale: 1.0 }).unwrap();
        let rgba = f.to_rgba8();
        let w = f.width as usize;
        assert_eq!(rgba[3], 255, "the left stays");
        assert_eq!(rgba[(w - 1) * 4 + 3], 0, "the right is transparent");
        s.matte = None;
        assert!(s.execute(CMD, json!({"clip": clip.0})).is_err(), "no Vision, no removal");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
