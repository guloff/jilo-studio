//! Caption style presets: popular looks for captions and subtitles (short-video captions, cinema
//! subtitles, boxes, neon…) that a click applies to a caption track or to all of them. Names and
//! values are our own. Most use the bundled faces (Inter Regular, Medium, SemiBold, Bold, Italic);
//! the rest use the caption font pack ([`crate::caption_fonts`], downloaded on request) and draw in
//! Inter until it is installed.
//!
//! | Command | |
//! |---|---|
//! | `captions.stylePresets` | the presets: `id`, `name`, `style` (as `captions.setStyle` takes it) |
//! | `captions.applyStylePreset` | `{preset, track: id \| "C1" \| "all"}`: the preset replaces the track's style (one undo step) |
//! | `captions.saveStylePreset` | `{name, track?}`: the track's style as one of the user's own (`user:<name>`) |
//! | `captions.deleteStylePreset` | `{name}` |
//! | `captions.exportStylePreset` | `{preset, path}`: a style as a `.json` file to share |
//! | `captions.importStylePreset` | `{path}`: a shared style among the user's own |
//!
//! The user's own styles are JSON files in `<data dir>/caption-styles/` (a `dir` parameter points
//! the commands elsewhere); their ids are `user:<name>`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::{CaptionAlign, CaptionAnchor, CaptionAnimation, CaptionHighlight, CaptionStyle};

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

/// Text without a box in a font of the caption font pack.
fn font(family: &str, style: &str, size: f32, color: [u8; 4]) -> CaptionStyle {
    CaptionStyle { font: family.into(), ..text(style, size, color) }
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

/// The spoken word stands out (karaoke), `chunk` words at a time (0 = all), with an entrance.
fn karaoke(mut y: CaptionStyle, how: CaptionHighlight, color: [u8; 4], scale: f32, chunk: u8, animation: CaptionAnimation) -> CaptionStyle {
    y.highlight = how;
    y.highlight_color = color;
    y.highlight_scale = scale;
    y.chunk_words = chunk;
    y.animation = animation;
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
        // the caption font pack
        p("punch", "Punch", shadowed(outlined(caps(font("Montserrat", "Black", 70.0, WHITE)), 7.0, BLACK), [0, 0, 0, 170], [0.0, 5.0], 3.0)),
        p("punch-yellow", "Punch Yellow", outlined(caps(font("Montserrat", "Black", 70.0, [255, 225, 77, 255])), 7.0, BLACK)),
        p("condensed", "Condensed", outlined(caps(font("Oswald", "Bold", 80.0, WHITE)), 5.0, BLACK)),
        p("tall-box", "Tall Box", caps(CaptionStyle { font: "Oswald".into(), ..boxed("SemiBold", 66.0, WHITE, [224, 38, 43, 255], 4.0) })),
        p("heavy", "Heavy", shadowed(caps(font("Fira Sans Condensed", "Black", 76.0, WHITE)), BLACK, [5.0, 5.0], 0.0)),
        p("friendly", "Friendly", CaptionStyle { font: "Nunito".into(), ..boxed("ExtraBold", 58.0, WHITE, [0, 0, 0, 200], 26.0) }),
        p("tech", "Tech", shadowed(outlined(font("Exo 2", "Bold", 64.0, [95, 245, 255, 255]), 3.0, [8, 30, 48, 255]), [0, 200, 255, 160], [0.0, 0.0], 10.0)),
        p("handwritten", "Handwritten", shadowed(font("Caveat", "Bold", 78.0, WHITE), [0, 0, 0, 170], [0.0, 3.0], 6.0)),
        p("elegant", "Elegant", shadowed(font("Raleway", "Medium", 54.0, WHITE), [0, 0, 0, 150], [0.0, 2.0], 8.0)),
        // karaoke: the spoken word stands out
        p(
            "karaoke-yellow",
            "Karaoke Yellow",
            karaoke(outlined(caps(text("Bold", 68.0, WHITE)), 7.0, BLACK), CaptionHighlight::Color, [255, 225, 77, 255], 1.12, 3, CaptionAnimation::Pop),
        ),
        p(
            "karaoke-green",
            "Karaoke Green",
            karaoke(outlined(caps(text("Bold", 68.0, WHITE)), 7.0, BLACK), CaptionHighlight::Color, [57, 255, 120, 255], 1.12, 3, CaptionAnimation::Pop),
        ),
        p("word-box", "Word Box", karaoke(caps(text("Bold", 64.0, WHITE)), CaptionHighlight::Box, [110, 68, 255, 255], 1.0, 3, CaptionAnimation::Fade)),
        p("word-box-red", "Word Box Red", karaoke(caps(text("Bold", 64.0, WHITE)), CaptionHighlight::Box, [224, 38, 43, 255], 1.0, 3, CaptionAnimation::None)),
        p(
            "one-word",
            "One Word",
            at(karaoke(outlined(caps(text("Bold", 96.0, WHITE)), 8.0, BLACK), CaptionHighlight::None, WHITE, 1.0, 1, CaptionAnimation::Pop), Middle, 0.08),
        ),
        p(
            "typewriter",
            "Typewriter",
            karaoke(
                shadowed(text("SemiBold", 56.0, WHITE), [0, 0, 0, 200], [0.0, 3.0], 5.0),
                CaptionHighlight::None,
                WHITE,
                1.0,
                0,
                CaptionAnimation::Typewriter,
            ),
        ),
        p(
            "fade-in",
            "Soft Fade",
            karaoke(shadowed(text("Medium", 52.0, WHITE), [0, 0, 0, 170], [0.0, 2.0], 8.0), CaptionHighlight::None, WHITE, 1.0, 0, CaptionAnimation::Fade),
        ),
        p(
            "slide-up",
            "Slide Up",
            karaoke(boxed("SemiBold", 52.0, WHITE, [0, 0, 0, 200], 22.0), CaptionHighlight::None, WHITE, 1.0, 0, CaptionAnimation::Slide),
        ),
        p(
            "punch-karaoke",
            "Punch Karaoke",
            karaoke(
                shadowed(outlined(caps(font("Montserrat", "Black", 72.0, WHITE)), 7.0, BLACK), [0, 0, 0, 170], [0.0, 5.0], 3.0),
                CaptionHighlight::Color,
                [255, 225, 77, 255],
                1.15,
                3,
                CaptionAnimation::Pop,
            ),
        ),
        p("clean-box", "Clean Box", CaptionStyle { font: "Roboto".into(), ..boxed("Medium", 50.0, WHITE, [0, 0, 0, 160], 10.0) }),
    ]
}

