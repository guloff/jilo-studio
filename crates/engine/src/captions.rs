//! Caption commands: caption tracks, caption editing, caption file import/export.
//!
//! Ids (`captions.*`) follow Premiere's Sequence ▸ Captions menu and the Text panel's Captions tab.
//! Captions are addressed by id (`caption`, `captions`); without one, commands use the caption
//! selection (`EditorState::caption_selection`) or the caption under the playhead. Caption tracks
//! are addressed by id or `"C1"`, `"C2"`… (top first); the default is the first caption track.

use serde_json::{Value, json};

use filmcraft_captions::{Document, Format, WriteOptions};
use filmcraft_edit as edit;
use filmcraft_edit::Edge;
use filmcraft_project::{
    CaptionAlign, CaptionAnchor, CaptionAnimation, CaptionFormat, CaptionHighlight, CaptionStyle, CaptionTrack, ClipId, SequenceSettings, TrackId,
};
use filmcraft_time::{Tick, TimeDisplay, format_time};

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, time_p, u64_p};
use crate::{EngineError, Result, Session};

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;

fn spec(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    shortcut: Option<&'static str>,
    params: &'static str,
    enabled: Enabled,
    run: Run,
) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut, params, enabled, run, journal: true }
}

pub(crate) fn has_caption_track(s: &Session) -> std::result::Result<(), String> {
    has_seq(s)?;
    if s.active_sequence().is_some_and(|q| !q.caption_tracks.is_empty()) { Ok(()) } else { Err("the sequence has no caption track".into()) }
}

fn has_captions(s: &Session) -> std::result::Result<(), String> {
    has_caption_track(s)?;
    if s.active_sequence().is_some_and(|q| q.caption_tracks.iter().any(|t| !t.captions.is_empty())) { Ok(()) } else { Err("there are no captions".into()) }
}

/// The caption track named by `track` (id or "C1"…), else the first one.
pub fn track_param(s: &Session, p: &Value) -> Option<TrackId> {
    let seq = s.active_sequence()?;
    match p.get("track") {
        Some(Value::Number(n)) => n.as_u64().map(TrackId).filter(|t| seq.caption_track(*t).is_some()),
        Some(Value::String(name)) => {
            let idx: usize = name.trim_start_matches(['C', 'c']).parse().ok()?;
            seq.caption_tracks.get(idx.checked_sub(1)?).map(|t| t.id)
        }
        _ => seq.caption_tracks.first().map(|t| t.id),
    }
}

fn caption_ids(s: &Session, p: &Value) -> Vec<ClipId> {
    if let Some(a) = p.get("captions").and_then(Value::as_array) {
        return a.iter().filter_map(Value::as_u64).map(ClipId).collect();
    }
    if let Some(c) = u64_p(p, "caption") {
        return vec![ClipId(c)];
    }
    if !s.state.caption_selection.is_empty() {
        return s.state.caption_selection.clone();
    }
    // the caption under the playhead on the first caption track that has one
    let t = s.playhead();
    s.active_sequence().and_then(|q| q.caption_tracks.iter().find_map(|tr| tr.caption_at(t).map(|c| c.id))).into_iter().collect()
}

fn one_caption(s: &Session, p: &Value, cmd: &str) -> Result<ClipId> {
    caption_ids(s, p).first().copied().ok_or_else(|| bad(cmd, "no caption (pass `caption` or select one)"))
}

fn parse_color(v: &Value) -> Option<[u8; 4]> {
    if let Some(a) = v.as_array() {
        let c: Vec<u8> = a.iter().filter_map(|x| x.as_u64().map(|n| n.min(255) as u8)).collect();
        return match c.len() {
            3 => Some([c[0], c[1], c[2], 255]),
            4 => Some([c[0], c[1], c[2], c[3]]),
            _ => None,
        };
    }
    let h = v.as_str()?.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some([b(0)?, b(2)?, b(4)?, 255]),
        8 => Some([b(0)?, b(2)?, b(4)?, b(6)?]),
        _ => None,
    }
}

fn hex(c: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
}

