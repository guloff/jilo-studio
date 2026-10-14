//! Graphics templates (`.fcgt`): FilmCraft's own format for reusable graphics with editable
//! properties, the counterpart of motion graphics templates in other editors.
//!
//! A template file is UTF-8 JSON ([`GraphicsTemplate`]):
//!
//! ```json
//! {
//!   "format": "filmcraft.graphicsTemplate",
//!   "version": 1,
//!   "id": "user:my-lower-third",
//!   "name": "My Lower Third",
//!   "category": "Lower Thirds",
//!   "description": "", "author": "", "license": "", "tags": [],
//!   "canvas": [1920, 1080],
//!   "duration": 1270080000000,
//!   "layers": [ /* graphic_text / graphic_shape effect instances, as in a project file */ ],
//!   "graphic": { /* roll, intro / outro (GraphicMeta) */ },
//!   "controls": [ { "id": "name", "name": "Name", "kind": "text", "layer": 1, "param": "text" } ],
//!   "resources": { "fonts": [ { "family": "…", "style": "…", "license": "OFL-1.1", "data": "<base64>" } ] }
//! }
//! ```
//!
//! - `version` is the format version; readers refuse newer versions.
//! - Layers are referenced by their [`LayerExtra::uid`](crate::graphic_design::LayerExtra); every
//!   layer of a template has one.
//! - `controls` are the properties a user of the template edits (Essential Graphics ▸ Edit):
//!   text, colour, slider (with `min` / `max`), checkbox, font and position, each bound to one
//!   layer parameter (`enabled` = the layer's visibility).
//! - `resources.fonts` optionally embeds font files (base64) that the template needs; installing
//!   the template registers them. Only embed fonts whose licence allows redistribution.
//!
//! FilmCraft never reads Adobe `.mogrt` files (or any other application's template format);
//! [`GraphicsTemplate::from_bytes`] rejects ZIP archives with a message saying so.
//!
//! The built-in templates ([`builtin_templates`]) are original designs made for FilmCraft in code
//! (no image, font or template files), under the project licence.

use filmcraft_geom::Vec2;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};

use crate::effect::EffectInstance;
use crate::graphic::{self, new_shape_layer, new_text_layer};
use crate::graphic_design::{GraphicMeta, LayerExtra, Pin, PinTarget, Roll, RollMode};
use crate::keyframe::{Param, ParamValue};

pub const TEMPLATE_FORMAT: &str = "filmcraft.graphicsTemplate";
pub const TEMPLATE_VERSION: u32 = 1;
pub const TEMPLATE_EXTENSION: &str = "fcgt";

/// Kind of an editable template property.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlKind {
    #[default]
    Text,
    Color,
    Slider,
    Checkbox,
    Font,
    Position,
}

impl ControlKind {
    pub const ALL: [ControlKind; 6] =
        [ControlKind::Text, ControlKind::Color, ControlKind::Slider, ControlKind::Checkbox, ControlKind::Font, ControlKind::Position];
    pub fn name(self) -> &'static str {
        match self {
            ControlKind::Text => "text",
            ControlKind::Color => "color",
            ControlKind::Slider => "slider",
            ControlKind::Checkbox => "checkbox",
            ControlKind::Font => "font",
            ControlKind::Position => "position",
        }
    }
    pub fn from_name(s: &str) -> Option<ControlKind> {
        let s = s.to_ascii_lowercase();
        let s = if s == "colour" { "color".to_string() } else { s };
        ControlKind::ALL.into_iter().find(|k| k.name() == s)
    }
    /// The kind that edits parameter `param` of a layer.
    pub fn for_param(param: &str) -> ControlKind {
        match param {
            "text" | "name" => ControlKind::Text,
            "font" | "font_style" => ControlKind::Font,
            "position" | "anchor" => ControlKind::Position,
            "enabled" => ControlKind::Checkbox,
            p if p.ends_with("_color") => ControlKind::Color,
            p => match crate::find_effect(graphic::TEXT_LAYER)
                .and_then(|d| d.param(p))
                .or_else(|| crate::find_effect(graphic::SHAPE_LAYER).and_then(|d| d.param(p)))
            {
                Some(d) => match &d.default {
                    ParamValue::Bool(_) => ControlKind::Checkbox,
                    ParamValue::Color(_) => ControlKind::Color,
                    ParamValue::Vec2(_) => ControlKind::Position,
                    ParamValue::Text(_) => ControlKind::Text,
                    _ => ControlKind::Slider,
                },
                None => ControlKind::Slider,
            },
        }
    }
}

/// One editable property of a template (Essential Graphics ▸ Edit).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateControl {
    /// Stable id (`[a-z0-9_-]`), unique in the template.
    pub id: String,
    /// Label shown to the user.
    pub name: String,
    pub kind: ControlKind,
    /// The layer (by uid) and parameter id it edits (`enabled` = layer visibility).
    pub layer: u64,
    pub param: String,
    /// Slider range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// The template a placed graphic came from (kept on the clip, `GraphicMeta::template`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateLink {
    pub id: String,
    pub name: String,
    pub controls: Vec<TemplateControl>,
}

/// A font embedded in a template.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontResource {
    pub family: String,
    pub style: String,
    /// SPDX id of the font's licence (must allow redistribution).
    pub license: String,
    /// Original file name (informational).
    pub file: String,
    /// The font file, base64.
    pub data: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Resources {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fonts: Vec<FontResource>,
}

impl Resources {
    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }
}

/// A graphics template (`.fcgt`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsTemplate {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    /// Frame size the template was designed for.
    pub canvas: [u32; 2],
    /// Default clip duration.
    pub duration: Tick,
    pub layers: Vec<EffectInstance>,
    /// Roll / responsive time settings (`template` is not stored here).
    pub graphic: GraphicMeta,
    pub controls: Vec<TemplateControl>,
    #[serde(skip_serializing_if = "Resources::is_empty")]
    pub resources: Resources,
}

