//! Stickers: emoji and our own shapes (arrows, a speech bubble, a subscribe button, a ring, a
//! badge, an underline) put on the picture in one click, with an entrance animation.
//!
//! Emoji are drawn on this computer from the system's colour emoji font (the host installs an
//! [`EmojiRenderer`]), saved once as PNG in `<data dir>/stickers/emoji/` (beside the preferences;
//! the temporary directory for a session without them) and imported into a
//! Stickers bin; the shapes are graphics templates (category Stickers). Every emoji in the
//! catalogue has English and Russian words, for the search and for placing emoji by what is said.
//!
//! | Command | |
//! |---|---|
//! | `stickers.list` | `{category?, query?}`: the catalogue (`emoji:<char>` and `builtin:sticker-…` ids), favourites and recent |
//! | `stickers.add` | `{id, time?, seconds=3, animation: pop\|fade\|slide\|none, size=0.22, x?, y?}`: on a free track above, one undo step |
//! | `stickers.favorite` | `{id, on?}` |
//! | `stickers.byWords` | `{maxCount?, minGap?}`: an emoji on the phrases that mention one (by words, ru/en) |

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use filmcraft_project::{ClipId, ItemId, TrackKind};
use filmcraft_time::Tick;

use crate::commands::{CommandSpec, always, bad, bool_p, f64_p, has_seq, str_p, u64_p};
use crate::{EngineError, Result, Session};

/// Draws an emoji `size` pixels tall: (width, height, straight RGBA).
pub type EmojiRenderer = Arc<dyn Fn(&str, u32) -> std::result::Result<(u32, u32, Vec<u8>), String> + Send + Sync>;

/// Sticker preferences (`Preferences::stickers`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StickerPrefs {
    pub favorites: Vec<String>,
    /// Newest first.
    pub recent: Vec<String>,
}

/// Recent stickers kept.
const RECENT: usize = 24;

/// The categories, in order: (id, name).
pub const CATEGORIES: [(&str, &str); 7] = [
    ("reactions", "Reactions"),
    ("hands", "Hands"),
    ("hearts", "Hearts and Fire"),
    ("symbols", "Symbols"),
    ("things", "Things"),
    ("nature", "Nature and Food"),
    ("shapes", "Arrows and Shapes"),
];