/// Parameters of `captions.setStyle`: every field of [`CaptionStyle`]; `track: "all"` styles every
/// caption track of the sequence in one undo step.
const STYLE_PARAMS: &str = r##"{"track":id|"C1"|"all","font":str?,"fontStyle":str?,"size":f32?,"uppercase":bool?,"color":"#rrggbb[aa]"?,"background":bool?,"backgroundColor":"#rrggbbaa"?,"boxRadius":f32?,"align":"left|center|right"?,"anchor":"top|middle|bottom"?,"margin":0..0.45 (fraction of frame height)?,"lineSpacing":f32?,"outline":f32?,"outlineColor":str?,"shadow":bool?,"shadowColor":str?,"shadowOffset":[x,y]?,"shadowBlur":f32?,"highlight":"none|color|box"?,"highlightColor":str?,"highlightScale":1..2?,"chunkWords":0..8?,"animation":"none|fade|pop|slide|typewriter"?,"tracking":-200..1000 (1/1000 em)?,"position":[x,y] (0..1, block centre)|null (back to anchor)?,"captions":[id]? (these phrases get a style of their own, from the track's),"own":false? (with captions: back to the track's style),"reset":bool?,"merge":bool? (a drag: one undo step),"begin":bool?}"##;

/// A caption style as JSON (the `style` of `captions.inspect`, `captions.setStyle`'s parameters).
pub fn style_json(y: &CaptionStyle) -> Value {
    json!({
        "font": y.font, "fontStyle": y.font_style, "size": y.size, "uppercase": y.uppercase, "color": hex(y.color),
        "background": y.background, "backgroundColor": hex(y.background_color), "boxRadius": y.box_radius,
        "align": format!("{:?}", y.align).to_lowercase(), "anchor": format!("{:?}", y.anchor).to_lowercase(),
        "margin": y.margin, "lineSpacing": y.line_spacing, "outline": y.outline, "outlineColor": hex(y.outline_color),
        "shadow": y.shadow, "shadowColor": hex(y.shadow_color), "shadowOffset": y.shadow_offset, "shadowBlur": y.shadow_blur,
        "highlight": format!("{:?}", y.highlight).to_lowercase(), "highlightColor": hex(y.highlight_color), "highlightScale": y.highlight_scale,
        "chunkWords": y.chunk_words, "animation": format!("{:?}", y.animation).to_lowercase(),
        "tracking": y.tracking, "position": y.position,
    })
}

/// Apply the style fields present in `p` (see [`STYLE_PARAMS`]) to `y`.
pub(crate) fn apply_style(y: &mut CaptionStyle, p: &Value, cmd: &str) -> Result<()> {
    if bool_p(p, "reset").unwrap_or(false) {
        *y = CaptionStyle::default();
    }
    if let Some(v) = str_p(p, "font") {
        y.font = v.to_string();
    }
    if let Some(v) = str_p(p, "fontStyle") {
        y.font_style = v.to_string();
    }
    if let Some(v) = f64_p(p, "size") {
        y.size = (v as f32).clamp(4.0, 400.0);
    }
    if let Some(b) = bool_p(p, "uppercase") {
        y.uppercase = b;
    }
    if let Some(c) = p.get("color").and_then(parse_color) {
        y.color = c;
    }
    if let Some(b) = bool_p(p, "background") {
        y.background = b;
    }
    if let Some(c) = p.get("backgroundColor").and_then(parse_color) {
        y.background_color = c;
    }
    if let Some(v) = f64_p(p, "boxRadius") {
        y.box_radius = (v as f32).clamp(0.0, 200.0);
    }
    if let Some(c) = p.get("outlineColor").and_then(parse_color) {
        y.outline_color = c;
    }
    if let Some(v) = f64_p(p, "outline") {
        y.outline = (v as f32).clamp(0.0, 40.0);
    }
    if let Some(v) = f64_p(p, "margin") {
        y.margin = (v as f32).clamp(0.0, 0.45);
    }
    if let Some(v) = f64_p(p, "lineSpacing") {
        y.line_spacing = (v as f32).clamp(0.8, 4.0);
    }
    if let Some(b) = bool_p(p, "shadow") {
        y.shadow = b;
    }
    if let Some(c) = p.get("shadowColor").and_then(parse_color) {
        y.shadow_color = c;
    }
    if let Some(a) = p.get("shadowOffset").and_then(Value::as_array) {
        let v: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|v| (v as f32).clamp(-100.0, 100.0)).collect();
        if let [x, y2] = v[..] {
            y.shadow_offset = [x, y2];
        } else {
            return Err(bad(cmd, "`shadowOffset` is [x, y]"));
        }
    }
    if let Some(v) = f64_p(p, "shadowBlur") {
        y.shadow_blur = (v as f32).clamp(0.0, 100.0);
    }
    match str_p(p, "highlight") {
        Some("none") => y.highlight = CaptionHighlight::None,
        Some("color") => y.highlight = CaptionHighlight::Color,
        Some("box") => y.highlight = CaptionHighlight::Box,
        Some(o) => return Err(bad(cmd, format!("unknown highlight `{o}` (none, color, box)"))),
        None => {}
    }
    if let Some(c) = p.get("highlightColor").and_then(parse_color) {
        y.highlight_color = c;
    }
    if let Some(v) = f64_p(p, "highlightScale") {
        y.highlight_scale = (v as f32).clamp(1.0, 2.0);
    }
    if let Some(v) = p.get("chunkWords").and_then(Value::as_u64) {
        y.chunk_words = v.min(8) as u8;
    }
    match str_p(p, "animation") {
        Some("none") => y.animation = CaptionAnimation::None,
        Some("fade") => y.animation = CaptionAnimation::Fade,
        Some("pop") => y.animation = CaptionAnimation::Pop,
        Some("slide") => y.animation = CaptionAnimation::Slide,
        Some("typewriter") => y.animation = CaptionAnimation::Typewriter,
        Some(o) => return Err(bad(cmd, format!("unknown animation `{o}` (none, fade, pop, slide, typewriter)"))),
        None => {}
    }
    match str_p(p, "align") {
        Some("left") => y.align = CaptionAlign::Left,
        Some("right") => y.align = CaptionAlign::Right,
        Some("center") => y.align = CaptionAlign::Center,
        Some(o) => return Err(bad(cmd, format!("unknown align `{o}`"))),
        None => {}
    }
    if let Some(v) = f64_p(p, "tracking") {
        y.tracking = (v as f32).clamp(-200.0, 1000.0);
    }
    match p.get("position") {
        Some(Value::Null) => y.position = None,
        Some(Value::Array(a)) => {
            let v: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|v| (v as f32).clamp(0.0, 1.0)).collect();
            let [x, y2] = v[..] else { return Err(bad(cmd, "`position` is [x, y] in 0..1, or null")) };
            y.position = Some([x, y2]);
        }
        Some(_) => return Err(bad(cmd, "`position` is [x, y] in 0..1, or null")),
        None => {}
    }
    match str_p(p, "anchor") {
        // a chosen anchor takes the block back from where it was dragged
        Some(_) if p.get("position").is_none() && y.position.is_some() => {
            y.position = None;
            return apply_style(y, &json!({"anchor": p["anchor"]}), cmd);
        }
        Some("top") => y.anchor = CaptionAnchor::Top,
        Some("middle") => y.anchor = CaptionAnchor::Middle,
        Some("bottom") => y.anchor = CaptionAnchor::Bottom,
        Some(o) => return Err(bad(cmd, format!("unknown anchor `{o}`"))),
        None => {}
    }
    Ok(())
}