impl Default for GraphicsTemplate {
    fn default() -> Self {
        GraphicsTemplate {
            format: TEMPLATE_FORMAT.into(),
            version: TEMPLATE_VERSION,
            id: String::new(),
            name: String::new(),
            category: "My Templates".into(),
            description: String::new(),
            author: String::new(),
            license: String::new(),
            tags: Vec::new(),
            canvas: [1920, 1080],
            duration: Tick(5 * TICKS_PER_SECOND),
            layers: Vec::new(),
            graphic: GraphicMeta::default(),
            controls: Vec::new(),
            resources: Resources::default(),
        }
    }
}

/// Problems reading or validating a template.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TemplateError {
    #[error(
        "this file is a ZIP archive (an Adobe .mogrt or another application's package?): FilmCraft does not read motion graphics templates from other applications, only its own .fcgt graphics templates"
    )]
    ForeignPackage,
    #[error("not a FilmCraft graphics template: {0}")]
    NotTemplate(String),
    #[error("this graphics template was made by a newer FilmCraft (format version {0}; this build reads up to {TEMPLATE_VERSION})")]
    TooNew(u32),
    #[error("invalid graphics template: {0}")]
    Invalid(String),
}

/// Lower-case slug of a name (`My Lower Third` → `my-lower-third`).
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "template".into() } else { s }
}

impl GraphicsTemplate {
    /// Serialize (pretty JSON).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Read a template file. ZIP archives (such as `.mogrt` packages) are refused without being
    /// inspected.
    pub fn from_bytes(bytes: &[u8]) -> Result<GraphicsTemplate, TemplateError> {
        if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            return Err(TemplateError::ForeignPackage);
        }
        let text =
            std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)).map_err(|_| TemplateError::NotTemplate("not UTF-8 text".into()))?;
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| TemplateError::NotTemplate(e.to_string()))?;
        if v.get("format").and_then(|f| f.as_str()) != Some(TEMPLATE_FORMAT) {
            return Err(TemplateError::NotTemplate(format!("`format` is not \"{TEMPLATE_FORMAT}\"")));
        }
        let ver = v.get("version").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if ver > TEMPLATE_VERSION {
            return Err(TemplateError::TooNew(ver));
        }
        if ver == 0 {
            return Err(TemplateError::Invalid("missing `version`".into()));
        }
        let t: GraphicsTemplate = serde_json::from_value(v).map_err(|e| TemplateError::Invalid(e.to_string()))?;
        t.validate()?;
        Ok(t)
    }

    /// Check the template's structure: layers are graphic layers with unique uids, controls
    /// point at existing layers and parameters, ids are unique.
    pub fn validate(&self) -> Result<(), TemplateError> {
        let bad = |m: String| Err(TemplateError::Invalid(m));
        if self.name.trim().is_empty() {
            return bad("the template has no name".into());
        }
        if self.layers.is_empty() {
            return bad("the template has no layers".into());
        }
        if self.canvas[0] == 0 || self.canvas[1] == 0 || self.canvas[0] > 16384 || self.canvas[1] > 16384 {
            return bad(format!("bad canvas size {:?}", self.canvas));
        }
        let mut uids = Vec::new();
        for (i, l) in self.layers.iter().enumerate() {
            if !graphic::is_layer(l) {
                return bad(format!("layer {i} is `{}`, not a graphic layer", l.effect));
            }
            let uid = layer_uid(l);
            if uid == 0 || uids.contains(&uid) {
                return bad(format!("layer {i} has no unique uid"));
            }
            uids.push(uid);
        }
        let mut ids = Vec::new();
        for c in &self.controls {
            if c.id.is_empty() || ids.contains(&c.id) {
                return bad(format!("control `{}` has no unique id", c.name));
            }
            ids.push(c.id.clone());
            let Some(l) = self.layers.iter().find(|l| layer_uid(l) == c.layer) else {
                return bad(format!("control `{}` points at a missing layer", c.name));
            };
            if c.param != "enabled" && !l.params.contains_key(&c.param) {
                return bad(format!("control `{}`: the layer has no parameter `{}`", c.name, c.param));
            }
        }
        for f in &self.resources.fonts {
            if base64_decode(&f.data).is_none() {
                return bad(format!("font `{}` is not valid base64", f.family));
            }
        }
        Ok(())
    }

    /// The layers and settings for a graphic clip on a `canvas`-sized frame. When the frame size
    /// differs from the template's, the layers are scaled uniformly (fit) and centred, and pin
    /// distances scale with them. The result links back to the template.
    pub fn instantiate(&self, canvas: (u32, u32)) -> (Vec<EffectInstance>, GraphicMeta) {
        let (w, h) = (canvas.0 as f64, canvas.1 as f64);
        let (tw, th) = (self.canvas[0] as f64, self.canvas[1] as f64);
        let s = (w / tw).min(h / th);
        let off = Vec2::new((w - tw * s) / 2.0, (h - th * s) / 2.0);
        let mut layers = self.layers.clone();
        if (s - 1.0).abs() > 1e-9 || off.x.abs() > 1e-9 || off.y.abs() > 1e-9 {
            for l in &mut layers {
                if let Some(p) = l.params.get_mut("position") {
                    p.map_values(|v| match v {
                        ParamValue::Vec2(q) if !q.x.is_nan() => ParamValue::Vec2(Vec2::new(q.x * s + off.x, q.y * s + off.y)),
                        o => o,
                    });
                }
                if let Some(p) = l.params.get_mut("scale") {
                    p.map_values(|v| match v {
                        ParamValue::Float(x) => ParamValue::Float(x * s),
                        o => o,
                    });
                }
                if let Some(pin) = l.layer.as_mut().and_then(|x| x.pin.as_mut()) {
                    pin.offsets.iter_mut().for_each(|o| *o *= s);
                }
            }
        }
        let mut meta = self.graphic.clone();
        meta.template = Some(TemplateLink { id: self.id.clone(), name: self.name.clone(), controls: self.controls.clone() });
        (layers, meta)
    }
}