/// The emoji: (emoji, English name, category, words — English and Russian stems).
pub const EMOJI: &[(&str, &str, &str, &str)] = &[
    ("😂", "Tears of Joy", "reactions", "laugh funny lol joke смех смешн ржу шутк"),
    ("🤣", "Rolling on the Floor", "reactions", "laugh hilarious смех угар"),
    ("😊", "Smiling", "reactions", "smile happy glad улыб рад счаст"),
    ("😍", "Heart Eyes", "reactions", "love beautiful wow обожа красив люблю"),
    ("😎", "Cool", "reactions", "cool sunglasses круто класс"),
    ("🤔", "Thinking", "reactions", "think hmm question думаю размышл вопрос"),
    ("😮", "Surprised", "reactions", "wow surprise ого удивл неожидан"),
    ("😱", "Shocked", "reactions", "shock scary omg шок страшн ужас"),
    ("🤯", "Mind Blown", "reactions", "mind blown amazing взрыв мозг невероятн"),
    ("😭", "Crying", "reactions", "cry sad плач груст"),
    ("😡", "Angry", "reactions", "angry mad злой злюсь бесит"),
    ("🥳", "Party Face", "reactions", "party celebrate birthday праздн вечеринк день рожд"),
    ("😴", "Sleeping", "reactions", "sleep tired boring сон спать устал скучн"),
    ("🙄", "Eye Roll", "reactions", "whatever annoyed закатыва надоел"),
    ("😅", "Nervous Laugh", "reactions", "awkward phew неловк фух"),
    ("🤩", "Star-Struck", "reactions", "amazing excited восторг восхищ"),
    ("🙈", "See No Evil", "reactions", "oops embarrassed стыдно упс"),
    ("👀", "Eyes", "reactions", "look watch see смотр гляд видишь"),
    ("😬", "Grimacing", "reactions", "yikes awkward ой неловк"),
    ("🤫", "Shush", "reactions", "secret quiet секрет тихо тайн"),
    ("👍", "Thumbs Up", "hands", "like good yes ok нравит хорош отличн лайк"),
    ("👎", "Thumbs Down", "hands", "dislike bad плох не нравит"),
    ("👏", "Clapping", "hands", "clap applause bravo аплодис браво молодец"),
    ("🙌", "Raised Hands", "hands", "hooray celebrate ура"),
    ("👋", "Waving", "hands", "hello hi bye привет пока здравств"),
    ("🤝", "Handshake", "hands", "deal partner agreement сделк партнер договор"),
    ("🙏", "Folded Hands", "hands", "thanks please спасибо пожалуйст благодар"),
    ("💪", "Flexed Biceps", "hands", "strong power gym сил мощн трениров спорт"),
    ("✌️", "Victory", "hands", "peace victory побед мир"),
    ("👉", "Pointing Right", "hands", "here look this вот здесь сюда"),
    ("👆", "Pointing Up", "hands", "up above above вверх выше"),
    ("👇", "Pointing Down", "hands", "down below ниже вниз"),
    ("🤞", "Fingers Crossed", "hands", "luck hope удач надеюсь"),
    ("👌", "OK", "hands", "ok perfect окей идеальн норм"),
    ("❤️", "Red Heart", "hearts", "love heart любов люблю сердц"),
    ("💔", "Broken Heart", "hearts", "heartbreak sad расстав разбит"),
    ("🔥", "Fire", "hearts", "fire hot lit огонь горяч жар топ"),
    ("✨", "Sparkles", "hearts", "magic shine new магия волшеб блеск"),
    ("⭐", "Star", "hearts", "star best звезд лучш"),
    ("💯", "Hundred", "hearts", "hundred perfect sure сто процент точно"),
    ("💥", "Boom", "hearts", "boom bang взрыв бум"),
    ("✅", "Check Mark", "symbols", "yes done correct да готово верно правильн"),
    ("❌", "Cross Mark", "symbols", "no wrong mistake нет неправильн ошибк"),
    ("❗", "Exclamation", "symbols", "important attention важн внимани"),
    ("❓", "Question", "symbols", "question why what вопрос почему зачем"),
    ("⚠️", "Warning", "symbols", "warning danger careful осторожн опасн предупрежд"),
    ("⚡", "Lightning", "symbols", "fast energy quick быстр энерги молни"),
    ("💡", "Light Bulb", "symbols", "idea tip insight иде совет лайфхак"),
    ("🎯", "Target", "symbols", "goal target aim цел задач"),
    ("🚀", "Rocket", "symbols", "launch growth start fast запуск рост старт ракет"),
    ("📈", "Chart Up", "symbols", "growth profit up рост прибыл растёт"),
    ("📉", "Chart Down", "symbols", "loss drop down падени убыт снижени"),
    ("🏆", "Trophy", "symbols", "win winner champion побед чемпион приз"),
    ("🎉", "Party Popper", "symbols", "congrats celebrate поздрав праздн"),
    ("🔔", "Bell", "symbols", "subscribe notify bell подпис колокол уведомл"),
    ("💬", "Speech Balloon", "symbols", "comment chat talk коммент чат разговор"),
    ("⏰", "Alarm Clock", "symbols", "time deadline morning врем срок утро"),
    ("🆕", "New", "symbols", "new fresh нов"),
    ("💰", "Money Bag", "things", "money cash rich деньг богат заработ"),
    ("💸", "Money Flying", "things", "spend expensive трат дорог расход"),
    ("📱", "Phone", "things", "phone mobile app телефон смартфон приложени"),
    ("💻", "Laptop", "things", "computer laptop work компьют ноутбук работ"),
    ("📷", "Camera", "things", "photo camera фото камер"),
    ("🎬", "Clapper", "things", "film video movie action кино видео фильм съёмк"),
    ("🎵", "Music", "things", "music song музык песн"),
    ("🎁", "Gift", "things", "gift present подар сюрприз"),
    ("📚", "Books", "things", "study learn book учеб книг учить"),
    ("🧠", "Brain", "things", "brain smart think мозг умн"),
    ("🏠", "House", "things", "home house дом квартир"),
    ("✈️", "Airplane", "things", "travel trip flight путешеств поездк полёт самолёт"),
    ("🚗", "Car", "things", "car drive машин авто"),
    ("📌", "Pin", "things", "pin note remember закреп запомн заметк"),
    ("☕", "Coffee", "nature", "coffee morning кофе утро"),
    ("🍕", "Pizza", "nature", "pizza food еда пицц"),
    ("🍔", "Burger", "nature", "burger food fast бургер еда"),
    ("🍎", "Apple", "nature", "apple healthy яблок полезн"),
    ("🌍", "Globe", "nature", "world earth global мир земл планет"),
    ("☀️", "Sun", "nature", "sun summer weather солнц лето погод"),
    ("🌧️", "Rain", "nature", "rain weather дожд погод"),
    ("❄️", "Snowflake", "nature", "snow winter cold снег зим холод"),
    ("🌱", "Seedling", "nature", "grow plant start рост растени начал"),
    ("🐱", "Cat", "nature", "cat kitty кот кошк"),
    ("🐶", "Dog", "nature", "dog puppy собак пёс щенок"),
];

