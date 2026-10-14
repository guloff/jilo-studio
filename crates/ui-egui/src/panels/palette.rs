//! Command palette (⌘J, Window ▸ Command Palette…): type a few letters of anything the app can do —
//! a menu item in the interface language or in English, a panel, a recipe — and press Return.
//! Matching forgives gaps ("rmpau" finds Remove Pauses); whole words and starts of words rank
//! first. With nothing typed it lists the commands run from it recently. Up / Down choose, Return
//! runs, Escape closes. Commands that are not available right now are listed dimmed with the reason.
//!
//! Automation ids: `palette.query`, `palette.item.<n>` (the n-th result).

use egui::{Align2, Color32, Key, RichText, vec2};
use serde_json::{Value, json};

use crate::FilmcraftApp;
use crate::theme::Tokens;

/// The palette's state.
#[derive(Clone, Debug, Default)]
pub struct PaletteUi {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    /// Ids run from the palette, newest first.
    pub recent: Vec<String>,
}

/// One thing the palette can run.
#[derive(Clone, Debug)]
pub struct Entry {
    /// A command id, or `recipe:<name>`.
    pub id: String,
    pub label: String,
    /// The English label (matched too).
    pub english: String,
    /// Where it lives in the menus, in the interface language.
    pub path: String,
    pub shortcut: Option<String>,
    pub enabled: bool,
}

const MAX_SHOWN: usize = 12;

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    app.ui.palette.open = true;
    app.ui.palette.query.clear();
    app.ui.palette.selected = 0;
    Ok(json!({"palette": true}))
}

/// Everything the palette offers: the menu commands and the recipes.
pub fn entries(app: &FilmcraftApp) -> Vec<Entry> {
    let english = crate::menus::menu_items_for(&app.session);
    let mut v: Vec<Entry> = crate::menus::menu_items(app)
        .into_iter()
        .filter(|it| it.id != "ui.commandPalette")
        .map(|it| {
            let en = english.iter().find(|e| e.id == it.id).map_or_else(|| it.label.clone(), |e| e.label.clone());
            let path = it.path.iter().map(|p| crate::i18n::t(p).to_string()).collect::<Vec<_>>().join(" › ");
            Entry {
                label: it.label.trim_end_matches('…').to_string(),
                english: en.trim_end_matches('…').to_string(),
                path,
                shortcut: it.shortcut,
                enabled: it.enabled,
                id: it.id,
            }
        })
        .collect();
    for r in filmcraft_engine::recipes::all() {
        v.push(Entry {
            id: format!("recipe:{}", r.name),
            label: tlf!("Run Recipe: {name}", name = crate::i18n::t(&r.name)),
            english: format!("Run Recipe: {}", r.name),
            path: format!("{} › {}", crate::i18n::t("AI"), tl!("Recipes")),
            shortcut: None,
            enabled: true,
        });
    }
    v
}

/// How well `query` matches `text` (None: not at all). Higher is better: the whole query as a
/// word or at the start beats a substring, which beats letters in order with gaps.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t = text.to_lowercase();
    let qs: String = query.trim().to_lowercase();
    if let Some(i) = t.find(&qs) {
        let word_start = i == 0 || t[..i].ends_with([' ', '›', '-', '(']);
        return Some(1000 - i as i32 + if word_start { 300 } else { 0 } - (t.chars().count() as i32 - qs.chars().count() as i32).max(0));
    }
    // letters in order: fewer and shorter gaps score higher, starts of words count extra
    let tc: Vec<char> = t.chars().collect();
    let (mut qi, mut s, mut last) = (0usize, 0i32, None::<usize>);
    for (i, c) in tc.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            let start = i == 0 || matches!(tc[i - 1], ' ' | '›' | '-' | '(');
            s += if start { 12 } else { 4 };
            if let Some(l) = last {
                s -= ((i - l - 1) as i32).min(6);
            }
            last = Some(i);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(s)
}