/// A layer's uid (0 = none).
pub fn layer_uid(e: &EffectInstance) -> u64 {
    e.layer.as_ref().map_or(0, |x| x.uid)
}

/// Give every layer without a uid a fresh one (unique among `layers`).
pub fn ensure_uids(layers: &mut [EffectInstance]) {
    let mut next = layers.iter().map(layer_uid).max().unwrap_or(0) + 1;
    for l in layers.iter_mut() {
        if layer_uid(l) == 0 {
            l.layer.get_or_insert_with(Default::default).uid = next;
            next += 1;
        }
    }
}

// ------------------------------------------------------------------------------------------------
// base64 (RFC 4648, standard alphabet, padded)
// ------------------------------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for b in s.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if b == b'=' {
            break;
        }
        let v = B64.iter().position(|&x| x == b)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ------------------------------------------------------------------------------------------------
// Built-in templates (original designs)
// ------------------------------------------------------------------------------------------------

fn set(e: &mut EffectInstance, id: &str, v: ParamValue) {
    e.params.insert(id.to_string(), Param::new(v));
}

fn rgb(hex: u32) -> ParamValue {
    let c = |s: u32| ((hex >> s) & 0xff) as f32 / 255.0;
    ParamValue::Color([c(16), c(8), c(0), 1.0])
}

fn text(uid: u64, name: &str, s: &str, pos: (f64, f64), size: f64, style: &str) -> EffectInstance {
    let mut e = new_text_layer(s, Vec2::new(pos.0, pos.1), size);
    set(&mut e, "name", ParamValue::Text(name.into()));
    set(&mut e, "font", ParamValue::Text("Inter".into()));
    set(&mut e, "font_style", ParamValue::Text(style.into()));
    e.layer = Some(Box::new(LayerExtra { uid, ..Default::default() }));
    e
}

fn shape(uid: u64, name: &str, kind: u32, pos: (f64, f64), size: (f64, f64), color: u32) -> EffectInstance {
    let mut e = new_shape_layer(kind, Vec2::new(pos.0, pos.1), Vec2::new(size.0, size.1), vec![]);
    set(&mut e, "name", ParamValue::Text(name.into()));
    set(&mut e, "fill_color", rgb(color));
    e.layer = Some(Box::new(LayerExtra { uid, ..Default::default() }));
    e
}

fn pin(e: &mut EffectInstance, to: u64, edges: [bool; 4], offsets: [f64; 4]) {
    let p =
        Pin { to: if to == 0 { PinTarget::Frame } else { PinTarget::Layer(to) }, left: edges[0], top: edges[1], right: edges[2], bottom: edges[3], offsets };
    e.layer.get_or_insert_with(Default::default).pin = Some(p);
}

fn ctl(id: &str, name: &str, layer: u64, param: &str) -> TemplateControl {
    TemplateControl { id: id.into(), name: name.into(), kind: ControlKind::for_param(param), layer, param: param.into(), min: None, max: None }
}

fn slider(id: &str, name: &str, layer: u64, param: &str, min: f64, max: f64) -> TemplateControl {
    TemplateControl { min: Some(min), max: Some(max), ..ctl(id, name, layer, param) }
}

fn builtin(
    id: &str,
    name: &str,
    category: &str,
    description: &str,
    seconds: i64,
    layers: Vec<EffectInstance>,
    controls: Vec<TemplateControl>,
) -> GraphicsTemplate {
    GraphicsTemplate {
        id: format!("builtin:{id}"),
        name: name.into(),
        category: category.into(),
        description: description.into(),
        author: "FilmCraft contributors".into(),
        license: "MIT OR Apache-2.0".into(),
        tags: vec![category.to_ascii_lowercase()],
        duration: Tick(seconds * TICKS_PER_SECOND),
        layers,
        controls,
        ..Default::default()
    }
}

