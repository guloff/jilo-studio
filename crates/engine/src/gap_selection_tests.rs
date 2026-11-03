//! A gap between clips selected in the Timeline (Premiere): Delete / Ripple Delete close it, the
//! later clips on every sync-locked track move up by its length, and one undo puts them back.

use jilo_project::ClipId;
use jilo_time::Tick;
use serde_json::json;

use crate::Session;

/// A sequence with one video and one audio track: the same clip at 0 and, after a gap of its own
/// length, again; each with its linked sound on A1.
fn two_clips_and_a_gap() -> (Session, Tick, Vec<ClipId>) {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name": "Gap", "video": 1, "audio": 1})).unwrap();
    let item = s.project.items.values().find(|i| i.has_video() && i.has_audio() && i.as_media().is_some()).map(|i| i.id).unwrap();
    s.execute("timeline.place", json!({"item": item.0, "time": 0})).unwrap();
    let len = s.active_sequence().unwrap().video_tracks[0].items[0].duration;
    s.execute("timeline.place", json!({"item": item.0, "time": (len + len).0})).unwrap();
    let q = s.active_sequence().unwrap();
    let later: Vec<ClipId> = q.all_tracks().flat_map(|t| t.items.iter().filter(|i| i.start > Tick::ZERO).map(|i| i.id)).collect();
    assert_eq!(later.len(), 2, "the later clip and its sound");
    s.state.selection.clear();
    (s, len, later)
}

fn start(s: &Session, c: ClipId) -> Tick {
    s.active_sequence().unwrap().find_item(c).unwrap().1.start
}

#[test]
fn delete_on_a_selected_gap_closes_it_on_every_track() {
    let (mut s, len, later) = two_clips_and_a_gap();
    assert!(!s.is_enabled("edit.clear"), "nothing selected yet");
    let r = s.execute("timeline.selectGap", json!({"track": "V1", "time": len.0 * 3 / 2})).unwrap();
    assert_eq!((r["start"].as_i64(), r["end"].as_i64()), (Some(len.0), Some((len + len).0)), "{r}");
    assert!(s.selected_gap().is_some());
    assert!(s.is_enabled("edit.clear") && s.is_enabled("edit.rippleDelete"));
    s.execute("edit.clear", json!({})).unwrap();
    for c in &later {
        assert_eq!(start(&s, *c), len, "the clip after the gap and its sound moved up by the gap");
    }
    assert!(s.state.gap_selection.is_none() && s.selected_gap().is_none());
    s.execute("edit.undo", json!({})).unwrap();
    for c in &later {
        assert_eq!(start(&s, *c), len + len, "one undo puts them back");
    }
    // Ripple Delete (Shift+Delete) does the same
    s.execute("timeline.selectGap", json!({"track": "A1", "time": len.0 * 3 / 2})).unwrap();
    s.execute("edit.rippleDelete", json!({})).unwrap();
    assert_eq!(start(&s, later[0]), len);
}

#[test]
fn only_a_gap_can_be_selected_and_a_clip_selection_replaces_it() {
    let (mut s, len, later) = two_clips_and_a_gap();
    assert!(s.execute("timeline.selectGap", json!({"track": "V1", "time": len.0 / 2})).is_err(), "on a clip");
    assert!(s.execute("timeline.selectGap", json!({"track": "V1", "time": len.0 * 4})).is_err(), "after the last clip");
    assert!(s.state.gap_selection.is_none());
    s.execute("timeline.selectGap", json!({"track": "V1", "time": len.0 * 3 / 2})).unwrap();
    s.execute("timeline.select", json!({"clips": [later[0].0]})).unwrap();
    assert!(s.state.gap_selection.is_none(), "a clip selection replaces the gap");
    // Delete then deletes the clip, not the gap
    s.execute("edit.clear", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().find_item(later[0]).is_none());
    // a gap the clips no longer leave is not selected any more
    let (mut s, len, _) = two_clips_and_a_gap();
    s.execute("timeline.selectGap", json!({"track": "V1", "time": len.0 * 3 / 2})).unwrap();
    s.execute("sequence.closeGap", json!({"track": "V1", "time": len.0 * 3 / 2})).unwrap();
    assert!(s.selected_gap().is_none() && !s.is_enabled("edit.clear"));
}

/// Sequence ▸ Close All Gaps: every gap between clips on every track, one undo step; with music
/// under the whole sequence on a sync-locked track nothing can close, and the track is named.
#[test]
fn close_all_gaps_closes_every_gap_and_respects_sync_lock() {
    let (mut s, len, later) = two_clips_and_a_gap();
    let item = s.project.items.values().find(|i| i.has_video() && i.has_audio() && i.as_media().is_some()).map(|i| i.id).unwrap();
    // a third clip after a second gap, of twice the length
    s.execute("timeline.place", json!({"item": item.0, "time": len.0 * 5})).unwrap();
    let starts = |s: &Session| -> Vec<i64> {
        let q = s.active_sequence().unwrap();
        let mut v: Vec<i64> = q.video_tracks[0].items.iter().map(|i| i.start.0).collect();
        v.sort();
        v
    };
    assert_eq!(starts(&s), vec![0, (len + len).0, len.0 * 5]);
    let r = s.execute("sequence.closeAllGaps", json!({})).unwrap();
    assert_eq!(r["closed"].as_u64(), Some(2), "{r}");
    assert_eq!(starts(&s), vec![0, len.0, (len + len).0], "back to back");
    for c in &later {
        assert_eq!(start(&s, *c), len, "the sound moved with its picture");
    }
    assert!(s.execute("sequence.closeAllGaps", json!({})).unwrap_err().to_string().contains("no gaps"));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(starts(&s), vec![0, (len + len).0, len.0 * 5], "one undo step");
    // music under everything on a sync-locked A2: nothing can close, and the reason names the track
    s.execute("sequence.addTracks", json!({"video": 0, "audio": 1})).unwrap();
    let a2 = s.active_sequence().unwrap().audio_tracks[1].id;
    let music = s.project.items.values().find(|i| !i.has_video() && i.has_audio()).map(|i| i.id).unwrap();
    s.execute("timeline.place", json!({"item": music.0, "track": a2.0, "time": 0, "duration": len.0 * 7})).unwrap();
    let e = s.execute("sequence.closeAllGaps", json!({})).unwrap_err().to_string();
    assert!(e.contains("A2") || e.contains("Audio 2"), "{e}");
    assert_eq!(starts(&s), vec![0, (len + len).0, len.0 * 5]);
    // without its sync lock the music stays and the gaps close
    s.execute("timeline.setTrack", json!({"track": a2.0, "syncLock": false})).unwrap();
    s.execute("sequence.closeAllGaps", json!({})).unwrap();
    assert_eq!(starts(&s), vec![0, len.0, (len + len).0]);
}
