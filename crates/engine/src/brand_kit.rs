//! Brand kits: a logo, colours, a font, a caption style, a lower third and an intro and outro kept
//! together, and put on the open sequence in one go (one undo step).
//!
//! | Command | |
//! |---|---|
//! | `brandKit.list` | the saved kits |
//! | `brandKit.get` | `{name}`: one kit |
//! | `brandKit.save` | `{kit}`: save (replace) a kit by its name |
//! | `brandKit.delete` | `{name}` |
//! | `brandKit.apply` | `{name \| kit, parts?}`: the kit on the open sequence — `captions` (the style preset and the font on every caption track), `graphics` (colours and font of template graphics such as lower thirds), `intro` (inserted at the start), `outro` (after the end), `logo` (a corner of the frame for the whole sequence, on a track named "Logo") |
//! | `brandKit.addLowerThird` | `{name \| kit, text?, role?}`: the kit's lower third at the playhead, in its colours and font |
//!
//! Kits are JSON files in `<data dir>/brand-kits/`. Applying again replaces the logo and does not
//! add the intro or outro twice.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::{ClipId, ItemId, TrackId};
use filmcraft_time::Tick;

use crate::commands::{CommandSpec, always, bad, has_seq, str_p};
use crate::{EngineError, Result, Session};

/// The parts of a kit `brandKit.apply` puts on the sequence, in the order it does them (the logo
/// last, so it spans the intro and outro).
pub const PARTS: [&str; 5] = ["captions", "graphics", "intro", "outro", "logo"];

/// Name of the track that holds a kit's logo.
pub const LOGO_TRACK: &str = "Logo";

/// A brand kit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrandKit {
    pub name: String,
    /// Logo image (a PNG with transparency is best).
    pub logo: Option<String>,
    /// `topLeft`, `topRight`, `bottomLeft` or `bottomRight`.
    pub logo_corner: String,
    /// Logo width, in percent of the frame width.
    pub logo_size: f64,
    /// Logo opacity, in percent.
    pub logo_opacity: f64,
    /// `#rrggbb`: main, second and accent colour.
    pub colors: Vec<String>,
    /// Font family for captions and titles.
    pub font: Option<String>,
    /// Caption style preset id (`captions.stylePresets`).
    pub caption_preset: Option<String>,
    /// Graphics template id for lower thirds (`graphics.template.list`).
    pub lower_third: Option<String>,
    /// Intro and outro clips (video files).
    pub intro: Option<String>,
    pub outro: Option<String>,
}

impl Default for BrandKit {
    fn default() -> Self {
        Self {
            name: String::new(),
            logo: None,
            logo_corner: "topRight".into(),
            logo_size: 12.0,
            logo_opacity: 90.0,
            colors: Vec::new(),
            font: None,
            caption_preset: None,
            lower_third: None,
            intro: None,
            outro: None,
        }
    }
}

impl BrandKit {
    fn color(&self, i: usize) -> Option<&str> {
        self.colors.get(i).map(String::as_str).filter(|c| c.starts_with('#'))
    }
    /// What `apply` would put on: the parts the kit has something for.
    pub fn parts(&self) -> Vec<&'static str> {
        PARTS
            .into_iter()
            .filter(|p| match *p {
                "captions" => self.caption_preset.is_some() || self.font.is_some(),
                "graphics" => !self.colors.is_empty() || self.font.is_some(),
                "intro" => self.intro.is_some(),
                "outro" => self.outro.is_some(),
                "logo" => self.logo.is_some(),
                _ => false,
            })
            .collect()
    }
}

/// Where the kits are kept.
pub fn dir() -> Option<PathBuf> {
    crate::autosave::default_data_dir().map(|d| d.join("brand-kits"))
}

fn file_name(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() { c.to_lowercase().next().unwrap_or(c) } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    format!("{}.json", if s.is_empty() { "brand-kit".to_string() } else { s })
}

/// The kits in `d`, by name.
pub fn list_in(d: &Path) -> Vec<BrandKit> {
    let mut v: Vec<BrandKit> = std::fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                .filter_map(|e| std::fs::read(e.path()).ok())
                .filter_map(|b| serde_json::from_slice::<BrandKit>(&b).ok())
                .filter(|k| !k.name.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|k| k.name.to_lowercase());
    v
}

