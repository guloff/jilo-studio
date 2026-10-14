//! Other formats of a sequence: a copy in 9:16 (shorts, reels), 1:1, 4:5 or 16:9 whose video
//! clips fill the new frame — centred, or following the speaker's face where Apple Vision finds one
//! (`clip.reframeToFace`) — and every format exported in one go.
//!
//! | Command | |
//! |---|---|
//! | `sequence.makeFormat` | `{aspect, follow?: "face" \| "center", name?}`: the copy (one undo step), opened |
//! | `export.allFormats` | `{aspects: [..], follow?, preset?, path?, start?}`: the open sequence and a copy per other aspect into the export queue |
//!
//! Captions and their styles carry over (they are placed by fractions of the frame); graphics keep
//! their positions in frame pixels and may need a look in the new frame.

use serde_json::{Value, json};

use filmcraft_project::ItemId;

use crate::commands::{CommandSpec, bad, has_seq, str_p};
use crate::{EngineError, Result, Session};

/// The supported aspects: (name, width, height) for a frame whose short side is `short`.
pub fn frame_for(aspect: &str, short: u32) -> Option<(u32, u32)> {
    let s = short.max(2) & !1;
    match aspect {
        "16:9" => Some(((s * 16 / 9) & !1, s)),
        "9:16" => Some((s, (s * 16 / 9) & !1)),
        "1:1" => Some((s, s)),
        "4:5" => Some((s, (s * 5 / 4) & !1)),
        _ => None,
    }
}

/// The aspect name closest to a frame.
pub fn aspect_of(w: u32, h: u32) -> &'static str {
    let r = w as f64 / h.max(1) as f64;
    [("16:9", 16.0 / 9.0), ("9:16", 9.0 / 16.0), ("1:1", 1.0), ("4:5", 0.8)]
        .into_iter()
        .min_by(|a, b| (a.1 - r).abs().total_cmp(&(b.1 - r).abs()))
        .map_or("16:9", |a| a.0)
}

/// Make the copy; returns its item id. The project returns to how it was if a step fails.
fn make(s: &mut Session, aspect: &str, follow: &str, name: Option<&str>) -> Result<ItemId> {
    let cmd = "sequence.makeFormat";
    let src = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let short = q.settings.width.min(q.settings.height);
    let (w, h) = frame_for(aspect, short).ok_or_else(|| bad(cmd, format!("`aspect`: 16:9, 9:16, 1:1 or 4:5, not `{aspect}`")))?;
    let base = s.project.item(src).map(|i| i.name.clone()).unwrap_or_default();
    let new_name = name.map(str::to_string).unwrap_or_else(|| format!("{base} {aspect}"));
    let selection = s.state.project_selection.clone();
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let r = (|| -> Result<ItemId> {
        s.execute("project.select", json!({"items": [src.0]}))?;
        let dup = s.execute("edit.duplicate", json!({}))?;
        let id = dup["items"][0].as_u64().map(ItemId).ok_or_else(|| EngineError::Other("the sequence could not be copied".into()))?;
        s.execute("sequence.open", json!({"item": id.0}))?;
        s.execute("sequence.settings", json!({"width": w, "height": h, "name": new_name}))?;
        // the video clips (not graphics) fill the new frame
        let clips: Vec<u64> = s
            .active_sequence()
            .map(|q| {
                q.video_tracks
                    .iter()
                    .flat_map(|t| t.items.iter())
                    .filter(|it| !matches!(s.project.item(it.item).map(|i| &i.kind), Some(filmcraft_project::ItemKind::Graphic { .. })))
                    .map(|it| it.id.0)
                    .collect()
            })
            .unwrap_or_default();
        if !clips.is_empty() {
            s.execute("clip.fillFrame", json!({"clips": clips}))?;
            if follow == "face" && s.face_detector.is_some() {
                // clips without a face stay centred
                let _ = s.execute("clip.reframeToFace", json!({"clips": clips, "target": "speaker"}));
            }
        }
        Ok(id)
    })();
    s.state.project_selection = selection;
    match r {
        Ok(id) => {
            crate::recipes::collapse_undo(s, before, &format!("Make {aspect} Copy"));
            Ok(id)
        }
        Err(e) => {
            // take back what was done
            s.revert_to(mark);
            if s.project.sequence(src).is_some() {
                s.state.active_sequence = Some(src);
            }
            Err(e)
        }
    }
}

fn make_format(s: &mut Session, p: &Value) -> Result<Value> {
    let aspect = str_p(p, "aspect").unwrap_or("9:16");
    let follow = str_p(p, "follow").unwrap_or("face");
    let id = make(s, aspect, follow, str_p(p, "name"))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    Ok(json!({"sequence": id.0, "width": q.settings.width, "height": q.settings.height}))
}