/// The built-in templates: original FilmCraft designs (lower thirds, titles, end credits,
/// callouts, a ticker), defined here in code. Designed at 1920×1080 with the bundled Inter font.
pub fn builtin_templates() -> Vec<GraphicsTemplate> {
    let mut v = Vec::new();

    // Lower third: a slab that grows with the name, a small accent square and a role line.
    {
        let mut slab = shape(1, "Slab", 0, (400.0, 900.0), (560.0, 150.0), 0x1d4ed8);
        set(&mut slab, "opacity", ParamValue::Float(92.0));
        pin(&mut slab, 2, [true, false, true, false], [-40.0, 0.0, 48.0, 0.0]);
        let mut accent = shape(4, "Accent", 0, (130.0, 900.0), (12.0, 150.0), 0xfacc15);
        pin(&mut accent, 1, [true, false, false, false], [-12.0, 0.0, 0.0, 0.0]);
        let mut name = text(2, "Name", "Alex Morgan", (160.0, 895.0), 62.0, "SemiBold");
        set(&mut name, "fill_color", rgb(0xffffff));
        let mut role = text(3, "Role", "Field Producer", (162.0, 950.0), 36.0, "Regular");
        set(&mut role, "fill_color", rgb(0xdbeafe));
        v.push(builtin(
            "lower-third-slab",
            "Lower Third – Slab",
            "Lower Thirds",
            "Name and role on a coloured slab that widens with the name.",
            6,
            vec![slab, accent, name, role],
            vec![
                ctl("name", "Name", 2, "text"),
                ctl("role", "Role", 3, "text"),
                ctl("slab_color", "Slab Color", 1, "fill_color"),
                ctl("accent_color", "Accent Color", 4, "fill_color"),
                ctl("text_color", "Name Color", 2, "fill_color"),
                ctl("show_role", "Show Role", 3, "enabled"),
                ctl("font", "Font", 2, "font"),
            ],
        ));
    }
    // Lower third: name with a rule underneath that stretches to the name's width.
    {
        let mut name = text(1, "Name", "Sam Rivera", (140.0, 900.0), 58.0, "Bold");
        let shadow = |e: &mut EffectInstance| {
            set(e, "shadow", ParamValue::Bool(true));
            set(e, "shadow_distance", ParamValue::Float(3.0));
            set(e, "shadow_blur", ParamValue::Float(12.0));
            set(e, "shadow_opacity", ParamValue::Float(60.0));
        };
        shadow(&mut name);
        let mut rule = shape(2, "Rule", 0, (300.0, 920.0), (320.0, 5.0), 0x22d3ee);
        pin(&mut rule, 1, [true, false, true, false], [0.0, 0.0, 0.0, 0.0]);
        let mut role = text(3, "Role", "Lead Engineer", (140.0, 966.0), 34.0, "Medium");
        set(&mut role, "tracking", ParamValue::Float(40.0));
        shadow(&mut role);
        v.push(builtin(
            "lower-third-rule",
            "Lower Third – Rule",
            "Lower Thirds",
            "Name over a thin rule that always matches the name's width.",
            6,
            vec![name, rule, role],
            vec![
                ctl("name", "Name", 1, "text"),
                ctl("role", "Role", 3, "text"),
                ctl("rule_color", "Rule Color", 2, "fill_color"),
                ctl("font", "Font", 1, "font"),
                slider("name_size", "Name Size", 1, "size", 24.0, 140.0),
            ],
        ));
    }
    // Centred title with a subtitle and a short rule between them.
    {
        let mut title = text(1, "Title", "THE LONG WAY HOME", (960.0, 520.0), 112.0, "Bold");
        set(&mut title, "align", ParamValue::Choice(1));
        set(&mut title, "tracking", ParamValue::Float(60.0));
        let mut rule = shape(2, "Rule", 0, (960.0, 556.0), (220.0, 4.0), 0xf97316);
        set(&mut rule, "opacity", ParamValue::Float(90.0));
        let mut sub = text(3, "Subtitle", "a story in three parts", (960.0, 628.0), 44.0, "Italic");
        set(&mut sub, "align", ParamValue::Choice(1));
        set(&mut sub, "fill_color", rgb(0xe5e7eb));
        v.push(builtin(
            "title-centered",
            "Title – Centered",
            "Titles",
            "Large centred title, accent rule and subtitle.",
            5,
            vec![title, rule, sub],
            vec![
                ctl("title", "Title", 1, "text"),
                ctl("subtitle", "Subtitle", 3, "text"),
                slider("title_size", "Title Size", 1, "size", 40.0, 240.0),
                ctl("title_color", "Title Color", 1, "fill_color"),
                ctl("accent", "Accent Color", 2, "fill_color"),
                ctl("font", "Font", 1, "font"),
            ],
        ));
    }
    // Title in a box (text background) with a soft shadow.
    {
        let mut title = text(1, "Title", "Chapter One", (960.0, 560.0), 96.0, "SemiBold");
        set(&mut title, "align", ParamValue::Choice(1));
        set(&mut title, "fill_color", rgb(0x111827));
        set(&mut title, "background", ParamValue::Bool(true));
        set(&mut title, "background_color", rgb(0xfef3c7));
        set(&mut title, "background_opacity", ParamValue::Float(100.0));
        set(&mut title, "background_size", ParamValue::Float(36.0));
        set(&mut title, "background_radius", ParamValue::Float(10.0));
        set(&mut title, "shadow", ParamValue::Bool(true));
        set(&mut title, "shadow_distance", ParamValue::Float(8.0));
        set(&mut title, "shadow_blur", ParamValue::Float(30.0));
        set(&mut title, "shadow_opacity", ParamValue::Float(45.0));
        v.push(builtin(
            "title-boxed",
            "Title – Boxed",
            "Titles",
            "A title on a rounded card with a soft shadow.",
            5,
            vec![title],
            vec![
                ctl("title", "Title", 1, "text"),
                ctl("text_color", "Text Color", 1, "fill_color"),
                ctl("box_color", "Box Color", 1, "background_color"),
                slider("box_opacity", "Box Opacity", 1, "background_opacity", 0.0, 100.0),
                ctl("position", "Position", 1, "position"),
            ],
        ));
    }
    // End credits that roll from below the frame to above it.
    {
        let mut heading = text(1, "Heading", "CREDITS", (960.0, 160.0), 72.0, "Bold");
        set(&mut heading, "align", ParamValue::Choice(1));
        set(&mut heading, "tracking", ParamValue::Float(120.0));
        let body = "Directed by\nJordan Lee\n\nWritten by\nPriya Natarajan\n\nEdited by\nMarco Bianchi\n\nMusic by\nHana Kobayashi\n\nMade with FilmCraft";
        let mut credits = text(2, "Credits", body, (960.0, 300.0), 46.0, "Regular");
        set(&mut credits, "align", ParamValue::Choice(1));
        set(&mut credits, "leading", ParamValue::Float(10.0));
        let mut t = builtin(
            "end-credits-roll",
            "End Credits – Roll",
            "End Credits",
            "Centred credits that roll up from below the frame and leave at the top.",
            12,
            vec![heading, credits],
            vec![
                ctl("heading", "Heading", 1, "text"),
                ctl("credits", "Credits", 2, "text"),
                ctl("font", "Font", 2, "font"),
                ctl("color", "Text Color", 2, "fill_color"),
            ],
        );
        t.graphic.roll = Roll { mode: RollMode::Roll, start_off_screen: true, end_off_screen: true, ..Default::default() };
        v.push(t);
    }
    // Callout: a label on a box with a pointer that follows it.
    {
        let mut label = text(1, "Label", "Look here", (1260.0, 380.0), 48.0, "SemiBold");
        set(&mut label, "fill_color", rgb(0xffffff));
        set(&mut label, "background", ParamValue::Bool(true));
        set(&mut label, "background_color", rgb(0xdc2626));
        set(&mut label, "background_opacity", ParamValue::Float(100.0));
        set(&mut label, "background_size", ParamValue::Float(18.0));
        set(&mut label, "background_radius", ParamValue::Float(6.0));
        let mut dot = shape(2, "Pointer", 1, (1200.0, 470.0), (34.0, 34.0), 0xdc2626);
        set(&mut dot, "stroke", ParamValue::Bool(true));
        set(&mut dot, "stroke_color", rgb(0xffffff));
        set(&mut dot, "stroke_width", ParamValue::Float(5.0));
        pin(&mut dot, 1, [true, true, false, false], [-60.0, 55.0, 0.0, 0.0]);
        v.push(builtin(
            "callout-pointer",
            "Callout – Pointer",
            "Callouts",
            "A boxed label with a round pointer; move the label and the pointer follows.",
            4,
            vec![dot, label],
            vec![
                ctl("label", "Label", 1, "text"),
                ctl("color", "Color", 1, "background_color"),
                ctl("pointer", "Pointer Color", 2, "fill_color"),
                ctl("position", "Position", 1, "position"),
            ],
        ));
    }
    // Callout: a pill-shaped tag around its text, with a status dot.
    {
        let mut tag = shape(1, "Tag", 0, (300.0, 140.0), (300.0, 70.0), 0x0f172a);
        set(&mut tag, "corner_radius", ParamValue::Float(35.0));
        set(&mut tag, "opacity", ParamValue::Float(88.0));
        pin(&mut tag, 2, [true; 4], [-70.0, -18.0, 30.0, 18.0]);
        let mut dot = shape(3, "Dot", 1, (190.0, 140.0), (22.0, 22.0), 0x22c55e);
        pin(&mut dot, 2, [true, false, false, false], [-44.0, 0.0, 0.0, 0.0]);
        let mut label = text(2, "Text", "LIVE", (220.0, 155.0), 40.0, "Bold");
        set(&mut label, "tracking", ParamValue::Float(80.0));
        v.push(builtin(
            "callout-tag",
            "Callout – Tag",
            "Callouts",
            "A rounded tag that fits its text, with a status dot.",
            4,
            vec![tag, dot, label],
            vec![
                ctl("text", "Text", 2, "text"),
                ctl("tag_color", "Tag Color", 1, "fill_color"),
                ctl("dot_color", "Dot Color", 3, "fill_color"),
                ctl("show_dot", "Show Dot", 3, "enabled"),
            ],
        ));
    }
    // Ticker: a crawl from right to left.
    {
        let mut line = text(
            1,
            "Ticker",
            "Breaking: the river festival moves to Saturday  •  Roads close at 6 pm  •  Shuttle buses run every 15 minutes",
            (40.0, 1010.0),
            44.0,
            "Medium",
        );
        set(&mut line, "background", ParamValue::Bool(true));
        set(&mut line, "background_color", rgb(0x111827));
        set(&mut line, "background_opacity", ParamValue::Float(85.0));
        set(&mut line, "background_size", ParamValue::Float(14.0));
        let mut t = builtin(
            "ticker-crawl",
            "Ticker – Crawl",
            "Lower Thirds",
            "A news-style line that crawls in from the right and leaves at the left.",
            15,
            vec![line],
            vec![ctl("text", "Text", 1, "text"), ctl("text_color", "Text Color", 1, "fill_color"), ctl("bar_color", "Bar Color", 1, "background_color")],
        );
        t.graphic.roll = Roll { mode: RollMode::CrawlLeft, start_off_screen: true, end_off_screen: true, ..Default::default() };
        v.push(t);
    }
    v.extend(social_and_short_video_templates());
    v
}