pub fn find(id: &str) -> Option<Preset> {
    builtin().into_iter().find(|p| p.id == id)
}

/// A caption style of the user's own, as saved and shared.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserStyle {
    /// `jilo-caption-style`.
    pub format: String,
    pub name: String,
    pub style: CaptionStyle,
}

const FORMAT: &str = "jilo-caption-style";

/// Where the user's own styles live.
pub fn user_dir() -> Option<PathBuf> {
    crate::autosave::default_data_dir().map(|d| d.join("caption-styles"))
}

fn file_name(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { '-' }).collect();
    format!("{}.json", s.trim_matches('-'))
}

/// The user's own styles in `dir`, by name.
pub fn user_styles_in(dir: &Path) -> Vec<UserStyle> {
    let mut v: Vec<UserStyle> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| std::fs::read(e.path()).ok())
                .filter_map(|b| serde_json::from_slice::<UserStyle>(&b).ok())
                .filter(|u| u.format == FORMAT && !u.name.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|u| u.name.to_lowercase());
    v
}

fn save_in(dir: &Path, u: &UserStyle) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(&u.name));
    std::fs::write(&path, serde_json::to_vec_pretty(u).map_err(std::io::Error::other)?)?;
    Ok(path)
}

fn dir_p(p: &Value, cmd: &str) -> Result<PathBuf> {
    match str_p(p, "dir") {
        Some(d) => Ok(PathBuf::from(d)),
        None => user_dir().ok_or_else(|| bad(cmd, "no data directory for caption styles")),
    }
}

/// The style `id` names: a built-in preset id, or `user:<name>` (in `dir`).
pub fn style_of(id: &str, dir: Option<&Path>) -> Option<(String, CaptionStyle)> {
    match id.strip_prefix("user:") {
        Some(name) => dir.and_then(|d| user_styles_in(d).into_iter().find(|u| u.name == name)).map(|u| (u.name, u.style)),
        None => find(id).map(|p| (p.name.to_string(), p.style)),
    }
}

