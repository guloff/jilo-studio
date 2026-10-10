//! The window after a run that ended without quitting (`crash::begin_session`): what is known
//! about it — the version, the panics logged before it ended, macOS's own crash report — to copy
//! into an issue. Nothing is sent; Report an Issue… opens the repository's issues in the browser.

use egui::RichText;

use crate::FilmcraftApp;

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.crash_report_taken {
        app.ui.crash_report_taken = true;
        app.ui.crash_report = crate::crash::take_previous();
    }
    let Some(report) = app.ui.crash_report.clone() else { return };
    let t = app.tokens;
    let (mut open, mut close) = (true, false);
    crate::dialog_style::Window::new(tlf!("{app} Quit Unexpectedly", app = crate::brand::APP_NAME))
        .id(egui::Id::new("crash-report"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(520.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add(
                egui::Label::new(tlf!(
                    "{app} quit unexpectedly the last time it ran. If it happens again, the report below helps to fix it: copy it into an issue on GitHub. Nothing is sent.",
                    app = crate::brand::APP_NAME
                ))
                .wrap(),
            );
            egui::Frame::new().fill(t.field_bg).corner_radius(4.0).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&report).monospace().small()).wrap());
                });
            });
            ui.label(RichText::new(tl!("Unsaved changes, if there were any, are offered for recovery separately.")).color(t.text_dim).small());
            ui.add_space(4.0);
            crate::dialog_style::actions(ui, |ui| {
                let c = ui.add(crate::dialog_style::primary(tl!("Close")));
                app.auto.add("crash.close", c.rect, "Close");
                let copy = ui.add(crate::dialog_style::secondary(tl!("Copy Report")));
                app.auto.add("crash.copy", copy.rect, "Copy Report");
                let issue = ui.add(crate::dialog_style::secondary(tl!("Report an Issue…")));
                app.auto.add("crash.issue", issue.rect, "Report an Issue…");
                if copy.clicked() {
                    ctx.copy_text(report.clone());
                    app.ui.status = tl!("The report is copied").to_string();
                }
                if issue.clicked() {
                    crate::links::open(ctx, crate::links::ISSUES);
                }
                close = c.clicked();
            });
        });
    if !open || close || crate::widgets::escape_closes(ctx) {
        app.ui.crash_report = None;
    }
}
