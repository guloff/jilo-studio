//! Help ▸ Tips…: one short tip at a time about what makes editing quick here — the start from a
//! video, the palette, editing by text, the quick actions, shorts, captions, stickers, sounds,
//! exports for the platforms, dubbing, recipes, undo. Shown when the app starts until "Show tips
//! when Jilo Studio starts" is turned off (here or in Settings ▸ General).
//!
//! Automation ids: `tips.window`, `tips.next`, `tips.previous`, `tips.atStart`.

use egui::RichText;
use serde_json::json;

use crate::FilmcraftApp;

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

pub fn open(app: &mut FilmcraftApp) {
    app.ui.tips.open = true;
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
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
            ui.horizontal(|ui| {
                let r = ui.checkbox(&mut at_start, tl!("Show tips when FilmCraft starts"));
                app.auto.add("tips.atStart", r.rect, "Show tips at start");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = ui.button(tl!("Next Tip"));
                    app.auto.add("tips.next", n.rect, "Next Tip");
                    if n.clicked() {
                        app.ui.tips.index = (i + 1) % TIPS.len();
                    }
                    let p = ui.button(tl!("Previous"));
                    app.auto.add("tips.previous", p.rect, "Previous");
                    if p.clicked() {
                        app.ui.tips.index = (i + TIPS.len() - 1) % TIPS.len();
                    }
                });
            });
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