fn list(_: &mut Session, p: &Value) -> Result<Value> {
    let mut v: Vec<Value> = builtin().iter().map(|p| json!({"id": p.id, "name": p.name, "style": style_json(&p.style)})).collect();
    if let Ok(d) = dir_p(p, "captions.stylePresets") {
        v.extend(user_styles_in(&d).iter().map(|u| json!({"id": format!("user:{}", u.name), "name": u.name, "user": true, "style": style_json(&u.style)})));
    }
    Ok(json!(v))
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.saveStylePreset";
    let name =
        str_p(p, "name").map(str::trim).filter(|n| !n.is_empty() && n.chars().count() <= 60).ok_or_else(|| bad(cmd, "`name`: what to call the style"))?;
    let track = *style_targets(s, p, cmd)?.first().ok_or_else(|| bad(cmd, "the sequence has no caption track"))?;
    let style = s.active_sequence().and_then(|q| q.caption_tracks.iter().find(|t| t.id == track)).map(|t| t.style.clone()).ok_or(EngineError::NoSequence)?;
    let u = UserStyle { format: FORMAT.into(), name: name.to_string(), style };
    let path = save_in(&dir_p(p, cmd)?, &u).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"id": format!("user:{name}"), "path": path.to_string_lossy()}))
}

fn delete(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.deleteStylePreset";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "`name`"))?;
    let name = name.strip_prefix("user:").unwrap_or(name);
    let d = dir_p(p, cmd)?;
    if !user_styles_in(&d).iter().any(|u| u.name == name) {
        return Err(bad(cmd, format!("no style of yours named `{name}`")));
    }
    std::fs::remove_file(d.join(file_name(name))).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"deleted": name}))
}

fn export(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.exportStylePreset";
    let id = str_p(p, "preset").ok_or_else(|| bad(cmd, "`preset`"))?;
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "`path`: the .json file to write"))?;
    let d = dir_p(p, cmd).ok();
    let (name, style) = style_of(id, d.as_deref()).ok_or_else(|| bad(cmd, format!("unknown preset `{id}`")))?;
    let u = UserStyle { format: FORMAT.into(), name, style };
    std::fs::write(path, serde_json::to_vec_pretty(&u).map_err(|e| EngineError::Other(e.to_string()))?)
        .map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"path": path}))
}

fn import(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.importStylePreset";
    let path = str_p(p, "path").ok_or_else(|| bad(cmd, "`path`: a caption style .json"))?;
    let bytes = std::fs::read(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    let u: UserStyle = serde_json::from_slice(&bytes).map_err(|e| bad(cmd, format!("{path} is not a caption style: {e}")))?;
    if u.format != FORMAT || u.name.trim().is_empty() {
        return Err(bad(cmd, format!("{path} is not a Jilo Studio caption style")));
    }
    save_in(&dir_p(p, cmd)?, &u).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"id": format!("user:{}", u.name), "name": u.name}))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "captions.applyStylePreset";
    let id = str_p(p, "preset").ok_or_else(|| bad(cmd, "`preset` (an id from captions.stylePresets) is required"))?;
    let d = dir_p(p, cmd).ok();
    let (_, style) = style_of(id, d.as_deref()).ok_or_else(|| bad(cmd, format!("unknown preset `{id}`")))?;
    // phrases set apart: the look becomes their own
    if let Some(ids) = p.get("captions").and_then(Value::as_array) {
        let ids: Vec<filmcraft_project::ClipId> = ids.iter().filter_map(Value::as_u64).map(filmcraft_project::ClipId).collect();
        let n = s.edit("Caption Style (Phrase)", |pr, st| {
            let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
            let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
            let mut n = 0;
            for c in q.caption_tracks.iter_mut().flat_map(|t| t.captions.iter_mut()).filter(|c| ids.contains(&c.id)) {
                c.style = Some(Box::new(style.clone()));
                n += 1;
            }
            Ok(n)
        })?;
        return Ok(json!({"preset": id, "captions": n}));
    }
    let tracks = style_targets(s, p, cmd)?;
    s.edit("Caption Style", |pr, st| {
        let seq = st.active_sequence.ok_or(EngineError::NoSequence)?;
        let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
        for tid in &tracks {
            q.caption_track_mut(*tid).ok_or(EngineError::NoSequence)?.style = style.clone();
        }
        Ok(())
    })?;
    Ok(json!({"preset": id, "tracks": tracks.iter().map(|t| t.0).collect::<Vec<_>>()}))
}

