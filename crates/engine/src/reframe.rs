//! Reframe to faces (`clip.reframeToFace`), as Jilo Studio frames a talk for a vertical screen: the
//! clip's faces are found (Apple Vision on macOS, installed by the host as [`Session::face_detector`])
//! on frames two a second (one a second past ten minutes), a target is followed through time, the
//! track is smoothed, and the clip gets Motion keyframes: Scale to fill the sequence frame and
//! Position keys that keep the face where it belongs, without ever showing an edge.
//!
//! Targets: `speaker` — the face that stays longest near itself over the first frames that have
//! faces (a speaker, not someone passing); `group` — everyone steady in frame (a face with a
//! neighbour within two frames on either side), framed by the group's middle; `face` — the face
//! nearest a point (`x`, `y`, fractions of the picture) at `seconds`, followed forward and backward.
//! A face may move `TRACK_SPEED` of the width a second (at least `TRACK_JUMP` between two analysed
//! frames, the reach growing while it is lost), so someone walking across the shot is followed;
//! farther, the frame holds, and a speaker lost for `REACQUIRE` seconds is picked up again
//! wherever the nearest face is (a chosen face is never swapped for someone else). The smoothing
//! is a moving average over `smoothing` seconds (1.5, 0–6) with a dead zone (a nod moves nothing),
//! and the keys are thinned while the straight line between neighbours stays within 0.4 % of the
//! frame width. Scale keys a clip already has (Auto Zoom) keep their zooms relative to the new
//! scale. One undo step; the analysis is kept for the session, so a second framing is instant.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use filmcraft_geom::Vec2;
use filmcraft_project::{ClipId, Interpolation, Keyframe, ParamValue, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, f64_p, str_p};
use crate::{EngineError, Result, Session};

/// A face on a picture: its box in fractions of the picture (`y` from the top) and its confidence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub confidence: f64,
}

impl FaceBox {
    fn centre(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Finds the faces of an 8-bit RGBA picture (`rgba`, width, height).
pub type FaceDetector = Arc<dyn Fn(&[u8], usize, usize) -> std::result::Result<Vec<FaceBox>, String> + Send + Sync>;

/// The faces of one analysed frame (media seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct FaceSample {
    pub time: f64,
    pub faces: Vec<FaceBox>,
}

/// A point of a track: the target's centre (fractions of the picture) at a media second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPoint {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    /// A face was found on this frame (else the place was held).
    pub found: bool,
}

/// How far a face may move in a second, in fractions of the width: someone walking across the
/// shot (a test of a walk in a room moved up to 0.16 a second).
pub const TRACK_SPEED: f64 = 0.3;
/// The least a face may move between two analysed frames, in fractions of the width.
pub const TRACK_JUMP: f64 = 0.06;
/// Seconds after which a lost face is looked for anywhere in the picture.
pub const REACQUIRE: f64 = 1.0;
/// Head movement smaller than this (fractions of the width) moves nothing.
pub const DEAD_ZONE: f64 = 0.012;
/// Keys are dropped while the straight line between neighbours stays this close (fractions of the
/// sequence width).
pub const KEY_TOLERANCE: f64 = 0.004;

/// Distance in fractions of the width (the height scaled by the aspect).
fn distance(a: (f64, f64), b: (f64, f64), aspect: f64) -> f64 {
    (a.0 - b.0).hypot((a.1 - b.1) / aspect.max(1e-6))
}

fn nearest(faces: &[FaceBox], p: (f64, f64), aspect: f64, limit: f64) -> Option<(f64, f64)> {
    faces.iter().map(FaceBox::centre).map(|c| (c, distance(c, p, aspect))).filter(|(_, d)| *d < limit).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(c, _)| c)
}

/// How far a face may have moved in `dt` seconds.
fn jump(dt: f64) -> f64 {
    (TRACK_SPEED * dt.abs()).max(TRACK_JUMP)
}