/// A text layer on its own rounded box (`size` = padding around the text).
fn on_box(e: &mut EffectInstance, color: u32, opacity: f64, size: f64, radius: f64) {
    set(e, "background", ParamValue::Bool(true));
    set(e, "background_color", rgb(color));
    set(e, "background_opacity", ParamValue::Float(opacity));
    set(e, "background_size", ParamValue::Float(size));
    set(e, "background_radius", ParamValue::Float(radius));
}

fn outlined(e: &mut EffectInstance, color: u32, width: f64) {
    set(e, "stroke", ParamValue::Bool(true));
    set(e, "stroke_color", rgb(color));
    set(e, "stroke_width", ParamValue::Float(width));
}

fn soft_shadow(e: &mut EffectInstance) {
    set(e, "shadow", ParamValue::Bool(true));
    set(e, "shadow_distance", ParamValue::Float(4.0));
    set(e, "shadow_blur", ParamValue::Float(18.0));
    set(e, "shadow_opacity", ParamValue::Float(55.0));
}

fn centred(e: &mut EffectInstance) {
    set(e, "align", ParamValue::Choice(1));
}

/// Templates for social and short video (original designs, Jilo Studio): calls to action, handles,
/// hooks, chapter and quote cards, lower thirds, banners, steps, labels and an outro.
fn social_and_short_video_templates() -> Vec<GraphicsTemplate> {
    let mut v = Vec::new();
    // Subscribe: a red pill with a white label, and a ringing-bell dot.
    {
        let mut label = text(1, "Label", "SUBSCRIBE", (960.0, 900.0), 58.0, "Bold");
        centred(&mut label);
        set(&mut label, "tracking", ParamValue::Float(40.0));
        on_box(&mut label, 0xe11d2e, 100.0, 26.0, 40.0);
        soft_shadow(&mut label);
        let mut bell = shape(2, "Bell", 1, (1180.0, 880.0), (34.0, 34.0), 0xfacc15);
        pin(&mut bell, 1, [false, true, true, false], [0.0, -24.0, 54.0, 0.0]);
        v.push(builtin(
            "social-subscribe",
            "Subscribe Button",
            "Social",
            "A bold call to subscribe on a red pill, with an accent dot.",
            4,
            vec![label, bell],
            vec![
                ctl("label", "Label", 1, "text"),
                ctl("pill", "Pill Color", 1, "background_color"),
                ctl("text_color", "Text Color", 1, "fill_color"),
                ctl("dot", "Dot Color", 2, "fill_color"),
                ctl("show_dot", "Show Dot", 2, "enabled"),
                ctl("position", "Position", 1, "position"),
            ],
        ));
    }
    // Like and follow: white pill, dark label.
    {
        let mut label = text(1, "Label", "LIKE  •  FOLLOW", (960.0, 920.0), 52.0, "Bold");
        centred(&mut label);
        set(&mut label, "fill_color", rgb(0x111111));
        on_box(&mut label, 0xffffff, 100.0, 24.0, 40.0);
        soft_shadow(&mut label);
        v.push(builtin(
            "social-like-follow",
            "Like and Follow",
            "Social",
            "A white pill asking to like and follow.",
            4,
            vec![label],
            vec![
                ctl("label", "Label", 1, "text"),
                ctl("pill", "Pill Color", 1, "background_color"),
                ctl("text_color", "Text Color", 1, "fill_color"),
                ctl("position", "Position", 1, "position"),
            ],
        ));
    }
    // Handle: @name on a translucent dark pill.
    {
        let mut handle = text(1, "Handle", "@yourname", (960.0, 980.0), 44.0, "SemiBold");
        centred(&mut handle);
        on_box(&mut handle, 0x000000, 60.0, 18.0, 30.0);
        v.push(builtin(
            "social-handle",
            "Handle",
            "Social",
            "Your @handle on a translucent pill, for a watermark or an end card.",
            6,
            vec![handle],
            vec![
                ctl("handle", "Handle", 1, "text"),
                ctl("pill", "Pill Color", 1, "background_color"),
                slider("pill_opacity", "Pill Opacity", 1, "background_opacity", 0.0, 100.0),
                ctl("position", "Position", 1, "position"),
            ],
        ));
    }
    // Link in the description, with an arrow.
    {
        let mut label = text(1, "Label", "Link in the description ↓", (960.0, 940.0), 50.0, "SemiBold");
        centred(&mut label);
        on_box(&mut label, 0x6e44ff, 100.0, 22.0, 18.0);
        soft_shadow(&mut label);
        v.push(builtin(
            "social-link",
            "Link in Description",
            "Social",
            "Points viewers to the link below the video.",
            4,
            vec![label],
            vec![ctl("label", "Label", 1, "text"), ctl("box", "Box Color", 1, "background_color"), ctl("position", "Position", 1, "position")],
        ));
    }
    // Hook: huge outlined words in the middle of the frame, for the first seconds.
    {
        let mut hook = text(1, "Hook", "WAIT FOR IT…", (960.0, 580.0), 150.0, "Bold");
        centred(&mut hook);
        outlined(&mut hook, 0x000000, 12.0);
        soft_shadow(&mut hook);
        v.push(builtin(
            "title-hook",
            "Hook – Big Words",
            "Titles",
            "Huge outlined words for the first seconds of a short.",
            3,
            vec![hook],
            vec![
                ctl("hook", "Text", 1, "text"),
                slider("size", "Size", 1, "size", 40.0, 260.0),
                ctl("text_color", "Text Color", 1, "fill_color"),
                ctl("outline", "Outline Color", 1, "stroke_color"),
                ctl("font", "Font", 1, "font"),
            ],
        ));
    }
    // Video title: a kicker line over a big uppercase headline with a yellow keyword line.
    {
        let mut kicker = text(1, "Kicker", "EPISODE 12", (960.0, 420.0), 40.0, "SemiBold");
        centred(&mut kicker);
        set(&mut kicker, "tracking", ParamValue::Float(200.0));
        set(&mut kicker, "fill_color", rgb(0xfacc15));
        let mut title = text(2, "Title", "HOW WE BUILT IT", (960.0, 560.0), 120.0, "Bold");
        centred(&mut title);
        outlined(&mut title, 0x000000, 8.0);
        soft_shadow(&mut title);
        v.push(builtin(
            "title-headline",
            "Title – Headline",
            "Titles",
            "A small kicker over a big outlined headline.",
            5,
            vec![kicker, title],
            vec![
                ctl("kicker", "Kicker", 1, "text"),
                ctl("title", "Title", 2, "text"),
                ctl("kicker_color", "Kicker Color", 1, "fill_color"),
                ctl("title_color", "Title Color", 2, "fill_color"),
                ctl("font", "Font", 2, "font"),
            ],
        ));
    }
    // Minimal title: light, widely spaced.
    {
        let mut title = text(1, "Title", "quiet mornings", (960.0, 560.0), 110.0, "Regular");
        centred(&mut title);
        set(&mut title, "tracking", ParamValue::Float(160.0));
        soft_shadow(&mut title);
        v.push(builtin(
            "title-minimal",
            "Title – Minimal",
            "Titles",
            "A light, widely spaced title for calm footage.",
            5,
            vec![title],
            vec![
                ctl("title", "Title", 1, "text"),
                slider("tracking", "Letter Spacing", 1, "tracking", 0.0, 400.0),
                ctl("text_color", "Text Color", 1, "fill_color"),
                ctl("font", "Font", 1, "font"),
            ],
        ));
    }
    // Chapter card: a number, a title and an accent bar on the left.
    {
        let mut bar = shape(1, "Bar", 0, (150.0, 540.0), (12.0, 220.0), 0x6e44ff);
        pin(&mut bar, 3, [true, false, false, false], [-36.0, 0.0, 0.0, 0.0]);
        let mut num = text(2, "Number", "CHAPTER 2", (190.0, 480.0), 40.0, "SemiBold");
        set(&mut num, "tracking", ParamValue::Float(150.0));
        set(&mut num, "fill_color", rgb(0xc4b5fd));
        let mut title = text(3, "Title", "The first prototype", (186.0, 580.0), 96.0, "Bold");
        soft_shadow(&mut title);
        v.push(builtin(
            "chapter-card",
            "Chapter Card",
            "Titles",
            "Chapter number and title beside an accent bar.",
            5,
            vec![bar, num, title],
            vec![
                ctl("number", "Number", 2, "text"),
                ctl("title", "Title", 3, "text"),
                ctl("accent", "Accent Color", 1, "fill_color"),
                ctl("font", "Font", 3, "font"),
            ],
        ));
    }
    // Quote card: a big quotation mark, the quote and who said it.
    {
        let mut mark = text(1, "Mark", "“", (960.0, 400.0), 220.0, "Bold");
        centred(&mut mark);
        set(&mut mark, "fill_color", rgb(0xfacc15));
        let mut quote = text(2, "Quote", "Make it simple,\nbut significant.", (960.0, 540.0), 72.0, "Italic");
        centred(&mut quote);
        soft_shadow(&mut quote);
        let mut who = text(3, "Author", "— Someone wise", (960.0, 720.0), 40.0, "Medium");
        centred(&mut who);
        set(&mut who, "fill_color", rgb(0xd1d5db));
        v.push(builtin(
            "quote-card",
            "Quote Card",
            "Titles",
            "A quotation with a large mark and its author.",
            6,
            vec![mark, quote, who],
            vec![
                ctl("quote", "Quote", 2, "text"),
                ctl("author", "Author", 3, "text"),
                ctl("mark_color", "Mark Color", 1, "fill_color"),
                ctl("text_color", "Text Color", 2, "fill_color"),
            ],
        ));
    }
    // Lower third: name and role on two pills.
    {
        let mut name = text(1, "Name", "Dilnoza Karimova", (140.0, 900.0), 56.0, "Bold");
        set(&mut name, "fill_color", rgb(0x111111));
        on_box(&mut name, 0xffffff, 100.0, 16.0, 28.0);
        let mut role = text(2, "Role", "Product Designer", (140.0, 980.0), 36.0, "Medium");
        on_box(&mut role, 0x6e44ff, 100.0, 12.0, 22.0);
        v.push(builtin(
            "lower-third-pills",
            "Lower Third – Pills",
            "Lower Thirds",
            "Name and role on two rounded pills.",
            6,
            vec![name, role],
            vec![
                ctl("name", "Name", 1, "text"),
                ctl("role", "Role", 2, "text"),
                ctl("name_pill", "Name Pill", 1, "background_color"),
                ctl("role_pill", "Role Pill", 2, "background_color"),
                ctl("show_role", "Show Role", 2, "enabled"),
            ],
        ));
    }
    // Lower third: name | role separated by a thin bar.
    {
        let mut name = text(1, "Name", "Timur Rahimov", (140.0, 960.0), 54.0, "Bold");
        soft_shadow(&mut name);
        let mut sep = shape(2, "Separator", 0, (560.0, 943.0), (4.0, 60.0), 0xffffff);
        pin(&mut sep, 1, [false, false, true, false], [0.0, 0.0, 30.0, 0.0]);
        let mut role = text(3, "Role", "Founder", (590.0, 960.0), 44.0, "Regular");
        pin(&mut role, 2, [true, false, false, false], [30.0, 0.0, 0.0, 0.0]);
        soft_shadow(&mut role);
        v.push(builtin(
            "lower-third-minimal",
            "Lower Third – Minimal",
            "Lower Thirds",
            "Name and role on one line with a thin separator.",
            6,
            vec![name, sep, role],
            vec![ctl("name", "Name", 1, "text"), ctl("role", "Role", 3, "text"), ctl("separator", "Separator Color", 2, "fill_color")],
        ));
    }
    // Breaking news: a red label beside a white bar with the headline.
    {
        let mut label = text(1, "Label", "BREAKING", (80.0, 960.0), 44.0, "Bold");
        on_box(&mut label, 0xe11d2e, 100.0, 16.0, 0.0);
        // beside the label (pins align like edges, so the bar is placed, not pinned)
        let mut headline = text(2, "Headline", "City opens a new bridge across the river", (370.0, 960.0), 44.0, "SemiBold");
        set(&mut headline, "fill_color", rgb(0x111111));
        on_box(&mut headline, 0xffffff, 100.0, 16.0, 0.0);
        v.push(builtin(
            "lower-third-breaking",
            "Breaking News",
            "Lower Thirds",
            "A red label and a white headline bar, news style.",
            8,
            vec![label, headline],
            vec![
                ctl("label", "Label", 1, "text"),
                ctl("headline", "Headline", 2, "text"),
                ctl("label_color", "Label Color", 1, "background_color"),
                ctl("bar_color", "Bar Color", 2, "background_color"),
            ],
        ));
    }
    // Top banner: a full-width bar with centred text.
    {
        let mut bar = shape(1, "Bar", 0, (960.0, 70.0), (1920.0, 140.0), 0x111827);
        set(&mut bar, "opacity", ParamValue::Float(85.0));
        let mut label = text(2, "Text", "PART 1 OF 3", (960.0, 92.0), 56.0, "Bold");
        centred(&mut label);
        set(&mut label, "tracking", ParamValue::Float(80.0));
        v.push(builtin(
            "banner-top",
            "Top Banner",
            "Banners",
            "A full-width bar at the top with centred text — series parts, topics, warnings.",
            5,
            vec![bar, label],
            vec![ctl("text", "Text", 2, "text"), ctl("bar_color", "Bar Color", 1, "fill_color"), ctl("text_color", "Text Color", 2, "fill_color")],
        ));
    }
    // Step: a number in a circle and the step's text.
    {
        let mut circle = shape(1, "Circle", 1, (190.0, 180.0), (96.0, 96.0), 0x22c55e);
        let mut num = text(2, "Number", "1", (190.0, 200.0), 60.0, "Bold");
        centred(&mut num);
        let mut step = text(3, "Step", "Open the settings", (270.0, 200.0), 52.0, "SemiBold");
        on_box(&mut step, 0x000000, 65.0, 16.0, 14.0);
        v.push(builtin(
            "label-step",
            "Step Label",
            "Banners",
            "A numbered step for tutorials and how-tos.",
            5,
            vec![circle, num, step],
            vec![
                ctl("number", "Number", 2, "text"),
                ctl("step", "Step", 3, "text"),
                ctl("circle", "Circle Color", 1, "fill_color"),
                ctl("position", "Position", 3, "position"),
            ],
        ));
    }
    // Place and date: a small two-line label.
    {
        let mut place = text(1, "Place", "DUSHANBE, TAJIKISTAN", (120.0, 140.0), 40.0, "Bold");
        set(&mut place, "tracking", ParamValue::Float(120.0));
        soft_shadow(&mut place);
        let mut date = text(2, "Date", "14 October 2026", (120.0, 196.0), 34.0, "Regular");
        set(&mut date, "fill_color", rgb(0xe5e7eb));
        soft_shadow(&mut date);
        v.push(builtin(
            "label-place-date",
            "Place and Date",
            "Banners",
            "Where and when, as a small two-line label.",
            5,
            vec![place, date],
            vec![ctl("place", "Place", 1, "text"), ctl("date", "Date", 2, "text"), ctl("show_date", "Show Date", 2, "enabled")],
        ));
    }
    // Outro: thanks and a call to subscribe.
    {
        let mut thanks = text(1, "Thanks", "Thanks for watching!", (960.0, 500.0), 100.0, "Bold");
        centred(&mut thanks);
        soft_shadow(&mut thanks);
        let mut more = text(2, "Call", "Subscribe for more", (960.0, 640.0), 52.0, "SemiBold");
        centred(&mut more);
        on_box(&mut more, 0xe11d2e, 100.0, 20.0, 34.0);
        v.push(builtin(
            "outro-thanks",
            "Outro – Thanks",
            "End Credits",
            "Thanks for watching, with a call to subscribe.",
            6,
            vec![thanks, more],
            vec![
                ctl("thanks", "Thanks", 1, "text"),
                ctl("call", "Call to Action", 2, "text"),
                ctl("call_color", "Call Color", 2, "background_color"),
                ctl("font", "Font", 1, "font"),
            ],
        ));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for n in 0..20 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("@@@").is_none());
    }

    #[test]
    fn builtins_are_valid_and_round_trip() {
        let all = builtin_templates();
        assert!(all.len() >= 22, "{}", all.len());
        let mut ids: Vec<&str> = all.iter().map(|t| t.id.as_str()).collect();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for cat in ["Lower Thirds", "Titles", "End Credits", "Callouts", "Social", "Banners"] {
            assert!(all.iter().any(|t| t.category == cat), "{cat}");
        }
        for t in all {
            t.validate().unwrap_or_else(|e| panic!("{}: {e}", t.name));
            let back = GraphicsTemplate::from_bytes(t.to_json().as_bytes()).unwrap();
            assert_eq!(back, t);
        }
    }

    #[test]
    fn refuses_foreign_and_newer_files() {
        assert_eq!(GraphicsTemplate::from_bytes(b"PK\x03\x04rest"), Err(TemplateError::ForeignPackage));
        assert!(matches!(GraphicsTemplate::from_bytes(b"{\"format\":\"x\"}"), Err(TemplateError::NotTemplate(_))));
        let mut t = builtin_templates().remove(0);
        t.version = 99;
        assert_eq!(GraphicsTemplate::from_bytes(t.to_json().as_bytes()), Err(TemplateError::TooNew(99)));
        let mut t = builtin_templates().remove(0);
        t.controls[0].layer = 999;
        assert!(matches!(t.validate(), Err(TemplateError::Invalid(_))));
    }

    #[test]
    fn instantiates_on_other_frame_sizes() {
        let t = builtin_templates().into_iter().find(|t| t.id == "builtin:title-centered").unwrap();
        let (same, meta) = t.instantiate((1920, 1080));
        assert_eq!(same, t.layers);
        assert_eq!(meta.template.as_ref().unwrap().id, t.id);
        let (half, _) = t.instantiate((960, 540));
        assert_eq!(half[0].params["position"].value, ParamValue::Vec2(Vec2::new(480.0, 260.0)));
        assert_eq!(half[0].params["scale"].value, ParamValue::Float(50.0));
        // a vertical frame: fit to width, centred vertically
        let (tall, _) = t.instantiate((1080, 1920));
        let ParamValue::Vec2(p) = tall[0].params["position"].value else { panic!() };
        assert!((p.x - 540.0).abs() < 1e-9 && (p.y - (520.0 * 0.5625 + (1920.0 - 1080.0 * 0.5625) / 2.0)).abs() < 1e-9);
        assert_eq!(slug("My Lower Third!"), "my-lower-third");
        assert_eq!(ControlKind::for_param("background_opacity"), ControlKind::Slider);
        assert_eq!(ControlKind::for_param("shadow"), ControlKind::Checkbox);
    }
}