/// The caption tracks a style command acts on: every one for `track: "all"`, else the one named.
pub(crate) fn style_targets(s: &Session, p: &Value, cmd: &str) -> Result<Vec<TrackId>> {
    if str_p(p, "track") == Some("all") {
        let ids: Vec<TrackId> = s.active_sequence().map(|q| q.caption_tracks.iter().map(|t| t.id).collect()).unwrap_or_default();
        return if ids.is_empty() { Err(bad(cmd, "the sequence has no caption track")) } else { Ok(ids) };
    }
    Ok(vec![track_param(s, p).ok_or_else(|| bad(cmd, "no such caption track"))?])
}

/// Characters per line and lines for caption `id`'s track: 42 × 2, a CEA-608 track's 32 columns.
fn fit_of(q: &filmcraft_project::Sequence, id: ClipId) -> (usize, usize) {
    let cols = q.find_caption(id).and_then(|(t, _)| q.caption_track(t)).and_then(|t| t.format.max_columns());
    (cols.unwrap_or(42).min(42), 2)
}

/// `captions.splitLong`: captions longer than `lines` lines of `maxChars` become captions that fit.
fn split_long(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.splitLong";
    let ids: Vec<ClipId> = match p.get("captions").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(ClipId).collect(),
        None if !s.state.caption_selection.is_empty() => s.state.caption_selection.clone(),
        None => {
            let tid = track_param(s, p).ok_or_else(|| bad(cmd, "no caption track"))?;
            s.active_sequence().and_then(|q| q.caption_track(tid)).map(|t| t.captions.iter().map(|c| c.id).collect()).unwrap_or_default()
        }
    };
    let max = p.get("maxChars").and_then(Value::as_u64).map(|v| v.clamp(8, 200) as usize);
    let lines = p.get("lines").and_then(Value::as_u64).map_or(2, |v| v.clamp(1, 4) as usize);
    let frame = s.sequence_rate().frame_duration();
    let (made, short) = s.edit_sequence("Split Long Captions", |q, ctx, _| {
        let (mut made, mut short) = (Vec::new(), 0);
        for id in &ids {
            let (cols, _) = fit_of(q, *id);
            match edit::captions::split_long(q, *id, max.unwrap_or(cols), lines, frame, ctx) {
                Ok(m) => made.extend(m),
                Err(edit::EditError::TooShort) => short += 1,
                Err(e) => return Err(e.into()),
            }
        }
        Ok((made, short))
    })?;
    Ok(json!({"made": made.iter().map(|c| c.0).collect::<Vec<_>>(), "tooShort": short}))
}

