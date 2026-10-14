//! Caption style presets: popular looks for captions and subtitles (short-video captions, cinema
//! subtitles, boxes, neon…) that a click applies to a caption track or to all of them. Names and
//! values are our own. They use the bundled faces only (Inter Regular, Medium, SemiBold, Bold,
//! Italic), so a preset looks the same on every computer.
//!
//! | Command | |
//! |---|---|
//! | `captions.stylePresets` | the presets: `id`, `name`, `style` (as `captions.setStyle` takes it) |
//! | `captions.applyStylePreset` | `{preset, track: id \| "C1" \| "all"}`: the preset replaces the track's style (one undo step) |

use serde_json::{Value, json};

use filmcraft_project::{CaptionAlign, CaptionAnchor, CaptionStyle};

use crate::captions::{has_caption_track, style_json, style_targets};
use crate::commands::{CommandSpec, always, bad, str_p};
use crate::{EngineError, Result, Session};

/// A caption style preset.
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub style: CaptionStyle,
}

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Text without a box in the given face, size and colour.
fn text(style: &str, size: f32, color: [u8; 4]) -> CaptionStyle {
    CaptionStyle { font_style: style.into(), size, color, background: false, ..Default::default() }
}

/// Text in a box with rounded corners.
fn boxed(style: &str, size: f32, color: [u8; 4], box_color: [u8; 4], radius: f32) -> CaptionStyle {
    CaptionStyle { font_style: style.into(), size, color, background: true, background_color: box_color, box_radius: radius, ..Default::default() }
}

fn outlined(mut y: CaptionStyle, width: f32, color: [u8; 4]) -> CaptionStyle {
    y.outline = width;
    y.outline_color = color;
    y
}

fn shadowed(mut y: CaptionStyle, color: [u8; 4], offset: [f32; 2], blur: f32) -> CaptionStyle {
    y.shadow = true;
    y.shadow_color = color;
    y.shadow_offset = offset;
    y.shadow_blur = blur;
    y
}

fn caps(mut y: CaptionStyle) -> CaptionStyle {
    y.uppercase = true;
    y
}

fn at(mut y: CaptionStyle, anchor: CaptionAnchor, margin: f32) -> CaptionStyle {
    y.anchor = anchor;
    y.margin = margin;
    y
}

/// The built-in presets, in gallery order.
pub fn builtin() -> Vec<Preset> {
    use CaptionAnchor::{Bottom, Middle, Top};
    let p = |id, name, style| Preset { id, name, style };
    vec![
        p("classic", "Classic", CaptionStyle::default()),
        p("clean-shadow", "Clean Shadow", shadowed(text("Bold", 56.0, WHITE), [0, 0, 0, 200], [0.0, 3.0], 6.0)),
        p("bold-outline", "Bold Outline", shadowed(outlined(caps(text("Bold", 64.0, WHITE)), 6.0, BLACK), [0, 0, 0, 140], [0.0, 4.0], 4.0)),
        p(
            "yellow-pop",
            "Yellow Pop",
            at(shadowed(outlined(caps(text("Bold", 68.0, [255, 225, 77, 255])), 7.0, BLACK), [0, 0, 0, 200], [0.0, 5.0], 2.0), Bottom, 0.22),
        ),
        p("big-center", "Big Center", at(outlined(caps(text("Bold", 86.0, WHITE)), 8.0, BLACK), Middle, 0.08)),
        p("highlighter", "Highlighter", boxed("SemiBold", 56.0, BLACK, [255, 225, 77, 255], 6.0)),
        p("white-pill", "White Pill", boxed("SemiBold", 52.0, [17, 17, 17, 255], WHITE, 24.0)),
        p("dark-pill", "Dark Pill", boxed("SemiBold", 52.0, WHITE, [0, 0, 0, 204], 24.0)),
        p("subtle-box", "Subtle Box", boxed("Medium", 48.0, WHITE, [0, 0, 0, 115], 8.0)),
        p("cinema", "Cinema", at(shadowed(text("Regular", 46.0, WHITE), [0, 0, 0, 220], [0.0, 2.0], 3.0), Bottom, 0.06)),
        p("minimal", "Minimal", shadowed(text("Medium", 44.0, WHITE), [0, 0, 0, 120], [0.0, 2.0], 8.0)),
        p("news", "News", CaptionStyle { align: CaptionAlign::Left, margin: 0.1, ..boxed("Bold", 50.0, WHITE, [11, 61, 145, 255], 0.0) }),
        p("alert", "Alert", caps(boxed("Bold", 58.0, WHITE, [224, 38, 43, 255], 6.0))),
        p("neon-cyan", "Neon Cyan", shadowed(outlined(text("Bold", 64.0, [95, 245, 255, 255]), 2.0, [10, 42, 64, 255]), [0, 220, 255, 230], [0.0, 0.0], 14.0)),
        p("neon-pink", "Neon Pink", shadowed(outlined(text("Bold", 64.0, [255, 95, 210, 255]), 2.0, [64, 10, 48, 255]), [255, 40, 190, 230], [0.0, 0.0], 14.0)),
        p("lime", "Lime", outlined(caps(text("Bold", 66.0, [182, 255, 59, 255])), 6.0, BLACK)),
        p("top-title", "Top Title", at(outlined(caps(text("Bold", 60.0, WHITE)), 6.0, BLACK), Top, 0.1)),
        p("gold", "Gold", shadowed(outlined(text("Bold", 62.0, [255, 201, 60, 255]), 4.0, [58, 37, 0, 255]), [0, 0, 0, 160], [0.0, 4.0], 4.0)),
        p("comic", "Comic", shadowed(outlined(caps(text("Bold", 70.0, WHITE)), 8.0, BLACK), BLACK, [6.0, 6.0], 0.0)),
        p("retro", "Retro", shadowed(outlined(text("Bold", 62.0, [255, 241, 214, 255]), 5.0, [122, 62, 0, 255]), [217, 72, 15, 255], [5.0, 5.0], 0.0)),
        p("pastel", "Pastel", outlined(text("SemiBold", 58.0, [255, 214, 236, 255]), 5.0, [122, 63, 209, 255])),
        p("violet-box", "Violet Box", boxed("SemiBold", 54.0, WHITE, [110, 68, 255, 255], 14.0)),
        p("sky", "Sky Blue", caps(boxed("Bold", 56.0, WHITE, [30, 144, 255, 255], 10.0))),
        p("italic-quote", "Italic Quote", shadowed(text("Italic", 54.0, WHITE), [0, 0, 0, 180], [0.0, 3.0], 6.0)),
        p("ink", "Ink", outlined(text("Bold", 62.0, BLACK), 6.0, WHITE)),
        p("orange-pop", "Orange Pop", outlined(caps(text("Bold", 66.0, [255, 138, 0, 255])), 6.0, BLACK)),
    ]
}

