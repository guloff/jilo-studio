//! Offline media and relinking end to end: moved media opens offline (the slate renders),
//! relinking one file finds the others by folder remap, fingerprint mismatches are refused,
//! Make Offline, folder search and undo.

use filmcraft_media::DemoScene;
use filmcraft_project::ItemId;
use filmcraft_render::offline::{OfflineReason, slate_rgba8};
use filmcraft_time::FrameRate;
use serde_json::json;

use crate::Session;
use crate::media_test_util::{frame_rgba, make_movie, make_movie_at, psnr, session_with, tmp_dir};
use crate::relink::{apply_remap, derive_remap};

const W: u32 = 160;
const H: u32 = 90;

/// A saved project at `<root>/Project/p.fcproj` using `<root>/Media/{a,b}.mov`.
fn saved_project(root: &std::path::Path) -> (String, Vec<ItemId>) {
    let media = root.join("Media");
    std::fs::create_dir_all(&media).unwrap();
    let (a, b) = (media.join("a.mov"), media.join("b.mov"));
    make_movie(&a, DemoScene::Aurora, W, H, 12);
    make_movie(&b, DemoScene::CityNight, W, H, 12);
    let (mut s, items, _) = session_with(&[&a, &b]);
    let proj = root.join("Project");
    std::fs::create_dir_all(&proj).unwrap();
    let path = proj.join("p.fcproj").to_string_lossy().into_owned();
    s.execute("file.save", json!({"path": path})).unwrap();
    (path, items)
}

fn open(path: &str) -> Session {
    let mut s = Session::default();
    s.execute("file.open", json!({"path": path})).unwrap();
    s
}

#[test]
fn identity_and_remap_helpers() {
    let a = crate::relink::identity_of_bytes(&vec![7u8; 3 << 20]);
    let mut v = vec![7u8; 3 << 20];
    v[(3 << 20) - 5] = 1;
    let b = crate::relink::identity_of_bytes(&v);
    assert_eq!(a.size, b.size);
    assert_ne!(a.fingerprint, b.fingerprint, "a change in the last MiB changes the fingerprint");
    assert_eq!(derive_remap("/Volumes/A/shoot/day1/a.mov", "/Users/me/shoot/day1/a.mov"), Some(("/Volumes/A".into(), "/Users/me".into())));
    assert_eq!(derive_remap(r"D:\proj\media\a.mov", "/Volumes/D/proj/media/a.mov"), Some(("D:".into(), "/Volumes/D".into())));
    assert_eq!(apply_remap(r"D:\proj\media\b.mov", "D:", "/Volumes/D").as_deref(), Some("/Volumes/D/proj/media/b.mov"));
    assert_eq!(apply_remap("/Volumes/AB/x.mov", "/Volumes/A", "/x"), None, "prefix must end at a separator");
}