/// Follow a face from `cur` (last found at `last`) through the samples `idx`, in that order: the
/// nearest face within reach of the time since it was last found (so the reach grows while it is
/// lost; a chosen face's for a second at most), else — with `reacquire`, missed on an earlier frame too and lost for `REACQUIRE`
/// seconds — the nearest face anywhere, else the place holds. A chosen face is not swapped for
/// someone else that way: it may only be looking away.
fn follow(s: &[&FaceSample], idx: impl Iterator<Item = usize>, mut cur: (f64, f64), mut last: f64, aspect: f64, reacquire: bool) -> Vec<TrackPoint> {
    let mut missed = 0;
    idx.map(|i| {
        let t = s[i].time;
        // a chosen face's reach grows for a second at most: never as far as someone else
        let lost = if reacquire { (t - last).abs() } else { (t - last).abs().min(REACQUIRE) };
        let mut n = nearest(&s[i].faces, cur, aspect, jump(lost));
        if n.is_none() && reacquire && missed > 0 && (t - last).abs() >= REACQUIRE {
            n = nearest(&s[i].faces, cur, aspect, f64::INFINITY);
        }
        if let Some(n) = n {
            cur = n;
            last = t;
            missed = 0;
        } else {
            missed += 1;
        }
        TrackPoint { time: t, x: cur.0, y: cur.1, found: n.is_some() }
    })
    .collect()
}

fn sorted(samples: &[FaceSample]) -> Vec<&FaceSample> {
    let mut v: Vec<&FaceSample> = samples.iter().collect();
    v.sort_by(|a, b| a.time.total_cmp(&b.time));
    v
}

/// The speaker's track (Jilo Studio's `trackTarget`).
pub fn track_speaker(samples: &[FaceSample], aspect: f64) -> Vec<TrackPoint> {
    let s = sorted(samples);
    let Some(start) = s.iter().position(|x| !x.faces.is_empty()) else { return Vec::new() };
    let persistence = |face: &FaceBox| {
        let mut cur = face.centre();
        let mut last = s[start].time;
        let mut hits = 0;
        for x in s.iter().skip(start + 1).take(10) {
            if let Some(n) = nearest(&x.faces, cur, aspect, jump(x.time - last)) {
                cur = n;
                last = x.time;
                hits += 1;
            }
        }
        hits
    };
    let seed = s[start].faces.iter().max_by(|a, b| persistence(a).cmp(&persistence(b)).then(a.w.total_cmp(&b.w))).map(FaceBox::centre);
    let Some(cur) = seed else { return Vec::new() };
    let mut out: Vec<TrackPoint> = s[..start].iter().map(|x| TrackPoint { time: x.time, x: cur.0, y: cur.1, found: false }).collect();
    out.extend(follow(&s, start..s.len(), cur, s[start].time, aspect, true));
    out
}

/// The track of the face nearest `point` at media second `time` (Jilo Studio's `trackFrom`).
pub fn track_from(samples: &[FaceSample], time: f64, point: (f64, f64), aspect: f64) -> Vec<TrackPoint> {
    let s = sorted(samples);
    let Some(seed_i) = (0..s.len()).min_by(|a, b| (s[*a].time - time).abs().total_cmp(&(s[*b].time - time).abs())) else { return Vec::new() };
    let Some(seed) = nearest(&s[seed_i].faces, point, aspect, f64::INFINITY) else { return Vec::new() };
    let at = s[seed_i].time;
    let mut back = follow(&s, (0..seed_i).rev(), seed, at, aspect, false);
    back.reverse();
    back.extend(follow(&s, seed_i..s.len(), seed, at, aspect, false));
    back
}