/// The catalogue entry of `id`: (name, category).
pub fn find(id: &str) -> Option<(&'static str, &'static str)> {
    let e = id.strip_prefix("emoji:")?;
    EMOJI.iter().find(|x| x.0 == e).map(|x| (x.1, x.2))
}

fn emoji_file_name(e: &str) -> String {
    let hex: Vec<String> = e.chars().map(|c| format!("{:x}", c as u32)).collect();
    format!("{}.png", hex.join("-"))
}

/// The PNG of emoji `e`, drawn once and kept.
fn emoji_png(s: &Session, e: &str) -> Result<std::path::PathBuf> {
    // beside the preferences (the app's data directory); a session without them (tests, scripts)
    // keeps its own in the temporary directory, so it never mixes with the app's
    let dir = s
        .prefs_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join("stickers/emoji"))
        .unwrap_or_else(|| crate::temp_dir().join(format!("jilo-stickers-{}", std::process::id())));
    let path = dir.join(emoji_file_name(e));
    if path.is_file() {
        return Ok(path);
    }
    let r = s.emoji_renderer.as_ref().ok_or_else(|| EngineError::Other("emoji stickers need the system emoji font (macOS)".into()))?;
    let (w, h, rgba) = r(e, 512).map_err(EngineError::Other)?;
    if rgba.len() != (w as usize) * (h as usize) * 4 {
        return Err(EngineError::Other("the emoji picture is the wrong size".into()));
    }
    let png = filmcraft_export::encode_png(rgba, w, h).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| EngineError::Other(e.to_string()))?;
    std::fs::write(&path, png).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// The Stickers bin, made if needed.
