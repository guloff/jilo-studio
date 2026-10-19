use serde_json::json;

use crate::Session;
use filmcraft_project::ClipId;
use filmcraft_time::{TICKS_PER_SECOND, Tick};

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

#[test]
fn captions_import_edit_export_and_burn_in() {
    let dir = std::env::temp_dir().join(format!("fc-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let srt = dir.join("talk.srt");
    std::fs::write(
        &srt,
        "\u{feff}1\r\n00:00:00,500 --> 00:00:02,000\r\nHello there\r\n\r\n2\r\n00:00:02,500 --> 00:00:04,000\r\nSecond line\r\nwith two rows\r\n",
    )
    .unwrap();
    let mut s = demo();
    let r = s.execute("file.import", json!({"paths": [srt.to_string_lossy()]})).unwrap();
    assert_eq!(r["documents"][0]["captions"], 2, "{r}");
    let q = s.active_sequence().unwrap();
    let rate = q.settings.frame_rate;
    let tr = q.caption_tracks[0].clone();
    assert_eq!(tr.name, "talk");
    assert_eq!(tr.captions[0].start, rate.snap_nearest(Tick(TICKS_PER_SECOND / 2)), "snapped to frames");
    let first = tr.captions[0].id;
    let second = tr.captions[1].id;

    // edit text, split, merge, trim, move, add at playhead, delete — each one undoable
    s.execute("captions.setText", json!({"caption": first.0, "text": "Hi!", "speaker": "Ann"})).unwrap();
    let split_at = rate.tick_of(rate.frame_at(tr.captions[1].start) + 10);
    let r = s.execute("captions.split", json!({"caption": second.0, "time": split_at.0})).unwrap();
    let right = ClipId(r["captions"][0].as_u64().unwrap());
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions.len(), 3);
    s.execute("captions.merge", json!({"captions": [second.0, right.0]})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions.len(), 2);
    s.execute("captions.trim", json!({"caption": second.0, "edge": "out", "deltaFrames": 12})).unwrap();
    s.execute("captions.move", json!({"captions": [second.0], "deltaFrames": 24})).unwrap();
    s.execute("playhead.set", json!({"seconds": 8.0})).unwrap();
    let r = s.execute("captions.add", json!({"text": "Added"})).unwrap();
    let added = ClipId(r["caption"].as_u64().unwrap());
    assert_eq!(s.state.caption_selection, vec![added]);
    s.execute("edit.clear", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().find_caption(added).is_none(), "Clear deletes selected captions");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().find_caption(added).is_some());
    let list = s.execute("captions.list", json!({})).unwrap();
    assert_eq!(list["tracks"][0]["captions"][0]["text"], "Hi!");
    assert_eq!(list["tracks"][0]["captions"][0]["speaker"], "Ann");
    s.active_sequence().unwrap().check().unwrap();

    // export in all three formats and read back: same texts
    for ext in ["srt", "vtt", "scc"] {
        let path = dir.join(format!("out.{ext}")).to_string_lossy().to_string();
        let r = s.execute("captions.export", json!({"path": path})).unwrap();
        assert_eq!(r["captions"], 3, "{ext}: {r}");
        let back = filmcraft_captions::parse(&std::fs::read(&path).unwrap(), filmcraft_captions::Format::from_name(ext).unwrap()).unwrap();
        let texts: Vec<String> = back.cues.iter().map(|c| filmcraft_project::plain_text(&c.text)).collect();
        assert_eq!(texts.len(), 3, "{ext}");
        assert_eq!(texts[0], "Hi!", "{ext}");
    }

    // the program renders captions; hiding the track removes them
    s.execute("playhead.set", json!({"seconds": 1.0})).unwrap();
    let with = s.render_program(0.25).unwrap();
    s.execute("captions.hideAll", json!({})).unwrap();
    let without = s.render_program(0.25).unwrap();
    let diff = with.px.iter().zip(&without.px).filter(|(a, b)| (*a - *b).abs() > 0.05).count();
    assert!(diff > 200, "captions change the picture ({diff} samples)");
    let half = with.w * 4 * (with.h / 2);
    let top_diff = with.px[..half].iter().zip(&without.px[..half]).filter(|(a, b)| (*a - *b).abs() > 0.05).count();
    assert_eq!(top_diff, 0, "bottom-anchored captions stay in the lower half");
    s.execute("captions.showAll", json!({})).unwrap();
    let plan = filmcraft_render::plan::plan_frame(
        &s.project,
        s.state.active_sequence.unwrap(),
        s.playhead(),
        filmcraft_render::RenderOptions { scale: 0.25, captions: true, ..Default::default() },
        &s.media.provider(s.project.clone(), s.services.clone()),
    );
    let cpu = filmcraft_render::plan::execute_cpu(&plan);
    let pd = cpu.px.iter().zip(&with.px).filter(|(a, b)| (*a - *b).abs() > 0.02).count();
    assert!(pd < cpu.px.len() / 200, "the GPU plan matches the CPU render ({pd})");

    // save/open keeps caption tracks
    let proj = dir.join("cap.fcproj").to_string_lossy().to_string();
    s.execute("file.saveAs", json!({"path": proj})).unwrap();
    let before = s.active_sequence().unwrap().caption_tracks.clone();
    s.execute("file.open", json!({"path": proj})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks, before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn caption_track_follows_sync_locked_insert() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({"format": "CEA-608"})).unwrap();
    s.execute("playhead.set", json!({"seconds": 4.0})).unwrap();
    s.execute("captions.add", json!({"text": "later"})).unwrap();
    let start = s.active_sequence().unwrap().caption_tracks[0].captions[0].start;
    let item = s.project.items.values().find(|i| i.as_media().is_some_and(|m| m.info.has_video())).unwrap().id;
    s.execute("source.open", json!({"item": item.0})).unwrap();
    s.execute("playhead.set", json!({"seconds": 0.0})).unwrap();
    s.execute("source.insert", json!({})).unwrap();
    let after = s.active_sequence().unwrap().caption_tracks[0].captions[0].start;
    assert!(after > start, "caption moved right with the insert");
    s.active_sequence().unwrap().check().unwrap();
}