pub fn save_in(d: &Path, k: &BrandKit) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(d)?;
    let path = d.join(file_name(&k.name));
    let bytes = serde_json::to_vec_pretty(k).map_err(std::io::Error::other)?;
    // written whole then renamed, so a kit is never left half written
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn find(name: &str) -> Option<BrandKit> {
    dir().and_then(|d| list_in(&d).into_iter().find(|k| k.name.eq_ignore_ascii_case(name.trim())))
}

/// The kit a command names (`name`) or carries (`kit`).
fn kit_param(p: &Value, cmd: &str) -> Result<BrandKit> {
    if let Some(k) = p.get("kit").filter(|k| k.is_object()) {
        return serde_json::from_value(k.clone()).map_err(|e| bad(cmd, format!("`kit`: {e}")));
    }
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "need `name` or `kit`"))?;
    find(name).ok_or_else(|| bad(cmd, format!("no brand kit `{name}`")))
}

fn list(_: &mut Session, _: &Value) -> Result<Value> {
    let kits = dir().map(|d| list_in(&d)).unwrap_or_default();
    Ok(json!(kits.iter().map(|k| json!({"name": k.name, "parts": k.parts()})).collect::<Vec<_>>()))
}

fn get(_: &mut Session, p: &Value) -> Result<Value> {
    let k = kit_param(p, "brandKit.get")?;
    serde_json::to_value(&k).map_err(|e| EngineError::Other(e.to_string()))
}

fn save(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brandKit.save";
    let k: BrandKit = serde_json::from_value(p.get("kit").cloned().unwrap_or(Value::Null)).map_err(|e| bad(cmd, format!("`kit`: {e}")))?;
    if k.name.trim().is_empty() {
        return Err(bad(cmd, "a brand kit needs a name"));
    }
    if let Some(c) = k.colors.iter().find(|c| !valid_hex(c)) {
        return Err(bad(cmd, format!("`{c}` is not a colour: #rrggbb")));
    }
    let d = dir().ok_or_else(|| bad(cmd, "no data directory for brand kits"))?;
    let path = save_in(&d, &k).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"name": k.name, "path": path.to_string_lossy()}))
}

fn delete(_: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brandKit.delete";
    let name = str_p(p, "name").ok_or_else(|| bad(cmd, "need `name`"))?;
    let d = dir().ok_or_else(|| bad(cmd, "no data directory for brand kits"))?;
    let k = list_in(&d).into_iter().find(|k| k.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| bad(cmd, format!("no brand kit `{name}`")))?;
    std::fs::remove_file(d.join(file_name(&k.name))).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"deleted": k.name}))
}

fn valid_hex(c: &str) -> bool {
    let h = c.trim_start_matches('#');
    c.starts_with('#') && (h.len() == 6 || h.len() == 8) && h.chars().all(|x| x.is_ascii_hexdigit())
}

/// The project item for a file: imported, or the one already in the project.
fn item_for(s: &mut Session, path: &str) -> Result<ItemId> {
    let r = s.execute("file.import", json!({"paths": [path]}))?;
    r["items"][0]
        .as_u64()
        .or_else(|| r["duplicates"][0]["item"].as_u64())
        .map(ItemId)
        .ok_or_else(|| EngineError::Other(format!("{path}: could not be imported")))
}

fn seq_end(s: &Session) -> Tick {
    s.active_sequence().map(|q| q.all_tracks().flat_map(|t| t.items.iter()).map(|i| i.end()).max().unwrap_or(Tick::ZERO)).unwrap_or(Tick::ZERO)
}

fn v1_items(s: &Session) -> Vec<(ItemId, Tick, Tick)> {
    s.active_sequence().and_then(|q| q.video_tracks.first()).map(|t| t.items.iter().map(|i| (i.item, i.start, i.end())).collect()).unwrap_or_default()
}