fn set_style(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.setStyle";
    let tracks = style_targets(s, p, cmd)?;
    // check the parameters once before the edit
    apply_style(&mut CaptionStyle::default(), p, cmd)?;
    let p = p.clone();
    // `merge`: a drag in the monitor is one undo step; `begin` starts a new one
    let merge =
        bool_p(&p, "merge").unwrap_or(false).then(|| format!("captionStyle:{}", p.get("captions").map_or_else(|| p["track"].to_string(), Value::to_string)));
    if bool_p(&p, "begin").unwrap_or(false) {
        s.history.merge_key = None;
    }
    // phrases of their own: the listed captions get (or keep) a style that starts as the track's
    let own: Option<Vec<ClipId>> = p.get("captions").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect());
    if let Some(ids) = own {
        let back = bool_p(&p, "own") == Some(false);
        let label = if back { "Use Track Caption Style" } else { "Caption Style (Phrase)" };
        let body = |pr: &mut filmcraft_project::Project, st: &mut crate::EditorState| {
            let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
            let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
            let mut n = 0;
            for t in &mut q.caption_tracks {
                let base = t.style.clone();
                for c in t.captions.iter_mut().filter(|c| ids.contains(&c.id)) {
                    if back {
                        c.style = None;
                    } else {
                        apply_style(c.style.get_or_insert_with(|| Box::new(base.clone())), &p, cmd)?;
                    }
                    n += 1;
                }
            }
            if n == 0 {
                return Err(bad(cmd, "none of `captions` is on this sequence"));
            }
            Ok(n)
        };
        let n = match &merge {
            Some(k) => s.edit_merged(label, k, body)?,
            None => s.edit(label, body)?,
        };
        return Ok(json!({"captions": n}));
    }
    let body = |pr: &mut filmcraft_project::Project, st: &mut crate::EditorState| {
        let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
        let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        for tid in &tracks {
            let t = q.caption_track_mut(*tid).ok_or(EngineError::NoSequence)?;
            apply_style(&mut t.style, &p, cmd)?;
        }
        Ok(())
    };
    match &merge {
        Some(k) => s.edit_merged("Caption Style", k, body)?,
        None => s.edit("Caption Style", body)?,
    }
    Ok(json!({"tracks": tracks.iter().map(|t| t.0).collect::<Vec<_>>()}))
}

/// The caption format of a file, if it is one (extension `.srt` / `.vtt` / `.scc` / `.mcc` /
/// `.stl` / `.ttml` / `.dfxp` / `.xml`, confirmed by content).
pub fn detect(path: &str, bytes: &[u8]) -> Option<Format> {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("srt" | "vtt" | "scc" | "webvtt" | "mcc" | "stl" | "ttml" | "dfxp" | "xml")) {
        return None;
    }
    filmcraft_captions::detect(bytes, ext.as_deref())
}

/// Import a caption file as a new caption track in the active sequence (a new sequence is made
/// when none is open), snapped to the sequence's frames. One undo step.
pub fn import(s: &mut Session, path: &str, bytes: &[u8], format: Format, name: Option<&str>) -> Result<Value> {
    let mut doc = filmcraft_captions::parse(bytes, format).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let stem = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Captions".into());
    let track_name = name.map(str::to_string).unwrap_or_else(|| stem.clone());
    let seq_id = match s.state.active_sequence.filter(|id| s.project.sequence(*id).is_some()) {
        Some(id) => id,
        None => {
            let id = s.edit("New Sequence", |p, _| Ok(p.new_sequence(&stem, SequenceSettings::default(), 3, 3, None)))?;
            s.state.active_sequence = Some(id);
            s.state.open_sequences.push(id);
            id
        }
    };
    let rate = s.project.sequence(seq_id).map(|q| q.settings.frame_rate).unwrap_or_default();
    doc.snap_to_frames(rate);
    let n = doc.cues.len();
    let warnings = doc.warnings.clone();
    let tid = s.edit(&format!("Import Captions {stem}"), |p, st| {
        let tid = TrackId(p.alloc_id());
        let mut next = p.next_id;
        let mut alloc = || {
            let v = next;
            next += 1;
            v
        };
        let mut track = filmcraft_captions::track_from_document(&doc, tid, &track_name, format.track_format(), &mut alloc);
        if format == Format::Scc {
            track.style.background_color = [0, 0, 0, 255];
        }
        p.next_id = next;
        let q = p.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        q.caption_tracks.insert(0, track);
        q.check().map_err(EngineError::Other)?;
        st.caption_selection.clear();
        Ok(tid)
    })?;
    Ok(json!({"format": format.name(), "sequence": seq_id.0, "track": tid.0, "captions": n, "warnings": warnings}))
}