#[test]
fn caption_commands_need_tracks_and_navigate() {
    let mut s = demo();
    assert!(s.execute("captions.split", json!({})).is_err(), "disabled without captions");
    for (sec, text) in [(1.0, "one"), (3.0, "two"), (6.0, "three")] {
        s.execute("playhead.set", json!({"seconds": sec})).unwrap();
        s.execute("captions.add", json!({"text": text, "durationSeconds": 1.0})).unwrap();
    }
    s.execute("playhead.set", json!({"seconds": 0.0})).unwrap();
    s.execute("captions.next", json!({})).unwrap();
    s.execute("captions.next", json!({})).unwrap();
    let id = s.state.caption_selection[0];
    assert_eq!(s.active_sequence().unwrap().find_caption(id).unwrap().1.text, "two");
    s.execute("captions.previous", json!({})).unwrap();
    let id = s.state.caption_selection[0];
    assert_eq!(s.active_sequence().unwrap().find_caption(id).unwrap().1.text, "one");
    s.execute("captions.setStyle", json!({"color": "#ffff00", "anchor": "top", "size": 40})).unwrap();
    let st = &s.active_sequence().unwrap().caption_tracks[0].style;
    assert_eq!(st.color, [255, 255, 0, 255]);
    assert!(s.execute("captions.setStyle", json!({"anchor": "sideways"})).is_err());
    s.execute("captions.setTimes", json!({"caption": id.0, "startSeconds": 0.5, "endSeconds": 2.5})).unwrap();
    assert!(s.execute("captions.setTimes", json!({"caption": id.0, "endSeconds": 3.5})).is_err(), "would overlap `two`");
    s.execute("captions.deleteTrack", json!({"track": "C1"})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks.is_empty());
}

#[test]
fn caption_list_track_filter_never_falls_back_to_every_track() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    s.execute("captions.newTrack", json!({})).unwrap();
    let all = s.execute("captions.list", json!({})).unwrap();
    let ids: Vec<u64> = all["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{all}");
    for (i, id) in ids.iter().enumerate() {
        for sel in [json!(id), json!(format!("C{}", i + 1))] {
            let r = s.execute("captions.list", json!({"track": sel})).unwrap();
            let got: Vec<u64> = r["tracks"].as_array().unwrap().iter().map(|t| t["id"].as_u64().unwrap()).collect();
            assert_eq!(got, vec![*id], "{sel}");
        }
    }
    let missing = ids.iter().max().unwrap() + 999;
    for sel in [json!(missing), json!("C3"), json!("C0"), json!("nope")] {
        assert!(s.execute("captions.list", json!({"track": sel})).is_err(), "{sel} names no caption track");
    }
}