fn all_formats(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "export.allFormats";
    let src = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let own = aspect_of(q.settings.width, q.settings.height);
    let aspects: Vec<String> = match p.get("aspects").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => vec!["16:9".into(), "9:16".into(), "1:1".into()],
    };
    if aspects.is_empty() {
        return Err(bad(cmd, "`aspects`: at least one of 16:9, 9:16, 1:1, 4:5"));
    }
    let follow = str_p(p, "follow").unwrap_or("face");
    let mut seqs = Vec::new();
    for a in &aspects {
        if a == own {
            seqs.push(src);
            continue;
        }
        s.execute("sequence.open", json!({"item": src.0}))?;
        seqs.push(make(s, a, follow, None)?);
    }
    s.execute("sequence.open", json!({"item": src.0}))?;
    let mut q = json!({"sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>()});
    for k in ["preset", "path"] {
        if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
            q[k] = v.clone();
        }
    }
    let queued = s.execute("export.queue.add", q)?;
    if p.get("start").and_then(Value::as_bool).unwrap_or(true) {
        s.execute("export.queue.start", json!({}))?;
    }
    Ok(json!({"sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>(), "aspects": aspects, "queue": queued}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "sequence.makeFormat",
            label: "Make a Copy in Another Format",
            menu: &[],
            shortcut: None,
            params: r#"{"aspect":"9:16|1:1|4:5|16:9","follow":"face|center"="face","name":str?}"#,
            enabled: has_seq,
            run: make_format,
            journal: true,
        },
        CommandSpec {
            id: "export.allFormats",
            label: "Export in All Formats…",
            menu: &["File", "Export"],
            shortcut: None,
            params: r#"{"aspects":["16:9"|"9:16"|"1:1"|"4:5"]=["16:9","9:16","1:1"],"follow":"face|center"="face","preset":str?,"path":dir?,"start":bool=true}"#,
            enabled: has_seq,
            run: all_formats,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_for_each_aspect() {
        assert_eq!(frame_for("9:16", 1080), Some((1080, 1920)));
        assert_eq!(frame_for("16:9", 1080), Some((1920, 1080)));
        assert_eq!(frame_for("1:1", 1080), Some((1080, 1080)));
        assert_eq!(frame_for("4:5", 1080), Some((1080, 1350)));
        assert_eq!(frame_for("3:2", 1080), None);
        assert_eq!(aspect_of(1920, 1080), "16:9");
        assert_eq!(aspect_of(1080, 1920), "9:16");
    }

    #[test]
    fn a_vertical_copy_fills_its_frame_in_one_undo_step() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let src = s.state.active_sequence.unwrap();
        let n_items = s.project.items.len();
        let undo = s.history.undo.len();
        let r = s.execute("sequence.makeFormat", json!({"aspect": "9:16", "follow": "center"})).unwrap();
        assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1080), Some(1920)));
        let copy = ItemId(r["sequence"].as_u64().unwrap());
        assert_ne!(copy, src);
        assert_eq!(s.state.active_sequence, Some(copy), "the copy is open");
        assert_eq!(s.project.items.len(), n_items + 1);
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
        // a 1920x1080 clip covers 1080x1920: scaled up about 3.16x (1920/1080 ÷ 1080/1920)
        let q = s.active_sequence().unwrap();
        let it = q.video_tracks[0]
            .items
            .iter()
            .find(|it| !matches!(s.project.item(it.item).map(|i| &i.kind), Some(filmcraft_project::ItemKind::Graphic { .. })))
            .unwrap();
        let scale = it.effect("motion").unwrap().f64_at("scale", it.effect_time_at(it.start));
        assert!(scale > 150.0, "fills the vertical frame: {scale}");
        // the original is untouched; undo takes the copy away
        let orig = s.project.sequence(src).unwrap();
        assert_eq!((orig.settings.width, orig.settings.height), (1920, 1080));
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.project.item(copy).is_none());
        assert!(s.execute("sequence.makeFormat", json!({"aspect": "2:1"})).is_err());
        assert_eq!(s.project.items.len(), n_items, "a failed copy leaves nothing behind");
    }

    #[test]
    fn all_formats_queue_the_sequence_and_a_copy_per_other_aspect() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let src = s.state.active_sequence.unwrap();
        let undo = s.history.undo.len();
        let r = s
            .execute("export.allFormats", json!({"aspects": ["16:9", "9:16", "1:1"], "follow": "center", "preset": "Waveform Audio 48 kHz 16-bit", "path": "formats-output/", "start": false}))
            .unwrap();
        let seqs: Vec<u64> = r["sequences"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        assert_eq!(seqs.len(), 3);
        assert_eq!(seqs[0], src.0, "the open sequence is already 16:9");
        let frames: Vec<(u32, u32)> = seqs.iter().map(|&i| s.project.sequence(ItemId(i)).map(|q| (q.settings.width, q.settings.height)).unwrap()).collect();
        assert_eq!(frames, [(1920, 1080), (1080, 1920), (1080, 1080)]);
        assert_eq!(s.export_queue.items.len(), 3);
        assert!(!s.export_queue.running, "start: false leaves the queue waiting");
        assert_eq!(s.state.active_sequence, Some(src), "the original stays open");
        assert_eq!(s.history.undo.len(), undo + 2, "one undo step per copy");
        assert!(s.execute("export.allFormats", json!({"aspects": []})).is_err());
    }
}