fn stickers_bin(s: &mut Session) -> Option<u64> {
    let find = |s: &Session| {
        s.project.root.children.iter().find_map(|e| match e {
            filmcraft_project::BinEntry::Bin(b) if b.name == "Stickers" => Some(b.id.0),
            _ => None,
        })
    };
    if find(s).is_none() {
        let _ = s.execute("file.newBin", json!({"name": "Stickers"}));
    }
    find(s)
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let q = str_p(p, "query").unwrap_or("").to_lowercase();
    let cat = str_p(p, "category");
    let prefs = &s.prefs.stickers;
    let templates: Vec<(String, String)> = crate::graphic_templates::library(s)
        .into_iter()
        .filter(|e| e.template.category == "Stickers")
        .map(|e| (e.template.id.clone(), e.template.name.clone()))
        .collect();
    let mut all: Vec<Value> = EMOJI
        .iter()
        .map(|(e, name, c, words)| json!({"id": format!("emoji:{e}"), "emoji": e, "name": name, "category": c, "words": words}))
        .chain(templates.iter().map(|(id, name)| json!({"id": id, "name": name, "category": "shapes", "words": name.to_lowercase()})))
        .collect();
    for v in &mut all {
        let id = v["id"].as_str().unwrap_or_default().to_string();
        v["favorite"] = json!(prefs.favorites.contains(&id));
    }
    let keep = |v: &Value| -> bool {
        let id = v["id"].as_str().unwrap_or_default();
        let in_cat = match cat {
            Some("favorites") => prefs.favorites.iter().any(|f| f == id),
            Some("recent") => prefs.recent.iter().any(|f| f == id),
            Some(c) => v["category"] == c,
            None => true,
        };
        let hit = q.is_empty()
            || v["name"].as_str().unwrap_or("").to_lowercase().contains(&q)
            || v["words"].as_str().unwrap_or("").split(' ').any(|w| w.starts_with(&q) || (w.chars().count() >= 4 && q.starts_with(w)));
        in_cat && hit
    };
    let mut out: Vec<Value> = all.into_iter().filter(keep).collect();
    if cat == Some("recent") {
        out.sort_by_key(|v| prefs.recent.iter().position(|r| v["id"] == r.as_str()).unwrap_or(usize::MAX));
    }
    Ok(
        json!({"stickers": out, "categories": CATEGORIES.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(), "emoji": s.emoji_renderer.is_some()}),
    )
}

/// Keyframes of an entrance on `clip`'s Motion and Opacity, from `start` (sequence time).
fn entrance(s: &mut Session, clip: ClipId, how: &str, scale: f64, pos: (f64, f64)) -> Result<()> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (_, it) = q.find_item(clip).ok_or(EngineError::NoSequence)?;
    let (t0, rate) = (it.start, q.settings.frame_rate);
    let at = |frames: i64| (t0 + rate.tick_of(frames)).0;
    let key = |s: &mut Session, effect: &str, param: &str, frames: i64, value: Value| {
        s.execute("effects.setParam", json!({"clip": clip.0, "effect": effect, "param": param, "value": value, "time": at(frames), "keyframe": true}))
    };
    match how {
        "pop" => {
            key(s, "motion", "scale", 0, json!(0.0))?;
            key(s, "motion", "scale", 5, json!(scale * 1.15))?;
            key(s, "motion", "scale", 8, json!(scale))?;
        }
        "fade" => {
            key(s, "opacity", "opacity", 0, json!(0.0))?;
            key(s, "opacity", "opacity", 8, json!(100.0))?;
        }
        "slide" => {
            let h = f64::from(q_height(s));
            key(s, "motion", "position", 0, json!([pos.0, pos.1 + h * 0.25]))?;
            key(s, "motion", "position", 8, json!([pos.0, pos.1]))?;
            key(s, "opacity", "opacity", 0, json!(0.0))?;
            key(s, "opacity", "opacity", 4, json!(100.0))?;
        }
        _ => {}
    }
    Ok(())
}

fn q_height(s: &Session) -> u32 {
    s.active_sequence().map_or(1080, |q| q.settings.height)
}