pub fn find(id: &str) -> Option<Preset> {
    builtin().into_iter().find(|p| p.id == id)
}

fn list(_: &mut Session, _: &Value) -> Result<Value> {
    Ok(json!(builtin().iter().map(|p| json!({"id": p.id, "name": p.name, "style": style_json(&p.style)})).collect::<Vec<_>>()))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.applyStylePreset";
    let id = str_p(p, "preset").ok_or_else(|| bad(cmd, "`preset` (an id from captions.stylePresets) is required"))?;
    let preset = find(id).ok_or_else(|| bad(cmd, format!("unknown preset `{id}`")))?;
    let tracks = style_targets(s, p, cmd)?;
    s.edit("Caption Style", |pr, st| {
        let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
        let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        for tid in &tracks {
            q.caption_track_mut(*tid).ok_or(EngineError::NoSequence)?.style = preset.style.clone();
        }
        Ok(())
    })?;
    Ok(json!({"preset": preset.id, "tracks": tracks.iter().map(|t| t.0).collect::<Vec<_>>()}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "captions.stylePresets",
            label: "List Caption Style Presets",
            menu: &[],
            shortcut: None,
            params: "{}",
            enabled: always,
            run: list,
            journal: false,
        },
        CommandSpec {
            id: "captions.applyStylePreset",
            label: "Apply Caption Style Preset",
            menu: &[],
            shortcut: None,
            params: r#"{"preset":str,"track":id|"C1"|"all"?}"#,
            enabled: has_caption_track,
            run: apply,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_unique_ids_and_bundled_faces() {
        let all = builtin();
        assert!(all.len() >= 25, "{}", all.len());
        let mut ids: Vec<&str> = all.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for p in &all {
            assert_eq!(p.style.font, "Inter", "{}", p.id);
            assert!(["Regular", "Medium", "SemiBold", "Bold", "Italic"].contains(&p.style.font_style.as_str()), "{}", p.id);
        }
        assert_eq!(all[0].style, CaptionStyle::default());
    }

    #[test]
    fn a_preset_styles_one_track_or_all_of_them_in_one_undo_step() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.execute("captions.newTrack", json!({})).unwrap();
        s.execute("captions.newTrack", json!({})).unwrap();
        let styles = |s: &Session| s.active_sequence().unwrap().caption_tracks.iter().map(|t| t.style.clone()).collect::<Vec<_>>();
        s.execute("captions.applyStylePreset", json!({"preset": "yellow-pop", "track": "C2"})).unwrap();
        let yellow = find("yellow-pop").unwrap().style;
        assert_eq!(styles(&s), vec![CaptionStyle::default(), yellow.clone()]);
        s.execute("captions.applyStylePreset", json!({"preset": "dark-pill", "track": "all"})).unwrap();
        let pill = find("dark-pill").unwrap().style;
        assert_eq!(styles(&s), vec![pill.clone(), pill]);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(styles(&s), vec![CaptionStyle::default(), yellow]);
        assert!(s.execute("captions.applyStylePreset", json!({"preset": "nope"})).is_err());
        let listed = s.execute("captions.stylePresets", json!({})).unwrap();
        assert_eq!(listed[3]["style"]["uppercase"], true);
        // captions.setStyle with track "all" changes every track
        s.execute("captions.setStyle", json!({"track": "all", "shadow": true, "shadowOffset": [2, 4], "fontStyle": "Bold"})).unwrap();
        for y in styles(&s) {
            assert!(y.shadow && y.shadow_offset == [2.0, 4.0] && y.font_style == "Bold");
        }
        assert!(s.execute("captions.setStyle", json!({"shadowOffset": [1]})).is_err());
    }
}