#[test]
fn moved_media_opens_offline_and_relinks_others_by_folder() {
    let root = tmp_dir("relink-moved");
    let (path, items) = saved_project(&root);
    let online = frame_rgba(&mut open(&path), 3, 1.0).2;
    // move the media folder
    let moved = root.join("Moved").join("Media");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(root.join("Media"), &moved).unwrap();

    let mut s = Session::default();
    let r = s.execute("file.open", json!({"path": path})).unwrap();
    assert_eq!(r["missingMedia"], json!(2), "{r}");
    assert!(s.offline.prompt, "the Link Media dialog is requested");
    let (w, h, px) = frame_rgba(&mut s, 3, 1.0);
    let slate = slate_rgba8(w, h, "a.mov", OfflineReason::Missing);
    assert!(psnr(&px, &slate) > 40.0, "offline clip renders the offline slate");
    let m = s.execute("media.findMissing", json!({})).unwrap();
    assert_eq!(m["missing"].as_array().unwrap().len(), 2);

    // link a; b follows by the derived folder remap
    let r = s.execute("media.relink", json!({"item": items[0].0, "path": moved.join("a.mov").to_string_lossy()})).unwrap();
    assert_eq!(r["relinked"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["remaining"], json!([]));
    assert!(!s.offline.prompt);
    assert_eq!(frame_rgba(&mut s, 3, 1.0).2, online, "relinked media renders as before the move");
    let b_path = s.project.item(items[1]).unwrap().as_media().unwrap().media.clone();
    let filmcraft_project::MediaRef::File { path: b_path } = b_path else { panic!("relinked media must remain file-based") };
    assert_eq!(std::path::Path::new(&b_path), moved.join("b.mov"));
    // undo brings the old (missing) paths back, and the slate with them
    s.execute("edit.undo", json!({})).unwrap();
    assert!(psnr(&frame_rgba(&mut s, 3, 1.0).2, &slate) > 40.0);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(frame_rgba(&mut s, 3, 1.0).2, online);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn auto_relink_by_prefix_and_by_folder_search() {
    let root = tmp_dir("relink-auto");
    let (path, items) = saved_project(&root);
    let dest = root.join("Archive").join("2026");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::rename(root.join("Media"), dest.join("Media")).unwrap();
    let mut s = open(&path);
    let r = s.execute("media.autoRelink", json!({"from": root.join("Media").to_string_lossy(), "to": dest.join("Media").to_string_lossy()})).unwrap();
    assert_eq!(r["relinked"].as_array().unwrap().len(), 2, "{r}");
    assert!(s.execute("media.findMissing", json!({})).unwrap()["missing"].as_array().unwrap().is_empty());

    // again, this time searching a folder tree
    let mut s = open(&path);
    let r = s.execute("media.search", json!({"folder": root.to_string_lossy(), "item": items[1].0})).unwrap();
    let c = &r["results"][0]["candidates"][0];
    assert_eq!(c["identityMatch"], json!(true), "{r}");
    let r = s.execute("media.autoRelink", json!({"folder": root.to_string_lossy()})).unwrap();
    assert_eq!(r["relinked"].as_array().unwrap().len(), 2, "{r}");
    let _ = std::fs::remove_dir_all(&root);
}

// An EDL line with no `* SOURCE FILE:` names its file by clip name only. Imported away from the
// file, the clip is missing like any file that isn't found: the folder search and Relink Others
// find it, and opening the project asks for it. It was made offline, which all three skip.
#[test]
fn edl_clips_named_only_by_clip_name_are_missing_not_made_offline() {
    let root = tmp_dir("relink-edl-reel");
    let media = root.join("Media");
    std::fs::create_dir_all(&media).unwrap();
    make_movie(&media.join("a.mov"), DemoScene::Aurora, W, H, 24);
    make_movie(&media.join("b.mov"), DemoScene::CityNight, W, H, 24);
    let edit = root.join("Edit");
    std::fs::create_dir_all(&edit).unwrap();
    let edl = "TITLE: CUT\nFCM: NON-DROP FRAME\n\n\
        001  A        V     C        00:00:00:00 00:00:00:12 01:00:00:00 01:00:00:12\n* FROM CLIP NAME: a.mov\n\n\
        002  B        V     C        00:00:00:00 00:00:00:12 01:00:00:12 01:00:01:00\n* FROM CLIP NAME: b.mov\n";
    std::fs::write(edit.join("cut.edl"), edl).unwrap();
    let mut s = Session::default();
    s.execute("file.import", json!({"paths": [edit.join("cut.edl").to_string_lossy()]})).unwrap();
    let m = s.execute("media.findMissing", json!({})).unwrap();
    let statuses: Vec<&str> = m["missing"].as_array().unwrap().iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["missing", "missing"], "{m}");
    let path = root.join("p.fcproj").to_string_lossy().into_owned();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    assert!(open(&path).offline.prompt, "opening the project asks for the missing files");

    // The EDL gives neither a duration nor a picture size, so those checks are off.
    let checks = json!({"duration": false, "metadata": false});
    let r = s.execute("media.autoRelink", json!({"folder": media.to_string_lossy(), "match": checks})).unwrap();
    assert_eq!(r["relinked"].as_array().unwrap().len(), 2, "{r}");
    let mut s = open(&path);
    let (item, name) = (m["missing"][0]["item"].as_u64().unwrap(), m["missing"][0]["fileName"].as_str().unwrap());
    let r = s.execute("media.relink", json!({"item": item, "path": media.join(name).to_string_lossy(), "match": checks})).unwrap();
    assert_eq!(r["remaining"], json!([]), "relinking one finds the other in its folder: {r}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn fingerprint_mismatch_is_refused_unless_forced() {
    let root = tmp_dir("relink-mismatch");
    let (path, items) = saved_project(&root);
    std::fs::remove_dir_all(root.join("Media")).unwrap();
    // same name, duration, size and rate — different pictures
    let other = root.join("Other");
    std::fs::create_dir_all(&other).unwrap();
    make_movie(&other.join("a.mov"), DemoScene::Dunes, W, H, 12);
    let mut s = open(&path);
    let e = s.execute("media.relink", json!({"item": items[0].0, "path": other.join("a.mov").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("not the same file"), "{e}");
    assert_eq!(s.offline.missing.len(), 2, "nothing changed");
    let r =
        s.execute("media.relink", json!({"item": items[0].0, "path": other.join("a.mov").to_string_lossy(), "force": true, "relinkOthers": false})).unwrap();
    assert_eq!(r["relinked"].as_array().unwrap().len(), 1);
    assert_eq!(r["remaining"], json!([items[1].0]));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn forced_relink_takes_the_new_files_properties() {
    let root = tmp_dir("relink-forced-info");
    let (path, items) = saved_project(&root);
    std::fs::remove_dir_all(root.join("Media")).unwrap();
    // same name, different pictures and a different size
    let other = root.join("Other");
    std::fs::create_dir_all(&other).unwrap();
    make_movie(&other.join("a.mov"), DemoScene::Dunes, W * 2, H * 2, 12);
    let mut s = open(&path);
    s.execute("media.relink", json!({"item": items[0].0, "path": other.join("a.mov").to_string_lossy(), "force": true, "relinkOthers": false})).unwrap();
    let width = s.project.item(items[0]).and_then(|i| i.as_media()).and_then(|m| m.info.video.as_ref().map(|v| v.width));
    assert_eq!(width, Some(W * 2), "the item describes the replacement file");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn make_offline_shows_the_slate_until_relinked() {
    let root = tmp_dir("relink-offline");
    let (path, items) = saved_project(&root);
    let mut s = open(&path);
    let online = frame_rgba(&mut s, 3, 1.0);
    s.execute("project.select", json!({"items": [items[0].0]})).unwrap();
    s.execute("media.makeOffline", json!({})).unwrap();
    let (w, h, px) = frame_rgba(&mut s, 3, 1.0);
    assert!(psnr(&px, &slate_rgba8(w, h, "a.mov", OfflineReason::MadeOffline)) > 40.0);
    let st = s.execute("media.status", json!({"item": items[0].0})).unwrap();
    assert_eq!(st[0]["status"], json!("offline"));
    assert!(root.join("Media/a.mov").exists(), "media stays on disk by default");
    let a = root.join("Media/a.mov").to_string_lossy().into_owned();
    s.execute("media.relink", json!({"item": items[0].0, "path": a})).unwrap();
    assert_eq!(frame_rgba(&mut s, 3, 1.0), online);
    let _ = std::fs::remove_dir_all(&root);
}

// Regression test for issue #110: after `file.import` re-imports a file at a path the project
// already listed as offline, `s.offline.missing` must drop that item so the slate and the
// "Media missing" badge clear without reopening the project. Before the fix (the patch in
// `file.import` that calls `crate::relink::refresh(s)`), the slate stayed stale until reopen
// or per-item relink.
#[test]
fn reimport_clears_offline_list_after_path_resolves() {
    let root = tmp_dir("relink-reimport");
    let (path, items) = saved_project(&root);
    let a = root.join("Media/a.mov");

    // Delete a.mov so the saved project now points at a missing file, then reopen:
    // `on_open` → `refresh` → `scan` registers items[0] as missing.
    let online = frame_rgba(&mut open(&path), 3, 1.0);
    std::fs::remove_file(&a).unwrap();
    let mut s = open(&path);
    assert!(s.offline.missing.contains(&items[0]), "setup: a.mov missing on reopen");
    assert_ne!(frame_rgba(&mut s, 3, 1.0), online, "setup: the slate shows while a.mov is missing");

    // Recreate the file at the exact path the project stores, then import. `file.import`
    // must drop items[0] from `s.offline.missing`. Without the fix, the entry stays stale
    // and the user has to reopen or `media.relink` each item.
    make_movie(&a, DemoScene::Aurora, W, H, 12);
    s.execute("file.import", json!({"paths": [a.to_string_lossy()]})).unwrap();
    assert!(!s.offline.missing.contains(&items[0]), "issue #110 — file.import did not refresh s.offline.missing (still contains {:?})", s.offline.missing);
    assert_eq!(s.execute("media.findMissing", json!({})).unwrap()["missing"].as_array().unwrap().len(), 0);
    // and the monitors show the file again, not the cached slate
    assert_eq!(frame_rgba(&mut s, 3, 1.0), online);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn explicit_folder_remaps_normalize_both_prefixes_and_respect_boundaries() {
    assert_eq!(apply_remap(r"C:\shoot\Media\a.mov", r"C:\shoot\Media\", r"D:\archive\Media\"), Some("D:/archive/Media/a.mov".into()));
    assert_eq!(apply_remap("C:/shoot/Media/a.mov", r"C:\shoot\Media", r"D:\archive\Media"), Some("D:/archive/Media/a.mov".into()));
    assert_eq!(apply_remap("C:/shoot/Media2/a.mov", r"C:\shoot\Media", r"D:\archive\Media"), None);
}

/// The first video clip of the active sequence: (its media item, its source in-point).
fn first_clip(s: &Session) -> (ItemId, filmcraft_time::Tick) {
    let seq = s.project.sequence(s.state.active_sequence.unwrap()).unwrap();
    let ti = &seq.video_tracks[0].items[0];
    (ti.item, ti.source_in)
}

// An EDL's source times are timecodes. Imported next to its media, a clip lands on the file's
// own time. Imported elsewhere and relinked with Align Timecode, it kept the timecode as media
// time, so it showed the file's last frame.
#[test]
fn edl_media_relinked_with_align_timecode_lands_where_import_puts_it() {
    // The EDL is read at 24 fps. Its file runs at 24 fps, or at 25, where each timecode counts in
    // its own file's frames.
    for (fps, rate) in [(24, FrameRate::FPS_24), (25, FrameRate::FPS_25)] {
        edl_media_relinked_at(fps, rate);
    }
}

fn edl_media_relinked_at(fps: i64, rate: FrameRate) {
    let root = tmp_dir(&format!("relink-edl-timecode-{fps}"));
    let media = root.join("Media");
    std::fs::create_dir_all(&media).unwrap();
    // 2 s from 01:00:00:00; the EDL cuts in at 01:00:01:00, one second in
    let file = media.join("A001.mxf");
    make_movie_at(&file, DemoScene::Aurora, W, H, rate, 2 * fps, 3600 * fps);
    let edl = "TITLE: CUT\nFCM: NON-DROP FRAME\n\n001  A001     V     C        01:00:01:00 01:00:01:12 01:00:00:00 01:00:00:12\n* FROM CLIP NAME: A001.mxf\n";
    let import = |dir: &std::path::Path| {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("cut.edl"), edl).unwrap();
        let mut s = Session::default();
        s.execute("file.import", json!({"paths": [dir.join("cut.edl").to_string_lossy()]})).unwrap();
        s
    };

    let mut next_to_media = import(&media);
    assert_eq!(first_clip(&next_to_media).1, FrameRate::FPS_24.tick_of(24), "one second into the {fps} fps file");

    let mut s = import(&root.join("Edit"));
    let (item, _) = first_clip(&s);
    let m = s.execute("media.findMissing", json!({})).unwrap();
    assert_eq!(m["missing"][0]["item"], json!(item.0), "setup: the media is not next to the EDL: {m}");
    // The EDL gives neither a duration nor a picture size, so those checks are off.
    let r = s
        .execute(
            "media.relink",
            json!({"item": item.0, "path": file.to_string_lossy(), "alignTimecode": true, "match": {"duration": false, "metadata": false}}),
        )
        .unwrap();
    assert_eq!(r["remaining"], json!([]), "{r}");
    assert_eq!(first_clip(&s).1, first_clip(&next_to_media).1, "{fps} fps");
    assert_eq!(frame_rgba(&mut s, 6, 1.0).2, frame_rgba(&mut next_to_media, 6, 1.0).2, "same picture at {fps} fps");
    let _ = std::fs::remove_dir_all(&root);
}
