//! Built-in project templates: a new project ready for a kind of video — its sequence's frame and
//! rate, named tracks, bins, and a caption track with a style where the kind needs captions.
//! `file.templates` lists them first (`builtin:<id>`), `file.newProjectFromTemplate` makes them;
//! templates saved with File ▸ Save as Template… come after.

use filmcraft_project::{CaptionFormat, CaptionTrack, ItemId, Project, SequenceSettings, TrackId};
use filmcraft_time::FrameRate;

/// A built-in template.
pub struct Builtin {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    width: u32,
    height: u32,
    rate: FrameRate,
    video: &'static [&'static str],
    audio: &'static [&'static str],
    /// Caption style preset for a caption track (None: no caption track).
    captions: Option<&'static str>,
}

/// The built-in templates, in the order they are offered.
pub const BUILTIN: [Builtin; 4] = [
    Builtin {
        id: "shorts",
        name: "Shorts 9:16",
        description: "Vertical 1080×1920 at 30 fps for Shorts, Reels and TikTok: video, B-roll and graphics tracks, voice and music, bold captions word by word.",
        width: 1080,
        height: 1920,
        rate: FrameRate::FPS_30,
        video: &["Video", "B-roll", "Graphics"],
        audio: &["Voice", "Music"],
        captions: Some("karaoke-yellow"),
    },
    Builtin {
        id: "podcast",
        name: "Podcast",
        description: "1920×1080 at 30 fps: three cameras, three microphones and music, ready for multi-camera editing.",
        width: 1920,
        height: 1080,
        rate: FrameRate::FPS_30,
        video: &["Camera 1", "Camera 2", "Camera 3"],
        audio: &["Mic 1", "Mic 2", "Mic 3", "Music"],
        captions: None,
    },
    Builtin {
        id: "interview",
        name: "Interview",
        description: "1920×1080 at 25 fps: two cameras and a track for lower thirds; host, guest and music; captions.",
        width: 1920,
        height: 1080,
        rate: FrameRate::FPS_25,
        video: &["Camera A", "Camera B", "Lower Thirds"],
        audio: &["Host", "Guest", "Music"],
        captions: Some("clean-shadow"),
    },
    Builtin {
        id: "vlog",
        name: "Vlog",
        description: "1920×1080 at 30 fps: A-roll, B-roll and titles; voice, music and sound effects; captions.",
        width: 1920,
        height: 1080,
        rate: FrameRate::FPS_30,
        video: &["A-roll", "B-roll", "Titles"],
        audio: &["Voice", "Music", "Effects"],
        captions: Some("yellow-pop"),
    },
];

/// The built-in template `key` names: `builtin:<id>`, its id or its name.
pub fn find(key: &str) -> Option<&'static Builtin> {
    let k = key.strip_prefix("builtin:").unwrap_or(key);
    BUILTIN.iter().find(|b| b.id.eq_ignore_ascii_case(k) || b.name.eq_ignore_ascii_case(k))
}

impl Builtin {
    /// A new project made from the template, and its sequence.
    pub fn project(&self, name: &str) -> (Project, ItemId) {
        let mut p = Project::new(name);
        for bin in ["Footage", "Audio", "Graphics"] {
            p.add_bin(bin, None);
        }
        let seqs = p.add_bin("Sequences", None);
        let preset = format!("{}x{} {}", self.width, self.height, self.rate.num / self.rate.den.max(1));
        let settings = SequenceSettings { width: self.width, height: self.height, frame_rate: self.rate, preset, ..Default::default() };
        let seq = p.new_sequence(self.name, settings, self.video.len(), self.audio.len(), Some(seqs));
        let caption_id = self.captions.map(|_| TrackId(p.alloc_id()));
        if let Some(q) = p.sequence_mut(seq) {
            for (t, name) in q.video_tracks.iter_mut().zip(self.video) {
                t.name = (*name).to_string();
            }
            for (t, name) in q.audio_tracks.iter_mut().zip(self.audio) {
                t.name = (*name).to_string();
            }
            if let (Some(id), Some(preset)) = (caption_id, self.captions) {
                let mut c = CaptionTrack::new(id, "Captions".into(), CaptionFormat::Subtitle);
                if let Some(pr) = crate::caption_presets::find(preset) {
                    c.style = pr.style;
                }
                q.caption_tracks.push(c);
            }
        }
        (p, seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_makes_a_valid_project() {
        for b in &BUILTIN {
            let (p, seq) = b.project("T");
            let q = p.sequence(seq).expect("sequence");
            q.check().unwrap_or_else(|e| panic!("{}: {e}", b.id));
            assert_eq!((q.settings.width, q.settings.height), (b.width, b.height));
            assert_eq!(q.video_tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), b.video);
            assert_eq!(q.audio_tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), b.audio);
            assert_eq!(q.caption_tracks.len(), usize::from(b.captions.is_some()), "{}", b.id);
            if let Some(id) = b.captions {
                assert!(crate::caption_presets::find(id).is_some(), "{}: no caption preset {id}", b.id);
            }
        }
        assert_eq!(find("builtin:shorts").map(|b| b.id), Some("shorts"));
        assert_eq!(find("Podcast").map(|b| b.id), Some("podcast"));
        assert!(find("nothing").is_none());
    }

    #[test]
    fn a_new_project_from_a_built_in_template() {
        let mut s = crate::Session::default();
        let listed = s.execute("file.templates", serde_json::json!({})).unwrap();
        let ids: Vec<&str> = listed.as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
        assert_eq!(&ids[..4], ["builtin:shorts", "builtin:podcast", "builtin:interview", "builtin:vlog"]);
        s.execute("file.newProjectFromTemplate", serde_json::json!({"template": "builtin:shorts", "name": "My Short"})).unwrap();
        assert_eq!(s.project.name, "My Short");
        let q = s.active_sequence().expect("the template's sequence is open");
        assert_eq!((q.settings.width, q.settings.height), (1080, 1920));
        assert_eq!(q.caption_tracks.len(), 1);
        assert!(!s.is_dirty(), "a new project starts saved");
        // the open project has changes: refused unless forced
        s.execute("captions.add", serde_json::json!({"seconds": 1.0, "text": "x"})).unwrap();
        assert!(s.execute("file.newProjectFromTemplate", serde_json::json!({"template": "Podcast"})).is_err());
        s.execute("file.newProjectFromTemplate", serde_json::json!({"template": "Podcast", "force": true})).unwrap();
        assert_eq!(s.active_sequence().unwrap().audio_tracks.len(), 4);
    }
}