/// Delete captions (Clear / Ripple Delete with captions selected).
pub fn delete(s: &mut Session, ids: &[ClipId], ripple: bool) -> Result<Value> {
    if ids.is_empty() {
        return Err(bad("captions.delete", "no captions"));
    }
    let ids = ids.to_vec();
    let n = s.edit_sequence(if ripple { "Ripple Delete Captions" } else { "Delete Captions" }, |q, _, st| {
        let n = edit::captions::delete_captions(q, &ids, ripple)?;
        st.caption_selection.retain(|c| !ids.contains(c));
        Ok(n)
    })?;
    Ok(json!({"deleted": n}))
}

fn caption_json(s: &Session, c: &filmcraft_project::Caption) -> Value {
    let q = s.active_sequence();
    let (rate, df) = q.map(|q| (q.settings.frame_rate, q.settings.drop_frame)).unwrap_or_default();
    let tc = |t: Tick| format_time(t, rate, df, TimeDisplay::Timecode, 48_000);
    json!({
        "id": c.id.0,
        "start": c.start.0,
        "end": c.end().0,
        "in": tc(c.start),
        "out": tc(c.end()),
        "duration": tc(c.duration),
        "text": c.text,
        "speaker": c.speaker,
        "settings": c.settings,
        "style": c.style.as_deref().map(style_json),
    })
}

fn track_json(s: &Session, t: &CaptionTrack, index: usize) -> Value {
    json!({
        "id": t.id.0,
        "label": format!("C{}", index + 1),
        "name": t.name,
        "format": t.format.label(),
        "language": t.language,
        "enabled": t.enabled,
        "locked": t.locked,
        "syncLock": t.sync_lock,
        "style": style_json(&t.style),
        "captions": t.captions.iter().map(|c| caption_json(s, c)).collect::<Vec<_>>(),
    })
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_p(p, "path").ok_or_else(|| bad("captions.export", "need `path`"))?.to_string();
    let ext = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("");
    let format = str_p(p, "format").and_then(Format::from_name).or_else(|| Format::from_name(ext)).unwrap_or(Format::Srt);
    let tid = track_param(s, p).ok_or_else(|| bad("captions.export", "no caption track"))?;
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let track = seq.caption_track(tid).ok_or_else(|| bad("captions.export", "no such caption track"))?;
    let doc: Document = filmcraft_captions::document_from_track(track, Tick::ZERO);
    let df = bool_p(p, "dropFrame").unwrap_or(true);
    let bytes = filmcraft_captions::write(&doc, format, WriteOptions { drop_frame: df, rate: Some(seq.settings.frame_rate) });
    let n = doc.cues.len();
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"path": path, "format": format.name(), "captions": n, "bytes": bytes.len()}))
}