/// Everyone steady in frame (Jilo Studio's `groupTrack`): the middle of the group's left and right
/// edges and the mean height of their centres.
pub fn track_group(samples: &[FaceSample], aspect: f64) -> Vec<TrackPoint> {
    let s = sorted(samples);
    let raw: Vec<(f64, Option<(f64, f64)>)> = (0..s.len())
        .map(|i| {
            let stable: Vec<&FaceBox> = s[i]
                .faces
                .iter()
                .filter(|f| {
                    (i.saturating_sub(2)..=(i + 2).min(s.len().saturating_sub(1)))
                        .any(|o| o != i && nearest(&s[o].faces, f.centre(), aspect, TRACK_JUMP).is_some())
                })
                .collect();
            if stable.is_empty() {
                return (s[i].time, None);
            }
            let left = stable.iter().map(|f| f.x).fold(f64::INFINITY, f64::min);
            let right = stable.iter().map(|f| f.x + f.w).fold(f64::NEG_INFINITY, f64::max);
            let y = stable.iter().map(|f| f.y + f.h / 2.0).sum::<f64>() / stable.len() as f64;
            (s[i].time, Some(((left + right) / 2.0, y)))
        })
        .collect();
    let Some(mut cur) = raw.iter().find_map(|r| r.1) else { return Vec::new() };
    raw.into_iter()
        .map(|(time, g)| {
            if let Some(g) = g {
                cur = g;
            }
            TrackPoint { time, x: cur.0, y: cur.1, found: g.is_some() }
        })
        .collect()
}

/// A moving average over `seconds` and a dead zone (Jilo Studio's `smoothTrack`).
pub fn smooth_track(track: &[TrackPoint], seconds: f64, dead_zone: f64) -> Vec<TrackPoint> {
    let half = seconds.max(0.0) / 2.0;
    let averaged: Vec<TrackPoint> = track
        .iter()
        .map(|p| {
            let w: Vec<&TrackPoint> = track.iter().filter(|o| (o.time - p.time).abs() <= half).collect();
            let n = w.len().max(1) as f64;
            TrackPoint { x: w.iter().map(|o| o.x).sum::<f64>() / n, y: w.iter().map(|o| o.y).sum::<f64>() / n, ..*p }
        })
        .collect();
    let Some(mut held) = averaged.first().copied() else { return Vec::new() };
    averaged
        .into_iter()
        .map(|p| {
            if (p.x - held.x).abs() > dead_zone || (p.y - held.y).abs() > dead_zone {
                held = p;
            }
            TrackPoint { x: held.x, y: held.y, ..p }
        })
        .collect()
}

/// The points to keep (Ramer–Douglas–Peucker over time and two values, `tolerance` in value units).
pub fn thin(points: &[(f64, f64, f64)], tolerance: f64) -> Vec<usize> {
    if points.len() <= 2 {
        return (0..points.len()).collect();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b - a < 2 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let (mut worst, mut wi) = (-1.0f64, a);
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let along = (p.0 - pa.0) / (pb.0 - pa.0).max(1e-9);
            let e = (p.1 - (pa.1 + (pb.1 - pa.1) * along)).abs().max((p.2 - (pa.2 + (pb.2 - pa.2) * along)).abs());
            if e > worst {
                worst = e;
                wi = i;
            }
        }
        if worst > tolerance {
            keep[wi] = true;
            stack.push((a, wi));
            stack.push((wi, b));
        }
    }
    (0..points.len()).filter(|i| keep[*i]).collect()
}

/// Where the layer's anchor goes along one axis so that `point` (source px) lands on `target`
/// (sequence px) without opening an edge; centred when the layer cannot cover the frame.
fn axis(point: f64, target: f64, anchor: f64, src: f64, scale: f64, frame: f64) -> f64 {
    let lo = frame - scale * (src - anchor);
    let hi = scale * anchor;
    if lo > hi {
        return frame / 2.0 - scale * (src / 2.0 - anchor);
    }
    (target - scale * (point - anchor)).clamp(lo, hi)
}

/// Analysed faces by (media item, first and last media millisecond).
pub type FaceCache = Arc<Mutex<HashMap<(u64, i64, i64), Arc<Vec<FaceSample>>>>>;

/// A clip to analyse: its media, the media times to look at, the decoder and the scale.
struct Work {
    key: (u64, i64, i64),
    name: String,
    src: filmcraft_media::SharedSource,
    scale: f32,
    times: Vec<f64>,
}

