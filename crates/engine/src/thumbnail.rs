//! A YouTube thumbnail (`youtube.thumbnail`): the frame at the playhead (or `time`) at 1280×720,
//! filled and centred, with a little more contrast and colour and a soft vignette, and a big title
//! over it in one of three looks — bold capitals with a heavy outline, white on a red box, black
//! on a yellow banner — written as a PNG (JPEG-sized under YouTube's 2 MB limit).
//!
//! The title is the one given, else the first of the sequence's YouTube titles
//! (`youtube.generate`), else the sequence's name; long titles wrap to two lines.

use filmcraft_project::{Caption, CaptionAlign, CaptionStyle, ClipId};
use filmcraft_time::Tick;
use serde_json::{Value, json};

use crate::commands::{CommandSpec, bad, bool_p, has_seq, str_p, time_p};
use crate::{EngineError, Result, Session};

const CMD: &str = "youtube.thumbnail";
pub const LOOKS: [(&str, &str); 3] = [("bold", "Bold Outline"), ("box", "Red Box"), ("banner", "Yellow Banner")];

fn look(id: &str) -> Option<CaptionStyle> {
    let base = CaptionStyle {
        font: "Inter".into(),
        font_style: "Black".into(),
        size: 150.0,
        uppercase: true,
        align: CaptionAlign::Left,
        line_spacing: 1.05,
        position: Some([0.36, 0.72]),
        ..CaptionStyle::default()
    };
    Some(match id {
        "bold" => CaptionStyle {
            color: [255, 221, 0, 255],
            background: false,
            outline: 12.0,
            outline_color: [0, 0, 0, 255],
            shadow: true,
            shadow_color: [0, 0, 0, 200],
            shadow_offset: [0.0, 8.0],
            shadow_blur: 10.0,
            ..base
        },
        "box" => CaptionStyle { color: [255, 255, 255, 255], background: true, background_color: [225, 29, 42, 255], box_radius: 18.0, ..base },
        "banner" => CaptionStyle { color: [10, 10, 10, 255], background: true, background_color: [255, 214, 0, 255], box_radius: 6.0, ..base },
        _ => return None,
    })
}

/// The title in at most two lines of about 14 characters (capitals are wide).
fn wrap(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().take(8).collect();
    let mut lines: Vec<String> = vec![String::new()];
    for w in words {
        let cur = lines.last_mut().map_or(0, |l| l.chars().count());
        if cur > 0 && cur + 1 + w.chars().count() > 14 && lines.len() < 2 {
            lines.push(String::new());
        }
        if let Some(l) = lines.last_mut() {
            if !l.is_empty() {
                l.push(' ');
            }
            l.push_str(w);
        }
    }
    lines.join("\n")
}

