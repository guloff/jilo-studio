//! Shorts from a long recording, the steps of Cut into Shorts (AI ▸ Automatic Editing): the
//! highlights the text engine marked (`highlights.find`) become shorts — a sequence per highlight,
//! captions from the transcript in a chosen look, a vertical copy whose clips follow the speaker's
//! face — and, if asked, the vertical copies go to the export queue. One undo step.
//!
//! | Command | |
//! |---|---|
//! | `shorts.fromHighlights` | `{markers, aspect="9:16", follow="face"\|"center", captions=preset\|null, export=false, preset?, path?}` |
//! | `sequence.makeVertical` | the open sequence as 9:16, following the face where Apple Vision finds one (AI ▸ Make Vertical) |

use filmcraft_project::ItemId;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, has_seq, str_p};
use crate::{EngineError, Result, Session};

/// Run `cmd` on sequence `seq` (made active for it), then put the open sequences back.
fn on(s: &mut Session, seq: ItemId, cmd: &str, p: Value) -> Result<Value> {
    let (active, open) = (s.state.active_sequence, s.state.open_sequences.clone());
    s.state.active_sequence = Some(seq);
    if !s.state.open_sequences.contains(&seq) {
        s.state.open_sequences.push(seq);
    }
    let r = s.execute(cmd, p);
    s.state.active_sequence = active;
    s.state.open_sequences = open;
    r
}

fn from_highlights(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "shorts.fromHighlights";
    let markers = p
        .get("markers")
        .filter(|m| m.as_array().is_some_and(|a| !a.is_empty()))
        .cloned()
        .ok_or_else(|| bad(cmd, "`markers`: the highlights to make shorts of"))?;
    let aspect = str_p(p, "aspect").unwrap_or("9:16").to_string();
    let follow = str_p(p, "follow").unwrap_or(if s.face_detector.is_some() { "face" } else { "center" }).to_string();
    let look = match p.get("captions") {
        Some(Value::Null) => None,
        Some(v) => Some(v.as_str().ok_or_else(|| bad(cmd, "`captions`: a caption style preset id, or null"))?.to_string()),
        None => Some("karaoke-yellow".to_string()),
    };
    let export = bool_p(p, "export").unwrap_or(false);
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let (active, open) = (s.state.active_sequence, s.state.open_sequences.clone());
    let run = |s: &mut Session| -> Result<Vec<Value>> {
        let made = s.execute("highlights.assemble", json!({"markers": markers, "as": "subsequences"}))?;
        let seqs: Vec<ItemId> = made["sequences"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(ItemId).collect();
        let mut out = Vec::new();
        for q in seqs {
            // captions from what is said, in the chosen look
            let mut captions = 0;
            if let Some(look) = &look
                && let Ok(r) = on(s, q, "transcript.createCaptions", json!({}))
            {
                captions = r["captions"].as_u64().unwrap_or(0);
                on(s, q, "captions.applyStylePreset", json!({"preset": look, "track": "all"}))?;
            }
            let v = on(s, q, "sequence.makeFormat", json!({"aspect": aspect, "follow": follow}))?;
            let vertical = v["sequence"].as_u64().ok_or_else(|| EngineError::Other("the vertical copy was not made".into()))?;
            let mut queued = Value::Null;
            if export {
                let mut e = json!({"sequence": vertical});
                for k in ["preset", "path"] {
                    if let Some(x) = p.get(k) {
                        e[k] = x.clone();
                    }
                }
                queued = s.execute("export.queue.add", e)?["added"].clone();
            }
            let name = s.project.item(ItemId(vertical)).map(|i| i.name.clone()).unwrap_or_default();
            out.push(json!({"sequence": q.0, "vertical": vertical, "name": name, "captions": captions, "queued": queued}));
        }
        Ok(out)
    };
    match run(s) {
        Ok(out) => {
            crate::recipes::collapse_undo(s, before, "Cut into Shorts");
            // the first short is opened
            s.state.active_sequence = active;
            s.state.open_sequences = open;
            if let Some(first) = out.first().and_then(|o| o["vertical"].as_u64()) {
                let _ = s.execute("sequence.open", json!({"item": first}));
            }
            Ok(json!({"shorts": out}))
        }
        Err(e) => {
            s.revert_to(mark);
            s.state.active_sequence = active;
            s.state.open_sequences = open;
            Err(e)
        }
    }
}

fn make_vertical(s: &mut Session, _: &Value) -> Result<Value> {
    let follow = if s.face_detector.is_some() { "face" } else { "center" };
    s.execute("sequence.makeFormat", json!({"aspect": "9:16", "follow": follow}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "shorts.fromHighlights",
            label: "Shorts from Highlights",
            menu: &[],
            shortcut: None,
            params: r#"{"markers":[id],"aspect":"9:16"|"1:1"|"4:5"?,"follow":"face|center"?,"captions":preset|null (default karaoke-yellow),"export":bool?,"preset":str?,"path":dir?}"#,
            enabled: has_seq,
            run: from_highlights,
            journal: true,
        },
        CommandSpec {
            id: "sequence.makeVertical",
            label: "Make Vertical",
            menu: crate::ai_menu::EDITING,
            shortcut: None,
            params: "{}",
            enabled: has_seq,
            run: make_vertical,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_time::Tick;

    #[test]
    fn highlights_become_vertical_shorts_with_captions() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let seq = s.state.active_sequence.unwrap();
        // two highlights marked by hand (as highlights.find marks them)
        let mut ids = Vec::new();
        for (a, b) in [(1.0, 4.0), (10.0, 13.0)] {
            let frames = s.sequence_rate().frame_at(Tick::from_seconds_f64(b - a));
            let r = s.execute("markers.add", json!({"time": Tick::from_seconds_f64(a).0, "durationFrames": frames, "name": format!("Moment {a}")})).unwrap();
            ids.push(r["marker"].as_u64().unwrap());
        }
        let undo = s.history.undo.len();
        let r = s.execute("shorts.fromHighlights", json!({"markers": ids, "follow": "center", "export": true})).unwrap();
        let shorts = r["shorts"].as_array().unwrap();
        assert_eq!(shorts.len(), 2, "{r}");
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
        for sh in shorts {
            let v = s.project.sequence(ItemId(sh["vertical"].as_u64().unwrap())).unwrap();
            assert_eq!((v.settings.width, v.settings.height), (1080, 1920), "vertical");
            assert!(v.duration() <= Tick::from_seconds_f64(3.1), "the highlight's length");
        }
        assert_eq!(s.state.active_sequence.map(|x| x.0), shorts[0]["vertical"].as_u64(), "the first short is open");
        assert_ne!(s.state.active_sequence, Some(seq));
        assert!(s.export_queue.items.len() >= 2, "queued");
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.project.sequence(ItemId(shorts[0]["vertical"].as_u64().unwrap())).is_none(), "undone at once");
        assert!(s.execute("shorts.fromHighlights", json!({"markers": []})).is_err());
    }
}