fn delta_p(s: &Session, p: &Value) -> Option<Tick> {
    if let Some(d) = p.get("delta").and_then(Value::as_i64) {
        return Some(Tick(d));
    }
    p.get("deltaFrames").and_then(Value::as_i64).map(|f| s.sequence_rate().tick_of(f))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        spec(
            "captions.newTrack",
            "Add New Caption Track…",
            &["Sequence", "Captions"],
            Some("Cmd+Alt+A"),
            r#"{"format":"Subtitle|CEA-608|CEA-708|Teletext","name":str?,"language":str?}"#,
            has_seq,
            |s, p| {
                let format = str_p(p, "format").and_then(CaptionFormat::from_name).unwrap_or_default();
                let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format.label().to_string());
                let lang = str_p(p, "language").unwrap_or("en").to_string();
                let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
                let id = s.edit("Add Caption Track", |pr, _| {
                    let id = TrackId(pr.alloc_id());
                    let mut t = CaptionTrack::new(id, name, format);
                    t.language = lang;
                    if format == CaptionFormat::Cea608 {
                        t.style.background_color = [0, 0, 0, 255];
                    }
                    pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?.caption_tracks.insert(0, t);
                    Ok(id)
                })?;
                Ok(json!({"track": id.0}))
            },
        ),
        spec("captions.deleteTrack", "Delete Caption Track", &[], None, r#"{"track":id|"C1"}"#, has_caption_track, |s, p| {
            let tid = track_param(s, p).ok_or_else(|| bad("captions.deleteTrack", "no such caption track"))?;
            s.edit_sequence("Delete Caption Track", |q, _, st| {
                q.caption_tracks.retain(|t| t.id != tid);
                st.caption_selection.retain(|c| q.find_caption(*c).is_some());
                Ok(())
            })?;
            Ok(Value::Null)
        }),
        spec(
            "captions.setTrack",
            "Caption Track Settings",
            &[],
            None,
            r#"{"track":id|"C1","name":str?,"format":str?,"language":str?,"enabled":bool?,"locked":bool?,"syncLock":bool?}"#,
            has_caption_track,
            |s, p| {
                let tid = track_param(s, p).ok_or_else(|| bad("captions.setTrack", "no such caption track"))?;
                let p = p.clone();
                s.edit("Caption Track Settings", |pr, st| {
                    let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
                    let t = pr.sequence_mut(seq).and_then(|q| q.caption_track_mut(tid)).ok_or(EngineError::NoSequence)?;
                    if let Some(n) = str_p(&p, "name") {
                        t.name = n.to_string();
                    }
                    if let Some(f) = str_p(&p, "format") {
                        t.format = CaptionFormat::from_name(f).ok_or_else(|| bad("captions.setTrack", "unknown format"))?;
                    }
                    if let Some(l) = str_p(&p, "language") {
                        t.language = l.to_string();
                    }
                    if let Some(b) = bool_p(&p, "enabled") {
                        t.enabled = b;
                    }
                    if let Some(b) = bool_p(&p, "locked") {
                        t.locked = b;
                    }
                    if let Some(b) = bool_p(&p, "syncLock") {
                        t.sync_lock = b;
                    }
                    Ok(())
                })?;
                Ok(Value::Null)
            },
        ),
        spec("captions.setStyle", "Caption Track Style", &[], None, STYLE_PARAMS, has_caption_track, set_style),
        spec(
            "captions.add",
            "Add Caption at Playhead",
            &["Sequence", "Captions"],
            Some("Cmd+Alt+C"),
            r#"{"track":id|"C1"?,"text":str?,"time":ticks?,"seconds":f64?,"durationSeconds":f64=3}"#,
            has_seq,
            |s, p| {
                // a caption track is created when the sequence has none
                if s.active_sequence().is_some_and(|q| q.caption_tracks.is_empty()) {
                    s.execute("captions.newTrack", json!({}))?;
                }
                let tid = track_param(s, p).ok_or_else(|| bad("captions.add", "no such caption track"))?;
                let t = time_p(s, p, "").unwrap_or(s.playhead());
                let rate = s.sequence_rate();
                let dur = Tick::from_seconds_f64(f64_p(p, "durationSeconds").unwrap_or(edit::captions::DEFAULT_SECONDS as f64));
                let dur = rate.snap_nearest(dur).max(rate.frame_duration());
                let text = str_p(p, "text").unwrap_or("New caption").to_string();
                let t = rate.snap(t);
                let id = s.edit_sequence("Add Caption", |q, ctx, st| {
                    let id = edit::captions::add_caption(q, tid, t, dur, &text, ctx)?;
                    st.caption_selection = vec![id];
                    st.selection.clear();
                    Ok(id)
                })?;
                Ok(json!({"caption": id.0}))
            },
        ),
        spec("captions.split", "Split Caption", &[], None, r#"{"caption":id?,"time":ticks?}"#, has_captions, |s, p| {
            let t = time_p(s, p, "").unwrap_or(s.playhead());
            let explicit = u64_p(p, "caption").map(ClipId);
            let ids = s.edit_sequence("Split Caption", |q, ctx, st| {
                let ids = match explicit {
                    Some(c) => vec![edit::captions::split_caption(q, c, t, ctx)?],
                    None => edit::captions::split_captions_at(q, &[], t, ctx),
                };
                if ids.is_empty() {
                    return Err(edit::EditError::Nothing.into());
                }
                st.caption_selection = ids.clone();
                Ok(ids)
            })?;
            Ok(json!({"captions": ids.iter().map(|c| c.0).collect::<Vec<_>>()}))
        }),
        spec("captions.merge", "Merge Captions", &[], None, r#"{"captions":[id]?}"#, has_captions, |s, p| {
            let ids = caption_ids(s, p);
            let id = s.edit_sequence("Merge Captions", |q, _, st| {
                let id = edit::captions::merge_captions(q, &ids)?;
                st.caption_selection = vec![id];
                Ok(id)
            })?;
            Ok(json!({"caption": id.0}))
        }),
        spec(
            "captions.splitLong",
            "Split Long Captions",
            &[],
            None,
            r#"{"captions":[id]? (default: the selection, else every caption of the track),"track":id|"C1"?,"maxChars":n=42 (a CEA-608 track: 32),"lines":n=2}"#,
            has_captions,
            split_long,
        ),
        spec(
            "captions.setText",
            "Edit Caption Text",
            &[],
            None,
            r#"{"caption":id?,"text":str?,"speaker":str|null?,"split":bool=true (text longer than two lines is split into captions that fit)}"#,
            has_captions,
            |s, p| {
                let id = one_caption(s, p, "captions.setText")?;
                let text = str_p(p, "text").map(str::to_string);
                let speaker: Option<Option<String>> = match p.get("speaker") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(v) => Some(v.as_str().map(str::to_string)),
                };
                if text.is_none() && speaker.is_none() {
                    return Err(bad("captions.setText", "need `text` or `speaker`"));
                }
                let split = bool_p(p, "split").unwrap_or(true);
                let frame = s.sequence_rate().frame_duration();
                let made = s.edit_sequence("Edit Caption", |q, ctx, _| {
                    edit::captions::set_caption(q, id, text.as_deref(), speaker.as_ref().map(|o| o.as_deref()))?;
                    let (max_chars, lines) = fit_of(q, id);
                    let long = q.find_caption(id).is_some_and(|(_, c)| edit::captions::too_long(&c.text, max_chars, lines));
                    // typed text that would not fit on screen: phrases that do (when there is time for them)
                    Ok(if split && text.is_some() && long {
                        edit::captions::split_long(q, id, max_chars, lines, frame, ctx).unwrap_or_default()
                    } else {
                        Vec::new()
                    })
                })?;
                Ok(if made.is_empty() { Value::Null } else { json!({"split": made.iter().map(|c| c.0).collect::<Vec<_>>()}) })
            },
        ),
        spec(
            "captions.setTimes",
            "Set Caption In/Out",
            &[],
            None,
            r#"{"caption":id?,"startTime|startFrame|startSeconds|startTimecode":…,"endTime|endFrame|endSeconds|endTimecode":…}"#,
            has_captions,
            |s, p| {
                let id = one_caption(s, p, "captions.setTimes")?;
                let cur = s
                    .active_sequence()
                    .and_then(|q| q.find_caption(id))
                    .map(|(_, c)| (c.start, c.end()))
                    .ok_or(EngineError::Edit(edit::EditError::NoItem(id)))?;
                let rate = s.sequence_rate();
                let start = time_p(s, p, "start").map(|t| rate.snap_nearest(t)).unwrap_or(cur.0);
                let end = time_p(s, p, "end").map(|t| rate.snap_nearest(t)).unwrap_or(cur.1);
                s.edit_sequence("Caption Timing", |q, ctx, _| Ok(edit::captions::set_caption_times(q, id, start, end, ctx)?))?;
                Ok(json!({"start": start.0, "end": end.0}))
            },
        ),
        spec("captions.trim", "Trim Caption", &[], None, r#"{"caption":id,"edge":"in|out","delta":ticks|"deltaFrames":i64}"#, has_captions, |s, p| {
            let id = one_caption(s, p, "captions.trim")?;
            let edge = if str_p(p, "edge") == Some("in") { Edge::In } else { Edge::Out };
            let d = delta_p(s, p).ok_or_else(|| bad("captions.trim", "need `delta` or `deltaFrames`"))?;
            let applied = s.edit_sequence("Trim Caption", |q, ctx, _| Ok(edit::captions::trim_caption(q, id, edge, d, ctx)?))?;
            Ok(json!({"applied": applied.0}))
        }),
        spec("captions.move", "Move Captions", &[], None, r#"{"captions":[id]?,"delta":ticks|"deltaFrames":i64}"#, has_captions, |s, p| {
            let ids = caption_ids(s, p);
            let d = delta_p(s, p).ok_or_else(|| bad("captions.move", "need `delta` or `deltaFrames`"))?;
            let applied = s.edit_sequence("Move Captions", |q, _, _| Ok(edit::captions::move_captions(q, &ids, d)?))?;
            Ok(json!({"applied": applied.0}))
        }),
        spec("captions.delete", "Delete Captions", &[], None, r#"{"captions":[id]?,"ripple":bool=false}"#, has_captions, |s, p| {
            let ids = caption_ids(s, p);
            delete(s, &ids, bool_p(p, "ripple").unwrap_or(false))
        }),
        spec("captions.select", "Select Captions", &[], None, r#"{"captions":[id],"add":bool?}"#, has_seq, |s, p| {
            let ids: Vec<ClipId> =
                p.get("captions").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect()).unwrap_or_default();
            let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
            let ids: Vec<ClipId> = ids.into_iter().filter(|c| seq.find_caption(*c).is_some()).collect();
            if bool_p(p, "add").unwrap_or(false) {
                for c in ids {
                    if !s.state.caption_selection.contains(&c) {
                        s.state.caption_selection.push(c);
                    }
                }
            } else {
                s.state.caption_selection = ids;
                s.state.selection.clear();
            }
            Ok(json!({"captions": s.state.caption_selection.iter().map(|c| c.0).collect::<Vec<_>>()}))
        }),
        spec("captions.goTo", "Go to Caption", &[], None, r#"{"caption":id}"#, has_captions, |s, p| {
            let id = one_caption(s, p, "captions.goTo")?;
            let start = s.active_sequence().and_then(|q| q.find_caption(id)).map(|(_, c)| c.start).ok_or(EngineError::Edit(edit::EditError::NoItem(id)))?;
            s.set_playhead(start);
            s.state.caption_selection = vec![id];
            s.state.selection.clear();
            Ok(json!({"time": start.0}))
        }),
        spec("captions.next", "Go to Next Caption Segment", &["Sequence", "Captions"], Some("Cmd+Alt+Down"), "{}", has_captions, |s, _| step(s, true)),
        spec("captions.previous", "Go to Previous Caption Segment", &["Sequence", "Captions"], Some("Cmd+Alt+Up"), "{}", has_captions, |s, _| step(s, false)),
        spec("captions.showAll", "Show All Caption Tracks", &["Sequence", "Captions"], None, "{}", has_caption_track, |s, _| show_all(s, true)),
        spec("captions.hideAll", "Hide All Caption Tracks", &["Sequence", "Captions"], None, "{}", has_caption_track, |s, _| show_all(s, false)),
        spec("captions.import", "Import Captions…", &[], None, r#"{"path":str,"name":str?}"#, always, |s, p| {
            let path = str_p(p, "path").ok_or_else(|| bad("captions.import", "need `path`"))?.to_string();
            let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            let ext = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            let format = filmcraft_captions::detect(&bytes, ext.as_deref()).ok_or_else(|| EngineError::Other(format!("{path}: not a caption file")))?;
            import(s, &path, &bytes, format, str_p(p, "name"))
        }),
        spec(
            "captions.export",
            "Captions…",
            &["File", "Export"],
            None,
            r#"{"path":str,"format":"srt|vtt|scc|mcc|stl|ttml|dfxp"? (default: from the extension),"track":id|"C1"?,"dropFrame":bool=true}"#,
            has_caption_track,
            export,
        ),
        CommandSpec {
            id: "captions.list",
            label: "List Captions",
            menu: &[],
            shortcut: None,
            params: r#"{"track":id|"C1"?}"#,
            enabled: always,
            run: |s, p| {
                let Some(seq) = s.active_sequence() else { return Ok(json!({"tracks": []})) };
                // a `track` that names no caption track is an error, never every track
                let only = match p.get("track") {
                    Some(_) => Some(track_param(s, p).ok_or_else(|| bad("captions.list", "no such caption track"))?),
                    None => None,
                };
                let tracks: Vec<Value> =
                    seq.caption_tracks.iter().enumerate().filter(|(_, t)| only.is_none_or(|o| o == t.id)).map(|(i, t)| track_json(s, t, i)).collect();
                Ok(json!({"tracks": tracks, "selection": s.state.caption_selection.iter().map(|c| c.0).collect::<Vec<_>>()}))
            },
            journal: false,
        },
    ]
}

fn step(s: &mut Session, forward: bool) -> Result<Value> {
    let t = s.playhead();
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let mut starts: Vec<(Tick, ClipId)> = seq.caption_tracks.iter().filter(|t| t.enabled).flat_map(|t| t.captions.iter().map(|c| (c.start, c.id))).collect();
    starts.sort();
    let hit = if forward { starts.iter().find(|(st, _)| *st > t).copied() } else { starts.iter().rev().find(|(st, _)| *st < t).copied() };
    let (time, id) = hit.ok_or_else(|| EngineError::Other("no more captions".into()))?;
    s.set_playhead(time);
    s.state.caption_selection = vec![id];
    Ok(json!({"time": time.0, "caption": id.0}))
}

fn show_all(s: &mut Session, on: bool) -> Result<Value> {
    s.edit(if on { "Show All Caption Tracks" } else { "Hide All Caption Tracks" }, |p, st| {
        let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
        for t in &mut p.sequence_mut(seq).ok_or(EngineError::NoSequence)?.caption_tracks {
            t.enabled = on;
        }
        Ok(())
    })?;
    Ok(Value::Null)
}

#[cfg(test)]
#[path = "captions_tests.rs"]
mod tests;