/// A project saved with captions running into each other (an older build, a script, a hand edit)
/// opens with them ending where the next starts, as one undoable step, and every edit works.
#[test]
fn overlapping_captions_are_fixed_when_the_project_opens() {
    let dir = std::env::temp_dir().join(format!("fc-cap-overlap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("overlap.fcproj");
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    let sec = |x: f64| Tick::from_seconds_f64(x);
    for (t, text) in [(2.0, "one"), (12.0, "two"), (22.0, "three")] {
        s.set_playhead(sec(t));
        s.execute("captions.add", json!({"text": text})).unwrap();
    }
    s.edit("overlap", |p, st| {
        let q = p.sequence_mut(st.active_sequence.unwrap()).unwrap();
        let caps = &mut q.caption_tracks[0].captions;
        caps[0].duration = caps[1].start - caps[0].start + sec(1.0); // runs into "two"
        Ok(())
    })
    .unwrap();
    assert!(s.active_sequence().unwrap().check().is_err());
    s.execute("file.saveAs", json!({"path": path.to_string_lossy()})).unwrap();

    let mut s = Session::default();
    let r = s.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["fixedCaptions"], 1, "{r}");
    let q = s.active_sequence().unwrap();
    q.check().unwrap();
    let caps = &q.caption_tracks[0].captions;
    assert_eq!((caps[0].start, caps[0].end()), (caps[0].start, caps[1].start), "ends where the next starts");
    assert_eq!(caps.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["one", "two", "three"], "nothing dropped");
    // Undo puts the overlap back
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().check().is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edit_is_refused_only_for_an_overlap_of_its_own() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    let sec = |x: f64| Tick::from_seconds_f64(x);
    for t in [2.0, 12.0, 22.0] {
        s.set_playhead(sec(t));
        s.execute("captions.add", json!({"text": "x"})).unwrap();
    }
    s.edit("overlap", |p, st| {
        let q = p.sequence_mut(st.active_sequence.unwrap()).unwrap();
        let caps = &mut q.caption_tracks[0].captions;
        caps[0].duration = caps[1].start - caps[0].start + sec(1.0);
        Ok(())
    })
    .unwrap();
    // an edit elsewhere goes through
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("timeline.trim", json!({"clip": clip.0, "edge": "out", "deltaFrames": -5})).unwrap();
    // a new overlap does not
    let caps = s.active_sequence().unwrap().caption_tracks[0].captions.clone();
    let r = s.execute("captions.setTimes", json!({"caption": caps[1].id.0, "endTime": (caps[2].start + sec(1.0)).0}));
    assert!(r.is_err(), "a new overlap is refused: {r:?}");
}

/// Letter spacing and a dragged position on the track; one phrase set apart with a style of its
/// own (starting as the track's), and back to the track's; all undoable.
#[test]
fn tracking_position_and_a_phrase_style() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    let sec = |x: f64| Tick::from_seconds_f64(x);
    for t in [2.0, 12.0] {
        s.set_playhead(sec(t));
        s.execute("captions.add", json!({"text": "Hello there"})).unwrap();
    }
    s.execute("captions.setStyle", json!({"tracking": 120, "position": [0.3, 0.2]})).unwrap();
    let st = s.active_sequence().unwrap().caption_tracks[0].style.clone();
    assert_eq!((st.tracking, st.position), (120.0, Some([0.3, 0.2])));
    // choosing an anchor puts the block back in its place
    s.execute("captions.setStyle", json!({"anchor": "top"})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].style.position, None);
    assert!(s.execute("captions.setStyle", json!({"position": [1]})).is_err());
    // one phrase in yellow, the other as the track
    let id = s.active_sequence().unwrap().caption_tracks[0].captions[1].id;
    s.execute("captions.setStyle", json!({"captions": [id.0], "color": "#ffdd00"})).unwrap();
    let q = s.active_sequence().unwrap();
    let t = &q.caption_tracks[0];
    assert_eq!(t.captions[1].style.as_ref().unwrap().color, [255, 221, 0, 255]);
    assert_eq!(t.captions[1].style.as_ref().unwrap().tracking, 120.0, "starts as the track's");
    assert!(t.captions[0].style.is_none());
    // the track's style no longer reaches it
    s.execute("captions.setStyle", json!({"size": 80})).unwrap();
    assert_ne!(s.active_sequence().unwrap().caption_tracks[0].captions[1].style.as_ref().unwrap().size, 80.0);
    s.execute("captions.setStyle", json!({"captions": [id.0], "own": false})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks[0].captions[1].style.is_none());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().caption_tracks[0].captions[1].style.is_some());
}

