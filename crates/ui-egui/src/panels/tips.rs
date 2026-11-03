//! Help ▸ Tips…: one short tip at a time about what makes editing quick here — the start from a
//! video, the palette, editing by text, the quick actions, shorts, captions, stickers, sounds,
//! exports for the platforms, dubbing, recipes, undo. Shown when the app starts until "Show tips
//! when Jilo Studio starts" is turned off (here or in Settings ▸ General).
//!
//! Automation ids: `tips.window`, `tips.next`, `tips.previous`, `tips.atStart`.

use egui::RichText;
use serde_json::json;

use crate::JiloApp;

/// The tips, in order.
pub const TIPS: [&str; 14] = [
    "Drop a video on the home screen: a vertical short with captions, or a clean talking head, in one click.",
    "⌘J opens the command palette: type what you want to do, in your own words.",
    "AI ▸ Edit by Text: delete words in the transcript and the picture and sound are cut with them.",
    "Select a clip and quick actions appear under the picture: split, speed, reframe, delete.",
    "Right-click a transition in the Library to put it on every cut; an effect or a look on every clip.",
    "AI ▸ Cut into Shorts finds the best moments and makes vertical shorts with captions.",
    "Select a caption and drag it in the Program monitor to place it anywhere.",
    "The Stickers tab: emoji, arrows, stickers cut out of your clips; Attach to Face follows a face.",
    "The Library's Audio tab has sound effects and music beds made on this computer, free to use.",
    "Export ▸ Social Platforms: TikTok, Reels and Shorts at the right size and loudness.",
    "AI ▸ Dub puts your speech into another language with a voice.",
    "View ▸ Simple Mode hides what you don't need yet; everything stays in the menus and ⌘J.",
    "Recipes record what you do and repeat it on another clip with one click.",
    "Every AI step is one undo: ⌘Z puts it all back.",
];

#[derive(Clone, Debug, Default)]
pub struct TipsUi {
    pub open: bool,
    pub index: usize,
}

pub fn open(app: &mut JiloApp) {
    app.ui.tips.open = true;
}

pub fn show(app: &mut JiloApp, ctx: &egui::Context) {
    if !app.ui.tips.open {
        return;
    }
    let t = app.tokens;
    let mut open = true;
    let mut at_start = app.session.prefs.general.show_tips;
    let i = app.ui.tips.index % TIPS.len();
    crate::dialog_style::Window::new(tl!("Tips"))
        .id(egui::Id::new("tips-window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(420.0);
            app.auto.add("tips.window", ui.min_rect(), "Tips");
            ui.label(RichText::new(tlf!("Tip {n} of {total}", n = i + 1, total = TIPS.len())).small().color(t.text_dim));
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(crate::i18n::t(TIPS[i])).size(15.0)).wrap());
            ui.add_space(10.0);
            let (check, next, previous) = (tl!("Show tips when Jilo Studio starts"), tl!("Next Tip"), tl!("Previous"));
            // the check box and the buttons share a row when they fit (in Russian they do not)
            let font = egui::TextStyle::Button.resolve(ui.style());
            let width = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), t.text).size().x;
            let sp = ui.spacing();
            let need =
                width(check) + sp.icon_width + sp.icon_spacing + width(next) + width(previous) + 2.0 * sp.button_padding.x * 2.0 + 3.0 * sp.item_spacing.x;
            let one_row = need <= ui.available_width();
            let buttons = |ui: &mut egui::Ui, app: &mut JiloApp| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = ui.button(next);
                    app.auto.add("tips.next", n.rect, "Next Tip");
                    if n.clicked() {
                        app.ui.tips.index = (i + 1) % TIPS.len();
                    }
                    let p = ui.button(previous);
                    app.auto.add("tips.previous", p.rect, "Previous");
                    if p.clicked() {
                        app.ui.tips.index = (i + TIPS.len() - 1) % TIPS.len();
                    }
                });
            };
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut at_start, check);
                app.auto.add("tips.atStart", r.rect, "Show tips at start");
                if one_row {
                    buttons(ui, app);
                }
            });
            if !one_row {
                ui.add_space(4.0);
                ui.horizontal(|ui| buttons(ui, app));
            }
        });
    if at_start != app.session.prefs.general.show_tips {
        let _ = app.session.execute("prefs.set", json!({"key": "general.showTips", "value": at_start}));
    }
    if !open {
        app.ui.tips.open = false;
        // the next start shows the next tip
        app.ui.tips.index = (i + 1) % TIPS.len();
    }
}