const ANALYSIS_WIDTH: f64 = 1920.0;

/// The video clips to frame: `clips`, else the selected video clips.
fn targets(s: &Session, p: &Value) -> Vec<ClipId> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let video = |c: &ClipId| q.find_item(*c).is_some_and(|(tid, _)| q.track(tid).is_some_and(|t| t.kind == TrackKind::Video));
    match p.get("clips") {
        Some(v) => v.as_array().into_iter().flatten().filter_map(Value::as_u64).map(ClipId).filter(video).collect(),
        None => s.state.selection.iter().copied().filter(video).collect(),
    }
}

/// The media range a clip shows, in media milliseconds (sorted for reversed clips).
fn media_range(it: &filmcraft_project::TrackItem) -> (i64, i64) {
    let a = it.source_time_at(it.start).seconds();
    let b = it.source_time_at(it.end() - Tick(1)).seconds();
    let (a, b) = (a.min(b), a.max(b));
    ((a * 1000.0).floor() as i64, (b * 1000.0).ceil() as i64)
}

fn work_for(s: &Session, clips: &[ClipId]) -> Vec<Work> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut out: Vec<Work> = Vec::new();
    for c in clips {
        let Some((_, it)) = q.find_item(*c) else { continue };
        let (a, b) = media_range(it);
        let key = (it.item.0, a, b);
        if out.iter().any(|w| w.key == key) || s.face_cache.lock().unwrap_or_else(PoisonError::into_inner).contains_key(&key) {
            continue;
        }
        let Some(src) = s.source(it.item) else { continue };
        let Some(v) = src.info().video.clone() else { continue };
        let span = (b - a) as f64 / 1000.0;
        let fps = if span <= 600.0 { 2.0 } else { 1.0 };
        let n = ((span * fps).floor() as usize + 1).min(3000);
        let times = (0..n).map(|i| a as f64 / 1000.0 + i as f64 / fps).collect();
        out.push(Work { key, name: it.name.clone(), src, scale: (ANALYSIS_WIDTH / v.width.max(1) as f64).min(1.0) as f32, times });
    }
    out
}

/// Analyse `work` (in the calling thread), reporting to `prog`; the results go to `cache`.
fn analyse(work: Vec<Work>, detector: FaceDetector, cache: FaceCache, prog: &filmcraft_export::Progress) -> std::result::Result<String, String> {
    let total: usize = work.iter().map(|w| w.times.len()).sum();
    prog.total.store(total.max(1) as u64, Ordering::Relaxed);
    let (mut done, mut found) = (0u64, 0usize);
    for w in work {
        let mut samples = Vec::with_capacity(w.times.len());
        for (k, t) in w.times.iter().enumerate() {
            if prog.cancel.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            let f = w
                .src
                .video_frame(filmcraft_media::FrameRequest { time: Tick::from_seconds_f64(*t), scale: w.scale })
                .map_err(|e| format!("{}: {e}", w.name))?;
            let faces = detector(&f.to_rgba8(), f.width as usize, f.height as usize)?;
            found += faces.len();
            samples.push(FaceSample { time: *t, faces });
            done += 1;
            prog.done.store(done, Ordering::Relaxed);
            *prog.status.lock().unwrap_or_else(PoisonError::into_inner) = format!("{}: frame {} of {}", w.name, k + 1, w.times.len());
        }
        cache.lock().unwrap_or_else(PoisonError::into_inner).insert(w.key, Arc::new(samples));
    }
    Ok(format!("Faces found: {found} in {done} frames"))
}