/// The parameters of a graphic's text layer that give it a caption look: font, size, capitals,
/// tracking, colour, outline, box (padding, corners) and shadow.
pub fn text_layer_params(y: &CaptionStyle) -> Vec<(&'static str, filmcraft_project::ParamValue)> {
    use filmcraft_project::ParamValue as V;
    let col = |c: [u8; 4]| V::Color([c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0]);
    let alpha = |c: [u8; 4]| V::Float(f64::from(c[3]) / 255.0 * 100.0);
    let (dx, dy) = (f64::from(y.shadow_offset[0]), f64::from(y.shadow_offset[1]));
    vec![
        ("font", V::Text(y.font.clone())),
        ("font_style", V::Text(y.font_style.clone())),
        ("size", V::Float(f64::from(y.size))),
        ("caps", V::Choice(u32::from(y.uppercase))),
        ("tracking", V::Float(f64::from(y.tracking))),
        ("fill", V::Bool(true)),
        ("fill_kind", V::Choice(0)),
        ("fill_color", col(y.color)),
        ("stroke", V::Bool(y.outline > 0.0)),
        ("stroke_color", col(y.outline_color)),
        ("stroke_width", V::Float(f64::from(y.outline))),
        ("stroke_type", V::Choice(0)),
        ("background", V::Bool(y.background && y.background_color[3] > 0)),
        ("background_color", col(y.background_color)),
        ("background_opacity", alpha(y.background_color)),
        ("background_size", V::Float(f64::from(y.size * 0.3).round())),
        ("background_radius", V::Float(f64::from(y.box_radius))),
        ("shadow", V::Bool(y.shadow && y.shadow_color[3] > 0)),
        ("shadow_color", col(y.shadow_color)),
        ("shadow_opacity", alpha(y.shadow_color)),
        ("shadow_distance", V::Float((dx * dx + dy * dy).sqrt())),
        ("shadow_angle", V::Float((dy.atan2(dx).to_degrees() + 90.0).rem_euclid(360.0))),
        ("shadow_blur", V::Float(f64::from(y.shadow_blur))),
    ]
}