/// Put sticker `id` on the picture; returns its clip.
fn add_one(s: &mut Session, id: &str, at: Tick, seconds: f64, how: &str, size: f64, x: f64, y: f64) -> Result<ClipId> {
    let cmd = "stickers.add";
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let (w, h, rate) = (f64::from(q.settings.width), f64::from(q.settings.height), q.settings.frame_rate);
    let dur = rate.snap_nearest(Tick::from_seconds_f64(seconds));
    let at = rate.snap_nearest(at);
    let pos = (x * w, y * h);
    if let Some(e) = id.strip_prefix("emoji:") {
        if find(id).is_none() && e.chars().count() > 8 {
            return Err(bad(cmd, format!("unknown sticker `{id}`")));
        }
        let png = emoji_png(s, e)?;
        let bin = stickers_bin(s);
        let r = s.execute("file.import", json!({"paths": [png.to_string_lossy()], "bin": bin}))?;
        let item = r["items"][0]
            .as_u64()
            .or_else(|| r["duplicates"][0]["item"].as_u64())
            .map(ItemId)
            .ok_or_else(|| EngineError::Other("the sticker could not be imported".into()))?;
        let track = crate::paste_media::free_track(s, TrackKind::Video, at, at + dur)?;
        let placed = s.execute("timeline.place", json!({"item": item.0, "track": track.0, "time": at.0, "duration": dur.0, "audio": false}))?;
        let clip = placed["clips"][0].as_u64().map(ClipId).ok_or_else(|| EngineError::Other("the sticker was not placed".into()))?;
        let ih = s.source(item).and_then(|src| src.info().video.as_ref().map(|v| f64::from(v.height.max(1)))).unwrap_or(512.0);
        let scale = size * h / ih * 100.0;
        s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "scale", "value": scale}))?;
        s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "position", "value": [pos.0, pos.1]}))?;
        entrance(s, clip, how, scale, pos)?;
        return Ok(clip);
    }
    if !id.starts_with("builtin:sticker-") && crate::graphic_templates::find_template(s, id).is_err() {
        return Err(bad(cmd, format!("unknown sticker `{id}`")));
    }
    let r = s.execute("graphics.template.apply", json!({"template": id, "time": at.0}))?;
    let clip = r["clip"].as_u64().map(ClipId).ok_or_else(|| EngineError::Other("the sticker was not placed".into()))?;
    // shapes are drawn for the frame's centre: move them where asked
    let centre = (w / 2.0, h / 2.0);
    if (pos.0 - centre.0).abs() > 0.5 || (pos.1 - centre.1).abs() > 0.5 {
        s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "position", "value": [pos.0, pos.1]}))?;
    }
    entrance(s, clip, how, 100.0, pos)?;
    Ok(clip)
}

fn remember(s: &mut Session, id: &str) {
    let mut prefs = s.prefs.clone();
    prefs.stickers.recent.retain(|r| r != id);
    prefs.stickers.recent.insert(0, id.to_string());
    prefs.stickers.recent.truncate(RECENT);
    let _ = s.set_prefs(prefs);
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "stickers.add";
    let id = str_p(p, "id").ok_or_else(|| bad(cmd, "`id`: emoji:😀 or builtin:sticker-…"))?.to_string();
    let how = str_p(p, "animation").unwrap_or("pop");
    if !matches!(how, "pop" | "fade" | "slide" | "none") {
        return Err(bad(cmd, "`animation`: pop, fade, slide or none"));
    }
    let at = p.get("time").and_then(Value::as_i64).map(Tick).unwrap_or(s.playhead());
    let seconds = f64_p(p, "seconds").unwrap_or(3.0).clamp(0.2, 600.0);
    let size = f64_p(p, "size").unwrap_or(0.22).clamp(0.02, 1.0);
    let (x, y) = (f64_p(p, "x").unwrap_or(0.5).clamp(0.0, 1.0), f64_p(p, "y").unwrap_or(0.35).clamp(0.0, 1.0));
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let selection = s.state.selection.clone();
    match add_one(s, &id, at, seconds, how, size, x, y) {
        Ok(clip) => {
            crate::recipes::collapse_undo(s, before, "Add Sticker");
            s.state.selection = vec![clip];
            remember(s, &id);
            Ok(json!({"clip": clip.0, "id": id}))
        }
        Err(e) => {
            s.revert_to(mark);
            s.state.selection = selection;
            Err(e)
        }
    }
}

fn favorite(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_p(p, "id").ok_or_else(|| bad("stickers.favorite", "`id`"))?.to_string();
    let mut prefs = s.prefs.clone();
    let on = bool_p(p, "on").unwrap_or(!prefs.stickers.favorites.contains(&id));
    prefs.stickers.favorites.retain(|f| *f != id);
    if on {
        prefs.stickers.favorites.push(id.clone());
    }
    s.set_prefs(prefs).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"id": id, "favorite": on}))
}