/// Text typed into a caption that would not fit in two lines becomes captions that do, the time
/// shared by length, in one undo step; `split: false` keeps it whole.
#[test]
fn a_long_typed_caption_is_split() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    s.set_playhead(Tick::from_seconds_f64(2.0));
    s.execute("captions.add", json!({"text": "x", "durationSeconds": 8.0})).unwrap();
    let id = s.active_sequence().unwrap().caption_tracks[0].captions[0].id;
    let long = "Children who learn more earn more, and they build a society that protects the next generation. That is why we start today, together.";
    let undo = s.history.undo.len();
    let r = s.execute("captions.setText", json!({"caption": id.0, "text": long})).unwrap();
    assert!(r["split"].as_array().is_some_and(|a| !a.is_empty()), "{r}");
    let t = &s.active_sequence().unwrap().caption_tracks[0];
    assert!(t.captions.len() >= 2);
    assert!(t.captions.iter().all(|c| c.text.lines().count() <= 2 && c.text.lines().all(|l| l.chars().count() <= 42)));
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("captions.setText", json!({"caption": id.0, "text": long, "split": false})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions.len(), 1);
    // and on demand
    let r = s.execute("captions.splitLong", json!({"captions": [id.0]})).unwrap();
    assert!(!r["made"].as_array().unwrap().is_empty(), "{r}");
}

/// A caption added with long text (as the AI assistant adds whole sentences) is split the same
/// way, in the same undo step; `split: false` keeps it whole.
#[test]
fn a_long_added_caption_is_split() {
    let mut s = demo();
    s.execute("captions.newTrack", json!({})).unwrap();
    let long = "Children who learn more earn more, and they build a society that protects the next generation. That is why we start today, together.";
    let undo = s.history.undo.len();
    let r = s.execute("captions.add", json!({"text": long, "seconds": 2.0, "durationSeconds": 8.0})).unwrap();
    assert!(r["split"].as_array().is_some_and(|a| !a.is_empty()), "{r}");
    let t = &s.active_sequence().unwrap().caption_tracks[0];
    assert!(t.captions.len() >= 2);
    assert!(t.captions.iter().all(|c| c.text.lines().count() <= 2 && c.text.lines().all(|l| l.chars().count() <= 42)));
    assert_eq!(s.state.caption_selection.len(), t.captions.len(), "the new phrases are selected");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    s.execute("captions.add", json!({"text": long, "seconds": 2.0, "durationSeconds": 8.0, "split": false})).unwrap();
    assert_eq!(s.active_sequence().unwrap().caption_tracks[0].captions.len(), 1);
    // short text is one caption
    let r = s.execute("captions.add", json!({"text": "Short", "seconds": 12.0})).unwrap();
    assert!(r.get("split").is_none(), "{r}");
}

/// Two caption tracks: C1 with captions at 1, 3, 5, 7 s and C2 (added second, so it is C1 now —
/// new tracks go on top) — returned top to bottom as (ids of C1, ids of C2).
fn two_caption_tracks(s: &mut Session) -> (Vec<ClipId>, Vec<ClipId>) {
    s.execute("captions.newTrack", json!({"name": "lower"})).unwrap();
    for t in [1.0, 3.0, 5.0, 7.0] {
        s.execute("captions.add", json!({"track": "C1", "seconds": t, "durationSeconds": 1.0, "text": format!("low {t}"), "split": false})).unwrap();
    }
    s.execute("captions.newTrack", json!({"name": "upper"})).unwrap();
    for t in [2.0, 4.0, 6.0] {
        s.execute("captions.add", json!({"track": "C1", "seconds": t, "durationSeconds": 1.0, "text": format!("up {t}"), "split": false})).unwrap();
    }
    let q = s.active_sequence().unwrap();
    let ids = |i: usize| q.caption_tracks[i].captions.iter().map(|c| c.id).collect::<Vec<_>>();
    (ids(0), ids(1))
}

