//! AI ▸ Voice and Sound ▸ Auto Mix…: the sound on autopilot — audio types for untyped clips, speech
//! at one loudness, music ducked under speech, the mix at a platform's loudness (`audio.autoMix`,
//! one undo step) — and the music's beats: markers on them (`audio.beatMarkers`) and the cuts
//! between the selected clips rolled onto them (`sequence.cutToBeat`).
//!
//! Automation ids: `autoMix.dialog`, `autoMix.platform`, `autoMix.step.<tag|match|duck|normalize>`,
//! `autoMix.go`, `autoMix.every`, `autoMix.markBeats`, `autoMix.cutToBeat`, `autoMix.close`.

use egui::RichText;
use filmcraft_engine::automix::PLATFORMS;
use serde_json::{Value, json};

use crate::FilmcraftApp;

const STEPS: [(&str, &str); 4] = [
    ("tag", "Tell speech from music (audio types)"),
    ("match", "Speech at one loudness"),
    ("duck", "Music quieter under speech"),
    ("normalize", "The mix at the platform's loudness"),
];

#[derive(Clone, Debug)]
pub struct AutoMixUi {
    pub dialog: bool,
    pub platform: String,
    pub steps: [bool; 4],
    /// Beats between markers and cuts.
    pub every: u32,
    pub message: String,
}

impl Default for AutoMixUi {
    fn default() -> Self {
        Self { dialog: false, platform: "youtube".into(), steps: [true; 4], every: 1, message: String::new() }
    }
}

pub fn open(app: &mut FilmcraftApp) -> Result<Value, String> {
    filmcraft_engine::find_command("audio.autoMix").map_or(Ok(()), |c| (c.enabled)(&app.session))?;
    app.ui.auto_mix.dialog = true;
    app.ui.auto_mix.message.clear();
    Ok(json!({"dialog": "autoMix"}))
}

fn mix_message(r: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(t) = r.get("tagged") {
        parts.push(tlf!("speech clips: {d}, music clips: {m}", d = t["dialogue"].as_u64().unwrap_or(0), m = t["music"].as_u64().unwrap_or(0)));
    }
    if let Some(g) = r["gainDb"].as_f64() {
        parts.push(tlf!(
            "mix {lufs} LUFS → {target} LUFS ({gain} dB)",
            lufs = format!("{:.1}", r["measuredLufs"].as_f64().unwrap_or(0.0)),
            target = format!("{:.0}", r["targetLufs"].as_f64().unwrap_or(0.0)),
            gain = format!("{g:+.1}")
        ));
    }
    format!("{} {}", tl!("Mixed:"), parts.join("; "))
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.auto_mix.dialog {
        return;
    }
    let t = app.tokens;
    let (mut go, mut close, mut mark, mut cut) = (false, false, false, false);
    let w = 470.0;
    egui::Modal::new(egui::Id::new("auto-mix-dialog")).frame(crate::dialog_style::frame(&t)).show(ctx, |ui| {
        ui.set_width(w);
        let bar = crate::dialog_style::header(ui, tl!("Auto Mix"), w);
        app.auto.add("autoMix.dialog", bar, "Auto Mix");
        egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let u = &mut app.ui.auto_mix;
            ui.horizontal(|ui| {
                ui.label(tl!("For"));
                let cur = PLATFORMS.iter().find(|p| p.0 == u.platform).map_or_else(String::new, |p| format!("{} · {} LUFS", crate::i18n::t(p.1), p.2));
                let r = egui::ComboBox::from_id_salt("autoMix.platform").width(280.0).selected_text(cur).show_ui(ui, |ui| {
                    for (id, name, lufs, _) in PLATFORMS {
                        ui.selectable_value(&mut u.platform, id.to_string(), format!("{} · {lufs} LUFS", crate::i18n::t(name)));
                    }
                });
                app.auto.add("autoMix.platform", r.response.rect, "Platform");
            });
            let u = &mut app.ui.auto_mix;
            for (k, (id, name)) in STEPS.iter().enumerate() {
                let r = ui.checkbox(&mut u.steps[k], crate::i18n::t(name));
                app.auto.add(&format!("autoMix.step.{id}"), r.rect, name);
            }
            ui.label(RichText::new(tl!("Speech is what has a transcript or is on a voice track; music is on a music track, or a long sound-only file. Everything happens on this computer. One Undo takes it back.")).color(t.text_dim).small());
            ui.horizontal(|ui| {
                let g = ui.add_enabled(app.ui.auto_mix.steps.iter().any(|x| *x), crate::dialog_style::primary(tl!("Mix")));
                app.auto.add("autoMix.go", g.rect, "Mix");
                go = g.clicked();
            });
            ui.separator();
            ui.label(RichText::new(tl!("Music beats")).strong());
            ui.horizontal(|ui| {
                ui.label(tl!("Every"));
                let u = &mut app.ui.auto_mix;
                let r = ui.add(egui::DragValue::new(&mut u.every).range(1..=16).speed(0.1));
                app.auto.add("autoMix.every", r.rect, "Every n beats");
                ui.label(tl!("beat(s)"));
            });
            ui.horizontal(|ui| {
                let m = ui.button(tl!("Mark Beats"));
                app.auto.add("autoMix.markBeats", m.rect, "Mark Beats");
                let c = ui.button(tl!("Cut Selected Clips to the Beat"));
                app.auto.add("autoMix.cutToBeat", c.rect, "Cut Selected Clips to the Beat");
                mark = m.clicked();
                cut = c.clicked();
            });
            ui.label(RichText::new(tl!("The music is the selected audio clip, else a Music clip, else the longest one without speech. The cuts between the selected clips move to the nearest beat; the edit keeps its length.")).color(t.text_dim).small());
            if !app.ui.auto_mix.message.is_empty() {
                ui.label(RichText::new(&app.ui.auto_mix.message).color(t.text_dim));
            }
            crate::dialog_style::actions(ui, |ui| {
                let c = ui.add(crate::dialog_style::secondary(tl!("Close")));
                app.auto.add("autoMix.close", c.rect, "Close");
                close = c.clicked();
            });
        });
    });
    let every = app.ui.auto_mix.every;
    let msg =
        if go {
            let u = &app.ui.auto_mix;
            let mut p = json!({"platform": u.platform});
            for (k, (id, _)) in STEPS.iter().enumerate() {
                p[*id] = json!(u.steps[k]);
            }
            Some(app.session.execute("audio.autoMix", p).map(|r| mix_message(&r)))
        } else if mark {
            Some(app.session.execute("audio.beatMarkers", json!({"every": every})).map(|r| {
                tlf!("{n} beat markers ({bpm} BPM)", n = r["markers"].as_u64().unwrap_or(0), bpm = format!("{:.0}", r["bpm"].as_f64().unwrap_or(0.0)))
            }))
        } else if cut {
            Some(
                app.session
                    .execute("sequence.cutToBeat", json!({"every": every}))
                    .map(|r| tlf!("Cuts on the beat: {n} of {all}", n = r["moved"].as_u64().unwrap_or(0), all = r["cuts"].as_u64().unwrap_or(0))),
            )
        } else {
            None
        };
    if let Some(m) = msg {
        let m = m.unwrap_or_else(|e| e.to_string());
        app.ui.status = m.clone();
        app.ui.auto_mix.message = m;
    }
    if close || crate::widgets::escape_closes(ctx) {
        app.ui.auto_mix.dialog = false;
    }
}