/// `graphics.applyCaptionStyle`: a caption look (a preset or the user's own) on a graphic's text
/// layers — the chosen ones, else the selected, else all. One undo step.
fn apply_to_text(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "graphics.applyCaptionStyle";
    let id = str_p(p, "preset").ok_or_else(|| bad(cmd, "`preset` (an id from captions.stylePresets) is required"))?;
    let d = dir_p(p, cmd).ok();
    let (_, style) = style_of(id, d.as_deref()).ok_or_else(|| bad(cmd, format!("unknown preset `{id}`")))?;
    let clip = crate::graphics::target_clip(s, p).ok_or_else(|| bad(cmd, "select a graphic (a title) first"))?;
    let chosen: Vec<usize> = match p.get("layers").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_u64).map(|v| v as usize).collect(),
        None => s.state.graphic_layers.clone(),
    };
    let params = text_layer_params(&style);
    let n = s.edit_sequence("Caption Look on Text", |q, _, _| {
        let (_, it) = q.find_item_mut(clip).ok_or(filmcraft_edit::EditError::NoItem(clip))?;
        // layer indices count the clip's layers (graphic effects) in order
        let mut n = 0;
        for (this, e) in it.effects.iter_mut().filter(|e| filmcraft_project::graphic::is_layer(e)).enumerate() {
            if e.effect != filmcraft_project::graphic::TEXT_LAYER || !(chosen.is_empty() || chosen.contains(&this)) {
                continue;
            }
            for (k, v) in &params {
                e.params.insert((*k).to_string(), filmcraft_project::Param::new(v.clone()));
            }
            n += 1;
        }
        if n == 0 {
            return Err(bad(cmd, "the graphic has no text layer to style"));
        }
        Ok(n)
    })?;
    Ok(json!({"preset": id, "clip": clip.0, "layers": n}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "graphics.applyCaptionStyle",
            label: "Apply Caption Look to Text",
            menu: &[],
            shortcut: None,
            params: r#"{"preset":str,"clip":id?,"layers":[n]? (default: the selected layers, else every text layer),"dir":path?}"#,
            enabled: always,
            run: apply_to_text,
            journal: true,
        },
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
            params: r#"{"preset":str,"track":id|"C1"|"all"?,"captions":[id]? (these phrases take the look as their own)}"#,
            enabled: has_caption_track,
            run: apply,
            journal: true,
        },
        CommandSpec {
            id: "captions.saveStylePreset",
            label: "Save Caption Style",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str,"track":id|"C1"?,"dir":path?}"#,
            enabled: has_caption_track,
            run: save,
            journal: false,
        },
        CommandSpec {
            id: "captions.deleteStylePreset",
            label: "Delete Caption Style",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str,"dir":path?}"#,
            enabled: always,
            run: delete,
            journal: false,
        },
        CommandSpec {
            id: "captions.exportStylePreset",
            label: "Export Caption Style",
            menu: &[],
            shortcut: None,
            params: r#"{"preset":id,"path":file,"dir":path?}"#,
            enabled: always,
            run: export,
            journal: false,
        },
        CommandSpec {
            id: "captions.importStylePreset",
            label: "Import Caption Style",
            menu: &[],
            shortcut: None,
            params: r#"{"path":file,"dir":path?}"#,
            enabled: always,
            run: import,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_unique_ids_and_bundled_faces() {
        let all = builtin();
        assert!(all.len() >= 35, "{}", all.len());
        let mut ids: Vec<&str> = all.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for p in &all {
            if p.style.font == "Inter" {
                assert!(["Regular", "Medium", "SemiBold", "Bold", "Italic"].contains(&p.style.font_style.as_str()), "{}", p.id);
            } else {
                assert!(crate::caption_fonts::FAMILIES.contains(&p.style.font.as_str()), "{} uses {}, not in the caption font pack", p.id, p.style.font);
            }
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

    #[test]
    fn own_styles_are_saved_shared_and_applied() {
        let d = std::env::temp_dir().join(format!("jilo-cap-styles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dir = d.join("mine").to_string_lossy().to_string();
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.execute("captions.newTrack", json!({})).unwrap();
        s.execute("captions.applyStylePreset", json!({"preset": "neon-cyan", "track": "C1"})).unwrap();
        s.execute("captions.setStyle", json!({"track": "C1", "size": 77.0})).unwrap();
        let r = s.execute("captions.saveStylePreset", json!({"name": "Channel Look", "track": "C1", "dir": dir})).unwrap();
        assert_eq!(r["id"], "user:Channel Look");
        let listed = s.execute("captions.stylePresets", json!({"dir": dir})).unwrap();
        assert!(listed.as_array().unwrap().iter().any(|p| p["id"] == "user:Channel Look" && p["user"] == true));
        // shared as a file and taken in elsewhere
        let file = d.join("look.json").to_string_lossy().to_string();
        s.execute("captions.exportStylePreset", json!({"preset": "user:Channel Look", "path": file, "dir": dir})).unwrap();
        let other = d.join("theirs").to_string_lossy().to_string();
        assert_eq!(s.execute("captions.importStylePreset", json!({"path": file, "dir": other})).unwrap()["id"], "user:Channel Look");
        // applied like a built-in one
        s.execute("captions.applyStylePreset", json!({"preset": "classic", "track": "C1"})).unwrap();
        s.execute("captions.applyStylePreset", json!({"preset": "user:Channel Look", "track": "C1", "dir": other})).unwrap();
        assert_eq!(s.active_sequence().unwrap().caption_tracks[0].style.size, 77.0);
        s.execute("captions.deleteStylePreset", json!({"name": "user:Channel Look", "dir": dir})).unwrap();
        assert!(user_styles_in(Path::new(&dir)).is_empty());
        std::fs::write(d.join("bad.json"), b"{}").unwrap();
        assert!(s.execute("captions.importStylePreset", json!({"path": d.join("bad.json").to_string_lossy(), "dir": other})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_caption_look_goes_on_a_titles_text() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let r = s.execute("graphics.newText", json!({"text": "Title"})).unwrap();
        let clip = filmcraft_project::ClipId(r["clip"].as_u64().unwrap());
        let undo = s.history.undo.len();
        s.execute("graphics.applyCaptionStyle", json!({"preset": "yellow-pop", "clip": clip.0})).unwrap();
        assert_eq!(s.history.undo.len(), undo + 1);
        let want = find("yellow-pop").unwrap().style;
        let (_, it) = s.active_sequence().unwrap().find_item(clip).unwrap();
        let e = it.effects.iter().find(|e| e.effect == filmcraft_project::graphic::TEXT_LAYER).unwrap();
        let at = filmcraft_time::Tick::ZERO;
        assert_eq!(e.params["font"].value_at(at), filmcraft_project::ParamValue::Text(want.font.clone()));
        let filmcraft_project::ParamValue::Color(c) = e.params["fill_color"].value_at(at) else { panic!("colour") };
        assert_eq!((c[0] * 255.0).round() as u8, want.color[0]);
        assert_eq!(e.params["caps"].value_at(at), filmcraft_project::ParamValue::Choice(u32::from(want.uppercase)));
        assert!(s.execute("graphics.applyCaptionStyle", json!({"preset": "nope", "clip": clip.0})).is_err());
    }
}