fn selected(s: &Session) -> Vec<ClipId> {
    let mut v = s.state.caption_selection.clone();
    v.sort_by_key(|c| c.0);
    v
}

fn sorted(mut v: Vec<ClipId>) -> Vec<ClipId> {
    v.sort_by_key(|c| c.0);
    v
}

#[test]
fn caption_selection_by_click_shift_click_cmd_click_track_and_forward() {
    let mut s = demo();
    let (up, low) = two_caption_tracks(&mut s);
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
    // a click selects one caption, and only it
    s.execute("captions.select", json!({"captions": [low[0].0]})).unwrap();
    assert_eq!(selected(&s), vec![low[0]]);
    assert!(s.state.selection.is_empty(), "a plain click replaces the clip selection");
    // Shift-click: everything from the first to the last, both included
    s.execute("captions.select", json!({"captions": [low[3].0], "range": true})).unwrap();
    assert_eq!(selected(&s), sorted(low.clone()));
    // the range runs from the anchor again: a shorter Shift-click adds nothing new but keeps the rest
    s.execute("captions.select", json!({"captions": [low[1].0], "range": true})).unwrap();
    assert_eq!(selected(&s), sorted(low.clone()));
    // Cmd-click takes one out and puts it back
    s.execute("captions.select", json!({"captions": [low[2].0], "toggle": true})).unwrap();
    assert!(!s.state.caption_selection.contains(&low[2]));
    s.execute("captions.select", json!({"captions": [low[2].0], "toggle": true})).unwrap();
    assert!(s.state.caption_selection.contains(&low[2]));
    // a range across tracks: from low 1 s to up 4 s takes the captions of both that start between
    s.execute("captions.select", json!({"captions": [low[0].0]})).unwrap();
    s.execute("captions.select", json!({"captions": [up[1].0], "range": true})).unwrap();
    assert_eq!(selected(&s), sorted(vec![low[0], low[1], up[0], up[1]]));
    // Cmd- and Shift-clicks keep the clips selected (captions and clips go together)
    s.execute("timeline.select", json!({"clips": [clip.0]})).unwrap();
    s.execute("captions.select", json!({"captions": [up[2].0], "toggle": true})).unwrap();
    assert!(s.state.selection.contains(&clip) && s.state.caption_selection.contains(&up[2]));
    // a whole track, and a caption with everything after it
    s.execute("captions.select", json!({"track": "C2", "all": true})).unwrap();
    assert_eq!(selected(&s), sorted(low.clone()));
    s.execute("captions.select", json!({"from": up[1].0})).unwrap();
    assert_eq!(selected(&s), sorted(vec![up[1], up[2]]));
    // Select All takes the captions too
    s.execute("edit.selectAll", json!({})).unwrap();
    assert_eq!(s.state.caption_selection.len(), 7);
    assert!(!s.state.selection.is_empty());
}