fn thumbnail(s: &mut Session, p: &Value) -> Result<Value> {
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?.clone();
    let style = look(str_p(p, "look").unwrap_or("bold")).ok_or_else(|| bad(CMD, "`look`: bold, box or banner"))?;
    let title = str_p(p, "title")
        .map(str::to_string)
        .or_else(|| crate::youtube::texts_of(s, seq_id).and_then(|t| t.titles.first().cloned()))
        .or_else(|| s.project.item(seq_id).map(|i| i.name.clone()))
        .unwrap_or_default();
    let at = time_p(s, p, "").unwrap_or(s.playhead()).clamp(Tick::ZERO, (q.duration() - Tick(1)).max(Tick::ZERO));
    // the frame, filling 1280×720
    let (fw, fh) = (f64::from(q.settings.width).max(1.0), f64::from(q.settings.height).max(1.0));
    let k = (1280.0 / fw).max(720.0 / fh) as f32;
    let provider = s.media.provider(s.project.clone(), s.services.clone());
    let img = filmcraft_render::render_sequence(
        &s.project,
        seq_id,
        s.sequence_rate().snap(at),
        filmcraft_render::RenderOptions { scale: k, ..Default::default() },
        &provider,
    );
    let (ox, oy) = (img.w.saturating_sub(1280) / 2, img.h.saturating_sub(720) / 2);
    let punch = bool_p(p, "punch").unwrap_or(true);
    let mut px = vec![0.0f32; 1280 * 720 * 4];
    for y in 0..720 {
        for x in 0..1280 {
            let (sx, sy) = ((x + ox).min(img.w.saturating_sub(1)), (y + oy).min(img.h.saturating_sub(1)));
            let mut c = img.get(sx, sy);
            if punch {
                // more colour and contrast, and a soft vignette
                let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                let (dx, dy) = (x as f32 / 1280.0 - 0.5, y as f32 / 720.0 - 0.5);
                let vig = 1.0 - 0.35 * ((dx * dx + dy * dy) * 2.0).min(1.0);
                for ch in c.iter_mut().take(3) {
                    let sat = l + (*ch - l) * 1.25;
                    *ch = (0.18 * (sat.max(0.0) / 0.18).powf(1.12) * vig).max(0.0);
                }
            }
            px[(y * 1280 + x) * 4..(y * 1280 + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 1.0]);
        }
    }
    let cap = Caption {
        id: ClipId(0),
        start: Tick::ZERO,
        duration: Tick(1),
        text: wrap(&title),
        speaker: None,
        cue_id: None,
        settings: String::new(),
        style: None,
        words: Vec::new(),
    };
    if !cap.text.trim().is_empty()
        && let Some(o) = filmcraft_captions::burn::render_caption(&cap, &style, 1280, 720)
    {
        o.composite_onto(&mut px, 1280, 720);
    }
    let out = filmcraft_render::Image { w: 1280, h: 720, px };
    let png = filmcraft_export::encode_png(out.over_black_rgba8(), 1280, 720).map_err(|e| EngineError::Other(e.to_string()))?;
    let path = match str_p(p, "path") {
        Some(x) => std::path::PathBuf::from(crate::export_tools::expand_home(x)),
        None => {
            let name = s.project.item(seq_id).map(|i| i.name.clone()).unwrap_or_else(|| "Sequence".into());
            crate::export_tools::default_export_dir(s).join(crate::export_tools::file_safe(&format!("{name} thumbnail.png")))
        }
    };
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| EngineError::Other(format!("{}: {e}", d.display())))?;
    }
    std::fs::write(&path, &png).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(json!({"path": path.to_string_lossy(), "title": title, "bytes": png.len(), "width": 1280, "height": 720}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "YouTube Thumbnail",
        menu: &[],
        shortcut: None,
        params: r#"{"title":str? (default: the YouTube title, else the sequence name),"look":"bold|box|banner"?,"time":ticks? (default: the playhead),"punch":bool=true,"path":str?}"#,
        enabled: has_seq,
        run: thumbnail,
        journal: false,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbnail_is_a_1280_frame_with_its_title() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let dir = std::env::temp_dir().join(format!("fc-thumb-{}", std::process::id()));
        let mut sizes = Vec::new();
        for (look, _) in LOOKS {
            let path = dir.join(format!("{look}.png"));
            let r = s.execute(CMD, json!({"title": "How to edit faster", "look": look, "path": path.to_string_lossy()})).unwrap();
            assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(1280), Some(720)));
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.len() < 2_000_000, "under YouTube's 2 MB");
            sizes.push(bytes.len());
        }
        // without a title the picture alone is smaller than with one (text adds detail)
        let plain = dir.join("plain.png");
        s.execute(CMD, json!({"title": " ", "path": plain.to_string_lossy()})).unwrap();
        assert!(std::fs::read(&plain).unwrap().len() < sizes[0], "the title is drawn");
        assert_eq!(wrap("How to edit faster in five minutes flat today"), "How to edit\nfaster in five minutes flat", "at most eight words");
        assert!(s.execute(CMD, json!({"look": "nope"})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