/// Colour and font values for a template graphic's controls: colour controls of text keep their
/// colour, accent controls take the accent, the rest (slabs, bars, boxes) the main colour.
fn graphic_values(k: &BrandKit, controls: &[Value]) -> serde_json::Map<String, Value> {
    let mut vals = serde_json::Map::new();
    for c in controls {
        let (Some(id), Some(kind)) = (c["id"].as_str(), c["kind"].as_str()) else { continue };
        let low = id.to_ascii_lowercase();
        match kind {
            "color" if ["text", "name", "role", "title", "subtitle"].iter().any(|w| low.contains(w)) => {}
            "color" => {
                let v = if low.contains("accent") || low.contains("rule") || low.contains("line") { k.color(2).or(k.color(0)) } else { k.color(0) };
                if let Some(v) = v {
                    vals.insert(id.to_string(), json!(v));
                }
            }
            "font" => {
                if let Some(f) = &k.font {
                    vals.insert(id.to_string(), json!(f));
                }
            }
            _ => {}
        }
    }
    vals
}

/// Restyle one template graphic; returns whether anything was set.
fn style_graphic(s: &mut Session, k: &BrandKit, clip: ClipId) -> Result<bool> {
    let c = s.execute("graphics.template.controls", json!({"clip": clip.0}))?;
    let vals = graphic_values(k, c["controls"].as_array().map(Vec::as_slice).unwrap_or(&[]));
    if vals.is_empty() {
        return Ok(false);
    }
    s.execute("graphics.template.set", json!({"clip": clip.0, "values": vals}))?;
    Ok(true)
}

fn logo_track(s: &mut Session) -> Result<TrackId> {
    let found = s
        .active_sequence()
        .and_then(|q| q.video_tracks.iter().find(|t| t.name == LOGO_TRACK).map(|t| (t.id, t.items.iter().map(|i| i.id).collect::<Vec<_>>())));
    if let Some((id, old)) = found {
        // the kit's previous logo goes: the new one takes its place
        if !old.is_empty() {
            s.edit_sequence("Remove Logo", move |q, _, _| {
                if let Some(t) = q.track_mut(id) {
                    t.items.retain(|i| !old.contains(&i.id));
                }
                Ok(())
            })?;
        }
        return Ok(id);
    }
    let added = s.execute("sequence.addTracks", json!({"video": 1}))?;
    let id = added["video"][0].as_u64().map(TrackId).ok_or_else(|| EngineError::Other("no track was added for the logo".into()))?;
    s.execute("timeline.setTrack", json!({"track": id.0, "name": LOGO_TRACK}))?;
    Ok(id)
}

fn place_logo(s: &mut Session, k: &BrandKit, path: &str) -> Result<ClipId> {
    let item = item_for(s, path)?;
    let end = seq_end(s);
    if end <= Tick::ZERO {
        return Err(EngineError::Other("the sequence is empty: there is nothing to put the logo on".into()));
    }
    let track = logo_track(s)?;
    let r = s.execute("timeline.place", json!({"item": item.0, "track": track.0, "time": 0, "duration": end.0, "audio": false}))?;
    let clip = r["clips"][0].as_u64().map(ClipId).ok_or_else(|| EngineError::Other("the logo could not be placed".into()))?;
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (fw, fh) = (f64::from(q.settings.width), f64::from(q.settings.height));
    let (lw, lh) =
        s.source(item).and_then(|src| src.info().video.as_ref().map(|v| (f64::from(v.width.max(1)), f64::from(v.height.max(1))))).unwrap_or((fw, fh));
    let size = k.logo_size.clamp(2.0, 60.0);
    let scale = size / 100.0 * fw / lw * 100.0;
    let (w, h) = (lw * scale / 100.0, lh * scale / 100.0);
    let m = fw.min(fh) * 0.045;
    let x = if k.logo_corner.ends_with("Left") { m + w / 2.0 } else { fw - m - w / 2.0 };
    let y = if k.logo_corner.starts_with("bottom") { fh - m - h / 2.0 } else { m + h / 2.0 };
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "scale", "value": scale}))?;
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "position", "value": [x, y]}))?;
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": "opacity", "param": "opacity", "value": k.logo_opacity.clamp(0.0, 100.0)}))?;
    Ok(clip)
}