fn reframe(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.reframeToFace";
    let target = str_p(p, "target").unwrap_or("speaker");
    if !matches!(target, "speaker" | "group" | "face") {
        return Err(bad(cmd, format!("`target`: speaker, group or face, not `{target}`")));
    }
    let point = (f64_p(p, "x").filter(|x| x.is_finite()), f64_p(p, "y").filter(|y| y.is_finite()));
    if target == "face" && (point.0.is_none() || point.1.is_none()) {
        return Err(bad(cmd, "the face target needs `x` and `y` (fractions of the picture) and `seconds`"));
    }
    let clips = targets(s, p);
    if clips.is_empty() {
        return Err(bad(cmd, "select the video clips to reframe"));
    }
    let detector =
        s.face_detector.clone().ok_or_else(|| EngineError::Other("face detection is not available on this system (it needs Apple Vision on macOS)".into()))?;
    let work = work_for(s, &clips);
    if !work.is_empty() {
        let job = crate::narration::new_job(s, "Finding faces".into());
        let (prog, res, cache) = (job.progress.clone(), job.result.clone(), s.face_cache.clone());
        let background = bool_p(p, "background").unwrap_or(false);
        let id = crate::narration::spawn(s, job, !background, "filmcraft-faces", move || {
            let t0 = web_time::Instant::now();
            let r = analyse(work, detector, cache, &prog);
            crate::narration::finish(&prog, &res, r, t0);
        })?;
        if background {
            return Ok(json!({"job": id, "background": true}));
        }
        if let Some(Err(e)) = s.jobs.iter().find(|j| j.id == id).and_then(|j| j.result.lock().unwrap_or_else(PoisonError::into_inner).clone()) {
            return Err(EngineError::Other(e));
        }
    }
    let smoothing = f64_p(p, "smoothing").filter(|x| x.is_finite()).unwrap_or(1.5).clamp(0.0, 6.0);
    let anchor_x = f64_p(p, "anchorX").filter(|x| x.is_finite()).unwrap_or(0.5).clamp(0.0, 1.0);
    let anchor_y = f64_p(p, "anchorY").filter(|x| x.is_finite()).unwrap_or(0.4).clamp(0.0, 1.0);
    let zoom = f64_p(p, "zoom").filter(|x| x.is_finite()).unwrap_or(100.0).clamp(100.0, 400.0) / 100.0;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let (fw, fh) = (q.settings.width as f64, q.settings.height as f64);
    let at = crate::commands::time_p(s, p, "").unwrap_or(s.playhead());
    // per clip: (clip, base scale, new scale, position keys)
    let mut plans: Vec<(ClipId, f64, f64, Vec<Keyframe>)> = Vec::new();
    let mut empty = Vec::new();
    for c in &clips {
        let Some((_, it)) = q.find_item(*c) else { continue };
        let (a, b) = media_range(it);
        let Some(samples) = s.face_cache.lock().unwrap_or_else(PoisonError::into_inner).get(&(it.item.0, a, b)).cloned() else { continue };
        let Some((sw, sh)) = s.project.source_size(it.item).map(|(w, h)| (w as f64, h as f64)) else { continue };
        let aspect = sw / sh.max(1.0);
        let track = match target {
            "group" => track_group(&samples, aspect),
            "face" => {
                let t = if it.range().contains(at) { it.source_time_at(at).seconds() } else { a as f64 / 1000.0 };
                track_from(&samples, t, (point.0.unwrap_or(0.5), point.1.unwrap_or(0.5)), aspect)
            }
            _ => track_speaker(&samples, aspect),
        };
        if track.iter().all(|t| !t.found) {
            empty.push(it.name.clone());
            continue;
        }
        let track = smooth_track(&track, smoothing, DEAD_ZONE);
        let Some(m) = it.effect("motion") else { continue };
        let mt0 = it.effect_time_at(it.start);
        let base = m.f64_at("scale", mt0);
        let anchor = m.param("anchor").map(|_| m.vec2_at("anchor", mt0)).filter(|v| v.x.is_finite()).unwrap_or(Vec2::new(sw / 2.0, sh / 2.0));
        let fit = if it.scale_to_frame { (fw / sw.max(1.0)).min(fh / sh.max(1.0)) } else { 1.0 };
        let cover = (fw / sw.max(1.0)).max(fh / sh.max(1.0)) * zoom;
        let scale_param = (cover / fit.max(1e-9) * 100.0 * 10.0).round() / 10.0;
        let points: Vec<(f64, f64, f64)> = track
            .iter()
            .filter(|t| t.time >= a as f64 / 1000.0 - 0.5 && t.time <= b as f64 / 1000.0 + 0.5)
            .map(|t| (t.time, axis(t.x * sw, anchor_x * fw, anchor.x, sw, cover, fw), axis(t.y * sh, anchor_y * fh, anchor.y, sh, cover, fh)))
            .collect();
        let keys: Vec<Keyframe> = thin(&points, KEY_TOLERANCE * fw)
            .into_iter()
            .map(|i| {
                let (t, x, y) = points[i];
                let mut k = Keyframe::new(Tick::from_seconds_f64(t), ParamValue::Vec2(Vec2::new((x * 10.0).round() / 10.0, (y * 10.0).round() / 10.0)));
                k.interp = Interpolation::Linear;
                k
            })
            .collect();
        plans.push((*c, base, scale_param, keys));
    }
    if plans.is_empty() {
        return Err(EngineError::Other(if empty.is_empty() {
            "no clip could be reframed".into()
        } else {
            format!("no faces were found in {}", empty.join(", "))
        }));
    }
    let n = plans.len();
    s.edit_sequence("Reframe to Face", |q, _, _| {
        for (c, base, scale, keys) in &plans {
            let Some((_, it)) = q.find_item_mut(*c) else { continue };
            let Some(m) = it.effect_mut("motion") else { continue };
            if let Some(sp) = m.params.get_mut("scale") {
                // zooms already keyed (Auto Zoom) stay relative to the new scale
                let ratio = if *base > 0.0 { scale / base } else { 1.0 };
                sp.value = ParamValue::Float(*scale);
                for k in &mut sp.keyframes {
                    if let Some(v) = k.value.as_f64() {
                        k.value = ParamValue::Float((v * ratio * 10.0).round() / 10.0);
                    }
                }
            }
            if let Some(pp) = m.params.get_mut("position") {
                if let Some(first) = keys.first() {
                    pp.value = first.value.clone();
                }
                // a face that never moves needs no keys
                pp.keyframes = if keys.iter().any(|k| k.value != keys[0].value) { keys.clone() } else { Vec::new() };
            }
        }
        Ok(())
    })?;
    let keys: usize = plans.iter().map(|p| if p.3.iter().any(|k| k.value != p.3[0].value) { p.3.len() } else { 0 }).sum();
    Ok(json!({"clips": n, "target": target, "keys": keys, "noFaces": empty}))
}