/// The entries matching `query`, best first (the recent ones first when nothing is typed).
pub fn search(entries: &[Entry], query: &str, recent: &[String]) -> Vec<usize> {
    if query.trim().is_empty() {
        let mut v: Vec<usize> = recent.iter().filter_map(|id| entries.iter().position(|e| &e.id == id)).collect();
        v.truncate(MAX_SHOWN);
        return v;
    }
    let mut hits: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let best = [score(query, &e.label), score(query, &e.english).map(|s| s - 5), score(query, &format!("{} {}", e.path, e.label)).map(|s| s - 40)]
                .into_iter()
                .flatten()
                .max()?;
            Some((best + if e.enabled { 0 } else { -200 }, i))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, i)| i).collect()
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.palette.open {
        return;
    }
    let t = app.tokens;
    let all = entries(app);
    let found = search(&all, &app.ui.palette.query, &app.ui.palette.recent);
    let n = found.len().min(MAX_SHOWN);
    let (up, down, enter, esc) =
        ctx.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown), i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    let p = &mut app.ui.palette;
    if down && n > 0 {
        p.selected = (p.selected + 1) % n;
    }
    if up && n > 0 {
        p.selected = (p.selected + n - 1) % n;
    }
    p.selected = p.selected.min(n.saturating_sub(1));
    let mut run: Option<usize> = (enter && n > 0).then(|| found[p.selected]);
    let mut close = esc;
    let w = 560.0;
    egui::Area::new(egui::Id::new("command-palette")).anchor(Align2::CENTER_TOP, vec2(0.0, 90.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        crate::dialog_style::frame(&t).show(ui, |ui| {
            ui.set_width(w);
            let before = app.ui.palette.query.clone();
            let r = ui.add(
                egui::TextEdit::singleline(&mut app.ui.palette.query)
                    .hint_text(tl!("Type a command, a panel or a recipe…"))
                    .desired_width(w - 16.0)
                    .font(Tokens::ui(15.0)),
            );
            app.auto.add("palette.query", r.rect, "Command palette");
            r.request_focus();
            if app.ui.palette.query != before {
                app.ui.palette.selected = 0;
            }
            ui.add_space(4.0);
            if n == 0 {
                let msg = if app.ui.palette.query.trim().is_empty() { tl!("Commands you run from here appear first.") } else { tl!("No command matches.") };
                ui.label(RichText::new(msg).color(t.text_dim));
            }
            for (k, &i) in found.iter().take(MAX_SHOWN).enumerate() {
                let e = &all[i];
                let sel = k == app.ui.palette.selected;
                let resp = ui
                    .horizontal(|ui| {
                        ui.set_min_height(26.0);
                        let col = if e.enabled { t.text } else { t.text_faint };
                        ui.label(RichText::new(&e.label).color(col).size(13.5));
                        ui.label(RichText::new(&e.path).color(t.text_faint).size(11.0));
                        if let Some(s) = &e.shortcut {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(crate::menus::shortcut_text(s)).color(t.text_dim).size(11.5));
                            });
                        }
                    })
                    .response;
                let resp = ui.interact(resp.rect, egui::Id::new(("palette-row", k)), egui::Sense::click());
                if sel || resp.hovered() {
                    ui.painter().rect_filled(resp.rect, 4.0, if sel { Color32::from_white_alpha(18) } else { Color32::from_white_alpha(10) });
                }
                app.auto.add(&format!("palette.item.{k}"), resp.rect, &e.label);
                if resp.clicked() {
                    run = Some(i);
                }
            }
        });
    });
    if let Some(i) = run {
        let e = all[i].clone();
        close = true;
        let p = &mut app.ui.palette;
        p.recent.retain(|r| r != &e.id);
        p.recent.insert(0, e.id.clone());
        p.recent.truncate(20);
        let r = match e.id.strip_prefix("recipe:") {
            Some(name) => app.session.execute("recipes.run", json!({"name": name})).map_err(|e| e.to_string()),
            None => crate::menus::invoke(app, ctx, &e.id, json!({})),
        };
        if let Err(err) = r {
            app.ui.status = err;
        }
    }
    if close {
        app.ui.palette.open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoring_prefers_words_then_letters_in_order() {
        assert!(score("rmpau", "Remove Pauses").is_some(), "letters in order with gaps");
        assert!(score("xyz", "Remove Pauses").is_none());
        assert!(score("remove", "Remove Pauses").unwrap() > score("move", "Remove Pauses").unwrap());
        assert!(score("пауз", "Убрать паузы").is_some(), "matches the interface language");
    }
}
