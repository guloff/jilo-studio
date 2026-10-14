//! Paste media from the system clipboard (#611). The desktop app reads the clipboard: copied files
//! paste by path, and an image is first saved by [`save_pasted_image`] as `Pasted Image <n>.png`
//! where voice-over takes go (Scratch Disks ▸ Captured Audio and Video, else next to the project).
//!
//! `edit.pasteMedia` imports the files and puts them on the Timeline at the playhead, end to end,
//! without overwriting anything: each clip goes on the first free track from the targeted one
//! outward (up for video, down for audio), and a track is added when every track is busy there.
//! With `insert` they are inserted on the targeted tracks instead, as Paste Insert does.

use filmcraft_project::{BinId, ClipId, ItemId, TrackId, TrackKind};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, bool_p, u64_p};
use crate::{EngineError, Result, Session};

/// Write a pasted image (PNG bytes) to a new `Pasted Image <n>.png`; returns its path.
pub fn save_pasted_image(s: &mut Session, png: &[u8]) -> Result<String> {
    let dir = crate::voiceover::captured_dir(s, "Pasted Images");
    let names: std::collections::HashSet<String> = s.project.items.values().map(|i| i.name.clone()).collect();
    let mut k = 1;
    let path = loop {
        let file = format!("Pasted Image {k}.png");
        let path = std::path::Path::new(&dir).join(&file).to_string_lossy().into_owned();
        if !names.contains(&file) && s.services.file_size(&path).is_err() {
            break path;
        }
        k += 1;
    };
    if !cfg!(target_arch = "wasm32") {
        std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(format!("{dir}: {e}")))?;
    }
    s.services.write_file(&path, png).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(path)
}

fn paste_media(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "edit.pasteMedia";
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    if paths.is_empty() {
        return Err(bad(cmd, "need `paths`"));
    }
    let place = bool_p(p, "place").unwrap_or(true);
    if place && s.active_sequence().is_none() {
        return Err(EngineError::NoSequence);
    }
    let insert = bool_p(p, "insert").unwrap_or(false);
    let n0 = s.history.undo.len();
    let r = import_and_place(s, &paths, u64_p(p, "bin").map(BinId), place, insert);
    crate::clip_ops::collapse_history(s, n0, if insert { "Paste Insert" } else { "Paste" });
    r
}