/// Put `parts` of `k` on the open sequence. Every step is a command; the caller folds them into
/// one undo step.
fn apply_parts(s: &mut Session, k: &BrandKit, parts: &[&str]) -> Result<Value> {
    let mut out = json!({});
    for part in PARTS.into_iter().filter(|p| parts.contains(p)) {
        match part {
            "captions" => {
                let tracks = s.active_sequence().map_or(0, |q| q.caption_tracks.len());
                if tracks == 0 {
                    out["captions"] = json!("no captions");
                    continue;
                }
                if let Some(p) = &k.caption_preset {
                    s.execute("captions.applyStylePreset", json!({"preset": p, "track": "all"}))?;
                }
                if let Some(f) = &k.font {
                    s.execute("captions.setStyle", json!({"track": "all", "font": f}))?;
                }
                out["captions"] = json!(tracks);
            }
            "graphics" => {
                let clips: Vec<ClipId> = s
                    .active_sequence()
                    .map(|q| {
                        q.video_tracks
                            .iter()
                            .flat_map(|t| t.items.iter())
                            .filter(|i| i.graphic.as_ref().is_some_and(|g| g.template.is_some()))
                            .map(|i| i.id)
                            .collect()
                    })
                    .unwrap_or_default();
                let mut n = 0;
                for c in clips {
                    if style_graphic(s, k, c)? {
                        n += 1;
                    }
                }
                out["graphics"] = json!(n);
            }
            "intro" => {
                let Some(path) = &k.intro else { continue };
                let item = item_for(s, path)?;
                if v1_items(s).iter().any(|(i, start, _)| *i == item && *start == Tick::ZERO) {
                    out["intro"] = json!("already there");
                    continue;
                }
                s.execute("timeline.place", json!({"item": item.0, "track": "V1", "audioTrack": "A1", "time": 0, "insert": true}))?;
                out["intro"] = json!(item.0);
            }
            "outro" => {
                let Some(path) = &k.outro else { continue };
                let item = item_for(s, path)?;
                let end = seq_end(s);
                if v1_items(s).iter().any(|(i, _, e)| *i == item && *e == end) {
                    out["outro"] = json!("already there");
                    continue;
                }
                s.execute("timeline.place", json!({"item": item.0, "track": "V1", "audioTrack": "A1", "time": end.0, "insert": false}))?;
                out["outro"] = json!(item.0);
            }
            "logo" => {
                let Some(path) = &k.logo else { continue };
                out["logo"] = json!(place_logo(s, k, path)?.0);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Run `f` as one undo step named `label`: every edit it makes is taken back when it fails.
pub(crate) fn as_one_step(s: &mut Session, label: &str, f: impl FnOnce(&mut Session) -> Result<Value>) -> Result<Value> {
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let selection = s.state.selection.clone();
    let r = f(s);
    s.state.selection = selection;
    match r {
        Ok(v) => {
            crate::recipes::collapse_undo(s, before, label);
            Ok(v)
        }
        Err(e) => {
            s.revert_to(mark);
            Err(e)
        }
    }
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "brandKit.apply";
    let k = kit_param(p, cmd)?;
    let parts: Vec<String> = match p.get("parts").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        None => k.parts().into_iter().map(str::to_string).collect(),
    };
    if let Some(x) = parts.iter().find(|x| !PARTS.contains(&x.as_str())) {
        return Err(bad(cmd, format!("unknown part `{x}`: {}", PARTS.join(", "))));
    }
    if parts.is_empty() {
        return Err(bad(cmd, "the brand kit has nothing to apply"));
    }
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let label = format!("Apply Brand Kit: {}", k.name);
    let mut r = as_one_step(s, &label, |s| apply_parts(s, &k, &parts))?;
    r["kit"] = json!(k.name);
    Ok(r)
}

fn add_lower_third(s: &mut Session, p: &Value) -> Result<Value> {
    let k = kit_param(p, "brandKit.addLowerThird")?;
    let template = k.lower_third.clone().unwrap_or_else(|| "builtin:lower-third-slab".into());
    let mut values = serde_json::Map::new();
    for (key, id) in [("text", "name"), ("role", "role")] {
        if let Some(v) = str_p(p, key) {
            values.insert(id.to_string(), json!(v));
        }
    }
    let label = format!("Lower Third: {}", k.name);
    as_one_step(s, &label, |s| {
        let mut q = json!({"template": template, "time": s.playhead().0});
        if !values.is_empty() {
            q["values"] = Value::Object(values);
        }
        let r = s.execute("graphics.template.apply", q)?;
        let clip = r["clip"].as_u64().map(ClipId).ok_or_else(|| EngineError::Other("the lower third was not placed".into()))?;
        style_graphic(s, &k, clip)?;
        Ok(json!({"clip": clip.0, "template": template}))
    })
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec { id: "brandKit.list", label: "List Brand Kits", menu: &[], shortcut: None, params: "{}", enabled: always, run: list, journal: false },
        CommandSpec {
            id: "brandKit.get",
            label: "Get Brand Kit",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str}"#,
            enabled: always,
            run: get,
            journal: false,
        },
        CommandSpec {
            id: "brandKit.save",
            label: "Save Brand Kit",
            menu: &[],
            shortcut: None,
            params: r##"{"kit":{"name":str,"logo":path?,"logoCorner":"topLeft|topRight|bottomLeft|bottomRight"="topRight","logoSize":%=12,"logoOpacity":%=90,"colors":["#rrggbb"]?,"font":family?,"captionPreset":id?,"lowerThird":templateId?,"intro":path?,"outro":path?}}"##,
            enabled: always,
            run: save,
            journal: false,
        },
        CommandSpec {
            id: "brandKit.delete",
            label: "Delete Brand Kit",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str}"#,
            enabled: always,
            run: delete,
            journal: false,
        },
        CommandSpec {
            id: "brandKit.apply",
            label: "Apply Brand Kit",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str|"kit":{…},"parts":["captions"|"graphics"|"intro"|"outro"|"logo"]? (default: all the kit has)}"#,
            enabled: has_seq,
            run: apply,
            journal: true,
        },
        CommandSpec {
            id: "brandKit.addLowerThird",
            label: "Add Lower Third",
            menu: &[],
            shortcut: None,
            params: r#"{"name":str|"kit":{…},"text":str?,"role":str?}"#,
            enabled: has_seq,
            run: add_lower_third,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jilo-brand-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn kits_are_saved_listed_and_read_back() {
        let d = temp("store");
        let k = BrandKit { name: "Acme Studio".into(), colors: vec!["#112233".into()], caption_preset: Some("karaoke-yellow".into()), ..Default::default() };
        let path = save_in(&d, &k).unwrap();
        assert!(path.ends_with("acme-studio.json"));
        assert_eq!(list_in(&d), vec![k.clone()]);
        assert_eq!(k.parts(), ["captions", "graphics"]);
        // a broken file is left out, not an error
        std::fs::write(d.join("broken.json"), b"{not json").unwrap();
        assert_eq!(list_in(&d).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn template_colours_follow_the_kit_and_text_keeps_its_own() {
        let k = BrandKit { colors: vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()], font: Some("Montserrat".into()), ..Default::default() };
        let controls: Vec<Value> = [("slab_color", "color"), ("accent_color", "color"), ("text_color", "color"), ("font", "font"), ("name", "text")]
            .iter()
            .map(|(id, kind)| json!({"id": id, "kind": kind}))
            .collect();
        let v = graphic_values(&k, &controls);
        assert_eq!(v.get("slab_color"), Some(&json!("#ff0000")));
        assert_eq!(v.get("accent_color"), Some(&json!("#0000ff")));
        assert_eq!(v.get("font"), Some(&json!("Montserrat")));
        assert!(!v.contains_key("text_color") && !v.contains_key("name"));
    }

    #[test]
    fn applying_a_kit_is_one_undo_step_and_twice_adds_nothing_more() {
        let d = temp("apply");
        let logo = d.join("logo.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([255, 255, 255, 200])).save(&logo).unwrap();
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        s.execute("captions.add", json!({"seconds": 1.0, "durationSeconds": 2.0, "text": "Hello"})).unwrap();
        s.execute("graphics.template.apply", json!({"template": "builtin:lower-third-slab", "time": 0})).unwrap();
        let undo = s.history.undo.len();
        let kit = json!({
            "name": "Acme", "logo": logo.to_string_lossy(), "logoCorner": "bottomLeft", "logoSize": 10, "logoOpacity": 80,
            "colors": ["#ff0000", "#00ff00", "#0000ff"], "captionPreset": "karaoke-yellow", "font": "Helvetica",
        });
        let r = s.execute("brandKit.apply", json!({"kit": kit})).unwrap();
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step: {r}");
        assert_eq!(s.history.undo.last().map(|u| u.0.as_str()), Some("Apply Brand Kit: Acme"));
        assert_eq!(r["captions"], 1);
        assert_eq!(r["graphics"], 1);
        // the logo: a 400 px image at 10 % of a 1920 px frame, bottom left, 80 % opaque
        let q = s.active_sequence().unwrap();
        let track = q.video_tracks.iter().find(|t| t.name == LOGO_TRACK).expect("logo track");
        assert_eq!(track.items.len(), 1);
        let it = &track.items[0];
        assert_eq!((it.start, it.end()), (Tick::ZERO, seq_end(&s)));
        let t = it.effect_time_at(it.start);
        let m = it.effect("motion").unwrap();
        assert!((m.f64_at("scale", t) - 48.0).abs() < 0.01, "scale {}", m.f64_at("scale", t));
        let pos = m.vec2_at("position", t);
        assert!(pos.x < 300.0 && pos.y > 900.0, "bottom left: {pos:?}");
        assert!((it.effect("opacity").unwrap().f64_at("opacity", t) - 80.0).abs() < 0.01);
        let cap = &q.caption_tracks[0].style;
        assert_eq!(cap.font, "Helvetica");
        // again: the logo is replaced, not added
        s.execute("brandKit.apply", json!({"kit": kit})).unwrap();
        let q = s.active_sequence().unwrap();
        assert_eq!(q.video_tracks.iter().filter(|t| t.name == LOGO_TRACK).count(), 1);
        assert_eq!(q.video_tracks.iter().find(|t| t.name == LOGO_TRACK).unwrap().items.len(), 1);
        // undo takes the whole kit back
        s.execute("edit.undo", json!({})).unwrap();
        s.execute("edit.undo", json!({})).unwrap();
        assert!(s.active_sequence().unwrap().video_tracks.iter().all(|t| t.name != LOGO_TRACK));
        // a kit with a missing logo fails and leaves nothing behind
        let n = s.history.undo.len();
        let before = s.project.to_json();
        assert!(s.execute("brandKit.apply", json!({"kit": {"name": "x", "logo": "/no/such/logo.png", "font": "Arial"}})).is_err());
        assert_eq!(s.history.undo.len(), n);
        assert_eq!(s.project.to_json(), before);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn intro_and_outro_go_on_once() {
        let d = temp("io");
        let (intro, outro) = (d.join("intro.png"), d.join("outro.png"));
        image::RgbaImage::from_pixel(64, 36, image::Rgba([10, 10, 10, 255])).save(&intro).unwrap();
        image::RgbaImage::from_pixel(64, 36, image::Rgba([200, 10, 10, 255])).save(&outro).unwrap();
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let end = seq_end(&s);
        let kit = json!({"name": "IO", "intro": intro.to_string_lossy(), "outro": outro.to_string_lossy()});
        s.execute("brandKit.apply", json!({"kit": kit})).unwrap();
        let v1 = v1_items(&s);
        let first = v1.iter().min_by_key(|x| x.1).unwrap();
        let last = v1.iter().max_by_key(|x| x.2).unwrap();
        assert_eq!(first.1, Tick::ZERO);
        assert!(seq_end(&s) > end, "the sequence is longer by the intro and outro");
        assert_eq!(last.2, seq_end(&s));
        let n = v1.len();
        let r = s.execute("brandKit.apply", json!({"kit": kit})).unwrap();
        assert_eq!((r["intro"].as_str(), r["outro"].as_str()), (Some("already there"), Some("already there")));
        assert_eq!(v1_items(&s).len(), n);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_lower_third_in_the_kit_colours() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        let kit = json!({"name": "LT", "colors": ["#123456"], "lowerThird": "builtin:lower-third-slab"});
        let r = s.execute("brandKit.addLowerThird", json!({"kit": kit, "text": "Rustam", "role": "Director"})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        let c = s.execute("graphics.template.controls", json!({"clip": clip})).unwrap();
        let val = |id: &str| c["controls"].as_array().unwrap().iter().find(|x| x["id"] == id).map(|x| x["value"].clone()).unwrap();
        assert_eq!(val("name"), "Rustam");
        assert_eq!(val("role"), "Director");
        assert_eq!(val("slab_color").as_str().map(str::to_ascii_lowercase).as_deref(), Some("#123456"));
    }
}