/// The emoji whose words a phrase's words share most (stems of five letters, or whole short words).
pub fn emoji_for(words: &[String]) -> Option<&'static str> {
    let mut best: Option<(usize, &str)> = None;
    for (e, _, _, kw) in EMOJI {
        // a short word must be the word itself; a longer one may be its stem ("смешн" → "смешно")
        let hit = |w: &String, k: &str| w == k || (k.chars().count() >= 4 && w.starts_with(k)) || (k.chars().count() >= 5 && crate::broll::same_word(w, k));
        let n = kw.split(' ').filter(|k| k.chars().count() >= 2).filter(|k| words.iter().any(|w| hit(w, k))).count();
        if n > 0 && best.is_none_or(|b| n > b.0) {
            best = Some((n, e));
        }
    }
    best.map(|b| b.1)
}

fn by_words(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "stickers.byWords";
    let words = crate::transcript::sequence_words(s);
    let cues = crate::highlights::cues_from_words(&words);
    if cues.is_empty() {
        return Err(bad(cmd, "the sequence has no transcript (Transcribe first)"));
    }
    let max = u64_p(p, "maxCount").unwrap_or(8).clamp(1, 100) as usize;
    let gap = f64_p(p, "minGap").unwrap_or(5.0).max(0.0);
    let mut plan: Vec<(f64, &str)> = Vec::new();
    for c in &cues {
        if plan.len() >= max || plan.last().is_some_and(|l| c.start < l.0 + gap) {
            continue;
        }
        let toks: Vec<String> = c.text.split(|ch: char| !ch.is_alphanumeric()).map(str::to_lowercase).filter(|w| !w.is_empty()).collect();
        if let Some(e) = emoji_for(&toks) {
            plan.push((c.start, e));
        }
    }
    let before = std::sync::Arc::as_ptr(&s.project) as usize;
    let mark = s.undo_mark();
    let mut placed = Vec::new();
    for (t, e) in &plan {
        match add_one(s, &format!("emoji:{e}"), Tick::from_seconds_f64(*t), 2.0, "pop", 0.18, 0.78, 0.25) {
            Ok(c) => placed.push(json!({"clip": c.0, "emoji": e, "seconds": t})),
            Err(err) => {
                s.revert_to(mark);
                return Err(err);
            }
        }
    }
    crate::recipes::collapse_undo(s, before, "Emoji by Words");
    Ok(json!({"placed": placed, "phrases": cues.len()}))
}