#[test]
fn clips_and_captions_selected_together_move_together() {
    let mut s = demo();
    let (up, low) = two_caption_tracks(&mut s);
    let s1 = TICKS_PER_SECOND;
    // a V2 clip (nothing else on V2) and two captions at once
    let clip = s.active_sequence().unwrap().video_tracks[1].items[0].clone();
    s.execute("timeline.select", json!({"clips": [clip.id.0]})).unwrap();
    s.execute("captions.select", json!({"captions": [up[0].0, low[3].0], "add": true})).unwrap();
    let undo = s.history.undo.len();
    let track = s.active_sequence().unwrap().video_tracks[1].id;
    let caps: Vec<u64> = s.state.caption_selection.iter().map(|c| c.0).collect();
    let start_of = |s: &Session, c: ClipId| s.active_sequence().unwrap().find_caption(c).unwrap().1.start;
    let (u0, l3) = (start_of(&s, up[0]), start_of(&s, low[3]));
    // 0.5 s later: the gaps between the captions leave room
    s.execute("timeline.move", json!({"moves": [{"clip": clip.id.0, "track": track.0, "time": clip.start.0 + s1 / 2}], "captions": caps})).unwrap();
    let d = s.active_sequence().unwrap().find_item(clip.id).unwrap().1.start - clip.start;
    assert!(d > Tick::ZERO);
    assert_eq!((start_of(&s, up[0]), start_of(&s, low[3])), (u0 + d, l3 + d), "the captions moved exactly as far as the clip");
    assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
    // 1 s later would run the 2.5 s caption into the 4 s one: nothing moves, nothing goes out of sync
    let before = s.active_sequence().unwrap().clone();
    let caps: Vec<u64> = s.state.caption_selection.iter().map(|c| c.0).collect();
    let e = s
        .execute("timeline.move", json!({"moves": [{"clip": clip.id.0, "track": track.0, "time": clip.start.0 + s1 / 2 + s1}], "captions": caps}))
        .unwrap_err();
    assert!(e.to_string().contains("out of sync"), "{e}");
    assert_eq!(s.active_sequence().unwrap(), &before);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(start_of(&s, up[0]), u0);
}

#[test]
fn captions_are_copied_cut_pasted_and_cleared_with_the_clips() {
    let mut s = demo();
    let (up, low) = two_caption_tracks(&mut s);
    let s1 = TICKS_PER_SECOND;
    // a caption alone: copy, paste at 20 s on its own track
    s.execute("captions.select", json!({"captions": [up[0].0]})).unwrap();
    s.execute("edit.copy", json!({})).unwrap();
    s.set_playhead(Tick(20 * s1));
    let at = s.playhead();
    s.execute("edit.paste", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let pasted = q.caption_tracks[0].captions.iter().find(|c| c.start == at).expect("pasted on C1");
    assert_eq!(pasted.text, "up 2");
    assert_eq!(s.state.caption_selection, vec![pasted.id]);
    // pasted over a caption: the one it overlaps is shortened to where the pasted one starts
    s.execute("captions.select", json!({"captions": [low[1].0]})).unwrap(); // C2, 3–4 s
    s.execute("edit.copy", json!({})).unwrap();
    let old = s.active_sequence().unwrap().find_caption(low[3]).unwrap().1.clone(); // C2, 7–8 s
    s.set_playhead(Tick(7 * s1 + s1 / 2));
    let at = s.playhead();
    s.execute("edit.paste", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let kept = q.find_caption(low[3]).unwrap().1;
    assert_eq!((kept.start, kept.end()), (old.start, at), "shortened to the pasted caption");
    assert!(q.caption_tracks[1].captions.iter().any(|c| c.start == at && c.text == "low 3"));
    q.check().unwrap();
    // a clip and a caption together keep their distance
    let clip = s.active_sequence().unwrap().video_tracks[1].items[0].clone();
    s.execute("timeline.select", json!({"clips": [clip.id.0]})).unwrap();
    s.execute("captions.select", json!({"captions": [up[2].0], "add": true})).unwrap(); // 6 s
    s.execute("edit.copy", json!({})).unwrap();
    let gap = s.active_sequence().unwrap().find_caption(up[2]).unwrap().1.start - clip.start;
    s.set_playhead(Tick(40 * s1));
    s.execute("edit.paste", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    let new_cap = q.find_caption(s.state.caption_selection[0]).unwrap().1.clone();
    let new_clip = q.find_item(s.state.selection[0]).unwrap().1.clone();
    assert_eq!(new_cap.start - new_clip.start, gap, "the caption is as far from the clip as before");
    // Clear removes both; Cut too, after copying them
    s.execute("edit.clear", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    assert!(q.find_caption(new_cap.id).is_none() && q.find_item(new_clip.id).is_none());
    s.execute("captions.select", json!({"captions": [up[1].0]})).unwrap();
    s.execute("edit.cut", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().find_caption(up[1]).is_none());
    assert_eq!(s.state.caption_clipboard.len(), 1);
}