/// The faces at the playhead in the selected clip under it (for picking the face to follow): the
/// clip, its media item and media time (ticks), and boxes in fractions of the picture.
fn faces_here(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "clip.facesAt";
    let detector =
        s.face_detector.clone().ok_or_else(|| EngineError::Other("face detection is not available on this system (it needs Apple Vision on macOS)".into()))?;
    let clips = targets(s, p);
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let at = crate::commands::time_p(s, p, "").unwrap_or(s.playhead());
    let it = clips
        .iter()
        .filter_map(|c| q.find_item(*c).map(|x| x.1.clone()))
        .find(|it| it.range().contains(at))
        .ok_or_else(|| bad(cmd, "no selected video clip under the playhead"))?;
    let src = s.source(it.item).ok_or_else(|| bad(cmd, "the clip has no media"))?;
    let w = src.info().video.as_ref().map_or(1920, |v| v.width).max(1);
    let time = it.source_time_at(at);
    let f = src
        .video_frame(filmcraft_media::FrameRequest { time, scale: (ANALYSIS_WIDTH / w as f64).min(1.0) as f32 })
        .map_err(|e| EngineError::Other(e.to_string()))?;
    let faces = detector(&f.to_rgba8(), f.width as usize, f.height as usize).map_err(EngineError::Other)?;
    Ok(json!({
        "clip": it.id.0,
        "item": it.item.0,
        "time": time.0,
        "faces": faces.iter().map(|f| json!({"x": f.x, "y": f.y, "w": f.w, "h": f.h, "confidence": f.confidence})).collect::<Vec<_>>(),
    }))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "clip.reframeToFace",
            label: "Reframe to Face…",
            menu: crate::ai_menu::EDITING,
            shortcut: None,
            params: r#"{"clips":[id]?,"target":"speaker|group|face"?,"x":0-1?,"y":0-1?,"seconds":f64?,"smoothing":0-6?,"anchorX":0-1?,"anchorY":0-1?,"zoom":100-400?,"background":bool?}"#,
            enabled: crate::commands::has_seq,
            run: reframe,
            journal: true,
        },
        CommandSpec {
            id: "clip.facesAt",
            label: "Faces at Playhead",
            menu: &[],
            shortcut: None,
            params: r#"{"clips":[id]?,"seconds":f64?}"#,
            enabled: crate::commands::has_seq,
            run: faces_here,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f64, y: f64, s: f64) -> FaceBox {
        FaceBox { x: x - s / 2.0, y: y - s / 2.0, w: s, h: s, confidence: 0.9 }
    }

    fn sample(time: f64, faces: Vec<FaceBox>) -> FaceSample {
        FaceSample { time, faces }
    }

    #[test]
    fn the_speaker_is_the_face_that_stays_not_the_biggest() {
        // a big face passes by on the first frame; the speaker drifts right and is lost once
        let samples: Vec<FaceSample> = (0..12)
            .map(|i| {
                let mut f = if i == 5 { vec![] } else { vec![face(0.30 + 0.01 * i as f64, 0.4, 0.1)] };
                if i == 0 {
                    f.push(face(0.80, 0.5, 0.3));
                }
                sample(i as f64 * 0.5, f)
            })
            .rev() // order does not matter
            .collect();
        let t = track_speaker(&samples, 16.0 / 9.0);
        assert_eq!(t.len(), 12);
        assert!((t[0].x - 0.30).abs() < 1e-9, "{:?}", t[0]);
        assert!(!t[5].found && (t[5].x - 0.34).abs() < 1e-9, "the place is held: {:?}", t[5]);
        assert!((t[11].x - 0.41).abs() < 1e-9);
        assert!(track_speaker(&[sample(0.0, vec![])], 1.0).is_empty());
        assert!(track_speaker(&[], 1.0).is_empty());
    }

    #[test]
    fn a_chosen_face_is_followed_both_ways_and_a_jump_holds_the_frame() {
        let samples: Vec<FaceSample> = (0..9).map(|i| sample(i as f64, vec![face(0.2, 0.4, 0.1), face(0.6 + 0.01 * i as f64, 0.4, 0.1)])).collect();
        let t = track_from(&samples, 4.0, (0.65, 0.4), 16.0 / 9.0);
        assert_eq!(t.len(), 9);
        assert!(t.iter().all(|p| p.found));
        assert!((t[0].x - 0.60).abs() < 1e-9 && (t[8].x - 0.68).abs() < 1e-9);
        // the face is gone for a frame: the place holds
        let mut s2 = samples.clone();
        s2[6].faces = vec![face(0.2, 0.4, 0.1)];
        let t = track_from(&s2, 4.0, (0.65, 0.4), 16.0 / 9.0);
        assert!(!t[6].found && (t[6].x - 0.65).abs() < 1e-9);
        assert!(track_from(&[sample(0.0, vec![])], 0.0, (0.5, 0.5), 1.0).is_empty());
    }

    /// Someone walking across the shot (0.12 of the width every half second, as in a test of a walk
    /// in a room) is followed, and picked up again after being lost.
    #[test]
    fn a_walk_across_the_shot_is_followed_and_picked_up_again() {
        let walk: Vec<FaceSample> = (0..12).map(|i| sample(i as f64 * 0.5, vec![face(0.1 + 0.07 * i as f64, 0.2, 0.07)])).collect();
        let t = track_speaker(&walk, 16.0 / 9.0);
        assert!(t.iter().all(|p| p.found), "{t:?}");
        assert!((t[11].x - 0.87).abs() < 1e-9);
        // gone for a second and a half, back on the other side
        let mut gap = walk.clone();
        for g in &mut gap[4..7] {
            g.faces.clear();
        }
        gap[7].faces = vec![face(0.9, 0.2, 0.07)];
        let t = track_speaker(&gap, 16.0 / 9.0);
        assert!(!t[4].found && !t[6].found && t[7].found && (t[7].x - 0.9).abs() < 1e-9, "{t:?}");
    }

    #[test]
    fn the_group_is_everyone_steady_in_frame() {
        // two steady faces and a flash on one frame
        let samples: Vec<FaceSample> = (0..6)
            .map(|i| {
                let mut f = vec![face(0.3, 0.4, 0.1), face(0.7, 0.5, 0.1)];
                if i == 3 {
                    f.push(face(0.95, 0.9, 0.05));
                }
                sample(i as f64, f)
            })
            .collect();
        let t = track_group(&samples, 16.0 / 9.0);
        assert_eq!(t.len(), 6);
        for p in &t {
            assert!((p.x - 0.5).abs() < 1e-9 && (p.y - 0.45).abs() < 1e-9, "{p:?}");
        }
    }

    #[test]
    fn smoothing_averages_and_a_nod_moves_nothing() {
        let jitter: Vec<TrackPoint> =
            (0..20).map(|i| TrackPoint { time: i as f64 * 0.5, x: 0.5 + if i % 2 == 0 { 0.004 } else { -0.004 }, y: 0.4, found: true }).collect();
        let s = smooth_track(&jitter, 1.5, DEAD_ZONE);
        assert!(s.iter().all(|p| p.x == s[0].x && p.y == s[0].y), "{s:?}");
        let ramp: Vec<TrackPoint> = (0..20).map(|i| TrackPoint { time: i as f64 * 0.5, x: 0.3 + 0.02 * i as f64, y: 0.4, found: true }).collect();
        let s = smooth_track(&ramp, 1.0, DEAD_ZONE);
        assert!((s[10].x - 0.5).abs() < 1e-9, "the middle of a straight move is kept: {:?}", s[10]);
        assert_eq!(smooth_track(&ramp, 0.0, 0.0), ramp);
        assert!(smooth_track(&[], 1.0, 0.0).is_empty());
    }

    #[test]
    fn keys_on_a_straight_line_are_thinned() {
        let line: Vec<(f64, f64, f64)> = (0..10).map(|i| (i as f64, i as f64 * 2.0, 5.0)).collect();
        assert_eq!(thin(&line, 0.1), vec![0, 9]);
        let mut bent = line.clone();
        bent[4].2 = 9.0;
        assert_eq!(thin(&bent, 0.1), vec![0, 3, 4, 5, 9]);
        assert_eq!(thin(&line[..2], 0.1), vec![0, 1]);
        assert!(thin(&[], 0.1).is_empty());
    }

    #[test]
    fn the_anchor_lands_on_the_target_without_opening_an_edge() {
        // a 1920 source scaled 1.5 in a 1080 frame, anchor in the middle
        assert!((axis(960.0, 540.0, 960.0, 1920.0, 1.5, 1080.0) - 540.0).abs() < 1e-9);
        assert!((axis(1200.0, 540.0, 960.0, 1920.0, 1.5, 1080.0) - 180.0).abs() < 1e-9);
        // too far left: the left edge stays at the frame edge
        assert!((axis(100.0, 540.0, 960.0, 1920.0, 1.5, 1080.0) - 1440.0).abs() < 1e-9);
        // too far right: the right edge stays at the frame edge
        assert!((axis(1900.0, 540.0, 960.0, 1920.0, 1.5, 1080.0) - (1080.0 - 1.5 * 960.0)).abs() < 1e-9);
        // a layer smaller than the frame is centred
        assert!((axis(100.0, 540.0, 960.0, 1920.0, 0.25, 1080.0) - 540.0).abs() < 1e-9);
    }
}