pub fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "stickers.list",
            label: "List Stickers",
            menu: &[],
            shortcut: None,
            params: r#"{"category":str?,"query":str?}"#,
            enabled: always,
            run: list,
            journal: false,
        },
        CommandSpec {
            id: "stickers.add",
            label: "Add Sticker",
            menu: &[],
            shortcut: None,
            params: r#"{"id":"emoji:😀|builtin:sticker-…","time":ticks?,"seconds":f=3,"animation":"pop|fade|slide|none"="pop","size":0.02..1=0.22,"x":0..1=0.5,"y":0..1=0.35}"#,
            enabled: has_seq,
            run: add,
            journal: true,
        },
        CommandSpec {
            id: "stickers.favorite",
            label: "Favorite Sticker",
            menu: &[],
            shortcut: None,
            params: r#"{"id":str,"on":bool?}"#,
            enabled: always,
            run: favorite,
            journal: false,
        },
        CommandSpec {
            id: "stickers.byWords",
            label: "Emoji by What Is Said",
            menu: crate::ai_menu::EDITING,
            shortcut: None,
            params: r#"{"maxCount":n=8,"minGap":seconds=5}"#,
            enabled: has_seq,
            run: by_words,
            journal: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(s: &mut Session) {
        s.emoji_renderer = Some(Arc::new(|_: &str, size: u32| Ok((size, size, vec![255u8; (size * size * 4) as usize]))));
    }

    #[test]
    fn the_catalogue_is_searchable_in_russian_and_english() {
        let mut s = Session::default();
        let ids = |s: &mut Session, q: &str| -> Vec<String> {
            s.execute("stickers.list", json!({"query": q})).unwrap()["stickers"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v["id"].as_str().map(str::to_string))
                .collect()
        };
        assert!(ids(&mut s, "огонь").contains(&"emoji:🔥".to_string()));
        assert!(ids(&mut s, "money").contains(&"emoji:💰".to_string()));
        assert!(ids(&mut s, "arrow").contains(&"builtin:sticker-arrow-right".to_string()));
        let mut seen: Vec<&str> = EMOJI.iter().map(|e| e.0).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), EMOJI.len(), "no emoji twice");
        assert!(EMOJI.iter().all(|e| CATEGORIES.iter().any(|c| c.0 == e.2)));
        assert_eq!(emoji_for(&["огонь".into(), "просто".into()]), Some("🔥"));
        assert_eq!(emoji_for(&["спасибо".into()]), Some("🙏"));
        assert_eq!(emoji_for(&["стол".into()]), None);
    }

    #[test]
    fn a_sticker_goes_on_a_free_track_with_a_pop_and_is_remembered() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        fake(&mut s);
        let undo = s.history.undo.len();
        let r = s.execute("stickers.add", json!({"id": "emoji:🔥", "seconds": 2.0})).unwrap();
        assert_eq!(s.history.undo.len(), undo + 1, "one undo step");
        let clip = ClipId(r["clip"].as_u64().unwrap());
        let q = s.active_sequence().unwrap();
        let (_, it) = q.find_item(clip).unwrap();
        let scale = it.effect("motion").unwrap().param("scale").unwrap();
        assert!(scale.is_animated(), "it pops in");
        assert_eq!(s.prefs.stickers.recent.first().map(String::as_str), Some("emoji:🔥"));
        // a shape sticker, faded in
        let r = s.execute("stickers.add", json!({"id": "builtin:sticker-arrow-right", "animation": "fade"})).unwrap();
        let (_, it) = s.active_sequence().unwrap().find_item(ClipId(r["clip"].as_u64().unwrap())).unwrap();
        assert!(it.effect("opacity").unwrap().param("opacity").unwrap().is_animated());
        s.execute("stickers.favorite", json!({"id": "emoji:🔥"})).unwrap();
        let favs = s.execute("stickers.list", json!({"category": "favorites"})).unwrap();
        assert_eq!(favs["stickers"].as_array().unwrap().len(), 1);
        assert!(s.execute("stickers.add", json!({"id": "emoji:🔥", "animation": "spin"})).is_err());
    }

    #[test]
    fn emoji_go_on_the_phrases_that_mention_them() {
        let mut s = Session::default();
        s.execute("file.openDemoProject", json!({})).unwrap();
        fake(&mut s);
        let a1 = s.active_sequence().unwrap().audio_tracks[0].items[0].clone();
        let at = |x: f64| (a1.source_in + Tick::from_seconds_f64(x)).0;
        let w = |t: &str, a: f64, b: f64| json!({"text": t, "start": at(a), "end": at(b)});
        let words = json!([w("Это", 0.0, 0.3), w("огонь!", 0.3, 0.8), w("Большое", 2.0, 2.4), w("спасибо.", 2.4, 3.0), w("Стол", 3.6, 3.9)]);
        s.execute("transcript.set", json!({"item": a1.item.0, "transcript": {"language": "ru", "speakers": [], "words": words}})).unwrap();
        let undo = s.history.undo.len();
        let r = s.execute("stickers.byWords", json!({"minGap": 1.0})).unwrap();
        let got: Vec<&str> = r["placed"].as_array().unwrap().iter().filter_map(|p| p["emoji"].as_str()).collect();
        assert_eq!(got, ["🔥", "🙏"], "{r}");
        assert_eq!(s.history.undo.len(), undo + 1);
    }
}
