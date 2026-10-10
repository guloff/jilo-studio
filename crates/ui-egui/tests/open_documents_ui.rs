//! Files the OS asks to open (macOS: double-click in Finder, Open With, a drop on the Dock icon)
//! reach the app through `HostHooks::opened_documents`: a project opens (or asks to save edits
//! first), media is imported, and the project is reported for the system's recent documents.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::state::Mode;
use serde_json::json;

type Inbox = Arc<Mutex<Vec<String>>>;

fn harness(session: Session, inbox: Inbox, noted: Inbox) -> Harness<'static, FilmcraftApp> {
    let mut app = FilmcraftApp::new(session);
    app.hooks.opened_documents = Some(Box::new(move || std::mem::take(&mut *inbox.lock().unwrap())));
    app.hooks.note_recent_document = Some(Box::new(move |p: &str| noted.lock().unwrap().push(p.to_string())));
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
    for _ in 0..3 {
        h.step();
    }
    h
}

fn saved_project(dir: &std::path::Path) -> String {
    let path = dir.join("Opened From Finder.fcproj").to_string_lossy().into_owned();
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.saveAs", json!({"path": path})).unwrap();
    path
}

#[test]
fn a_project_opened_from_finder_opens_and_is_noted_as_recent() {
    let dir = std::env::temp_dir().join(format!("filmcraft-open-docs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = saved_project(&dir);
    let (inbox, noted) = (Inbox::default(), Inbox::default());
    let mut h = harness(Session::default(), inbox.clone(), noted.clone());
    assert!(noted.lock().unwrap().is_empty(), "nothing to note before a project has a file");
    inbox.lock().unwrap().push(path.clone());
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().session.path.as_deref(), Some(path.as_str()));
    assert_eq!(h.state().ui.mode, Mode::Edit);
    assert_eq!(*noted.lock().unwrap(), std::slice::from_ref(&path), "reported once");

    // with unsaved edits the Save Project prompt comes first, and nothing is replaced yet
    h.state_mut().session.execute("markers.add", json!({"target": "program", "frame": 12})).unwrap();
    let other = dir.join("Other.fcproj").to_string_lossy().into_owned();
    std::fs::copy(&path, &other).unwrap();
    inbox.lock().unwrap().push(other.clone());
    for _ in 0..3 {
        h.step();
    }
    let draft = h.state().ui.clip_dialog.clone().expect("save prompt");
    assert_eq!((draft.command.as_str(), draft.params["path"].as_str()), ("file.open", Some(other.as_str())));
    assert_eq!(h.state().session.path.as_deref(), Some(path.as_str()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn media_opened_from_finder_is_imported() {
    let dir = std::env::temp_dir().join(format!("filmcraft-open-media-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("tone.wav");
    std::fs::write(&wav, filmcraft_media::wav::write_wav16(&[0.0; 9600], 2, 48_000)).unwrap();
    let (inbox, noted) = (Inbox::default(), Inbox::default());
    let mut s = Session::default();
    s.execute("file.newProject", json!({"force": true})).unwrap();
    let mut h = harness(s, inbox.clone(), noted);
    let before = h.state().session.project.items.len();
    inbox.lock().unwrap().push(wav.to_string_lossy().into_owned());
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().session.project.items.len(), before + 1, "{}", h.state().ui.status);
    let _ = std::fs::remove_dir_all(&dir);
}