fn import_and_place(s: &mut Session, paths: &[String], bin: Option<BinId>, place: bool, insert: bool) -> Result<Value> {
    let mut import = json!({"paths": paths});
    if let Some(b) = bin {
        import["bin"] = json!(b.0);
    }
    let imported = s.execute("file.import", import)?;
    // in the order pasted: new items, and files already in the project (`duplicates`) as their item
    let mut added = imported["items"].as_array().into_iter().flatten().filter_map(Value::as_u64).map(ItemId);
    let row_path = |v: &Value| v["path"].as_str().map(str::to_string);
    let rows = |k: &str| imported[k].as_array().cloned().unwrap_or_default();
    let (duplicates, relink) = (rows("duplicates"), rows("relink"));
    let mut items: Vec<ItemId> = Vec::new();
    for path in paths {
        if let Some(d) = duplicates.iter().find(|d| row_path(d).as_deref() == Some(path.as_str())) {
            items.extend(d["item"].as_u64().map(ItemId));
        } else if !relink.iter().any(|r| row_path(r).as_deref() == Some(path.as_str())) {
            items.extend(added.next());
        }
    }
    items.extend(added);
    if !place {
        return Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "clips": []}));
    }
    let mut at = s.playhead();
    let mut clips: Vec<ClipId> = Vec::new();
    for item in &items {
        let Some(pi) = s.project.item(*item) else { continue };
        let (dur, video, audio) = (crate::commands::full_duration(s, pi), pi.has_video(), pi.has_audio());
        let mut params = json!({"item": item.0, "time": at.0, "insert": insert});
        if !insert {
            if video {
                params["track"] = json!(free_track(s, TrackKind::Video, at, at + dur)?.0);
            }
            if audio {
                params["audioTrack"] = json!(free_track(s, TrackKind::Audio, at, at + dur)?.0);
            }
        }
        let placed = s.execute("timeline.place", params)?;
        let ids: Vec<ClipId> = placed["clips"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect()).unwrap_or_default();
        let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
        at = ids.iter().filter_map(|c| seq.find_item(*c)).map(|(_, it)| it.end()).max().unwrap_or(at + dur);
        clips.extend(ids);
    }
    // the playhead goes to the end of what was pasted, as after Paste
    s.set_playhead(at);
    s.state.selection = clips.clone();
    Ok(json!({"items": items.iter().map(|i| i.0).collect::<Vec<_>>(), "clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

/// The first unlocked track of `kind` from the targeted one outward with nothing in `start..end`.
/// When every track is busy there, a new one is added (on top for video, at the bottom for audio).
pub(crate) fn free_track(s: &mut Session, kind: TrackKind, start: Tick, end: Tick) -> Result<TrackId> {
    let seq = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let tg = s.targeting();
    let (tracks, dest) = match kind {
        TrackKind::Video => (&seq.video_tracks, tg.video_dest),
        TrackKind::Audio => (&seq.audio_tracks, tg.audio_dest),
    };
    let from = dest.and_then(|d| tracks.iter().position(|t| t.id == d)).unwrap_or(0);
    let free = tracks.iter().skip(from).find(|t| !t.locked && !t.items.iter().any(|i| i.start < end && i.end() > start)).map(|t| t.id);
    if let Some(t) = free {
        return Ok(t);
    }
    let (params, key) = match kind {
        TrackKind::Video => (json!({"video": 1}), "video"),
        TrackKind::Audio => (json!({"video": 0, "audio": 1}), "audio"),
    };
    let added = s.execute("sequence.addTracks", params)?;
    added[key][0].as_u64().map(TrackId).ok_or_else(|| EngineError::Other("no track was added".into()))
}

/// `timeline.addAtPlayhead`: a project item on the timeline at the playhead (or `time`), on free
/// tracks (new ones when every track is busy there), selected. The Library's double-click.
fn add_at_playhead(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "timeline.addAtPlayhead";
    let item = p.get("item").and_then(Value::as_u64).map(ItemId).ok_or_else(|| crate::commands::bad(cmd, "`item`"))?;
    let pi = s.project.item(item).ok_or_else(|| crate::commands::bad(cmd, format!("no item {}", item.0)))?;
    let (dur, video, audio) = (crate::commands::full_duration(s, pi), pi.has_video(), pi.has_audio());
    let at = p.get("time").and_then(Value::as_i64).map(Tick).unwrap_or(s.playhead());
    let mut params = json!({"item": item.0, "time": at.0, "insert": false});
    if video {
        params["track"] = json!(free_track(s, TrackKind::Video, at, at + dur)?.0);
    }
    if audio {
        params["audioTrack"] = json!(free_track(s, TrackKind::Audio, at, at + dur)?.0);
    }
    let placed = s.execute("timeline.place", params)?;
    let clips: Vec<ClipId> = placed["clips"].as_array().map(|a| a.iter().filter_map(Value::as_u64).map(ClipId).collect()).unwrap_or_default();
    s.state.selection = clips.clone();
    Ok(json!({"clips": clips.iter().map(|c| c.0).collect::<Vec<_>>()}))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "timeline.addAtPlayhead",
            label: "Add to Timeline at Playhead",
            menu: &[],
            shortcut: None,
            params: r#"{"item":id,"time":ticks?}"#,
            enabled: crate::commands::has_seq,
            run: add_at_playhead,
            journal: true,
        },
        CommandSpec {
            id: "edit.pasteMedia",
            label: "Paste Media",
            menu: &[],
            shortcut: None,
            params: r#"{"paths":[str],"insert":bool?,"place":bool=true,"bin":binId?}"#,
            enabled: always,
            run: paste_media,
            journal: true,
        },
    ]
}
