//! The AI panel: every AI feature as a card, by what it is for — what it does in a line, whether
//! it can run now (and if not, what it needs: a transcript, a key, a model, a clip), "Do It" with
//! the usual settings where one click is enough, and "Customize…" for the feature's dialog. The
//! commands are the same as the AI menu's.
//!
//! Under a feature, its presets (`ai.presets.*`): the ones that come with the app and the user's,
//! each run with one click; "Save as Preset…" keeps the settings the feature last ran with.
//!
//! Automation ids: `aiTools.card.<command>`, `aiTools.do.<command>`, `aiTools.open.<command>`,
//! `aiTools.preset.<preset id>`, `aiTools.savePreset.<command>`, `aiTools.presetName`,
//! `aiTools.presetSave`.

use egui::{RichText, Sense, vec2};
use serde_json::{Value, json};

use crate::JiloApp;

/// The dot of a feature that can run now, and of one that needs something first.
const READY: egui::Color32 = egui::Color32::from_rgb(72, 187, 120);
const WAITING: egui::Color32 = egui::Color32::from_rgb(230, 162, 60);

/// What a feature needs before it can run.
#[derive(Clone, Copy, PartialEq)]
enum Need {
    Sequence,
    Transcript,
    Speech,
    TextKey,
    ImageKey,
    Captions,
    Multicam,
    Selection,
}

/// A card: (command, title, what it does, needs, one-click parameters).
struct Card {
    command: &'static str,
    title: &'static str,
    about: &'static str,
    needs: &'static [Need],
    quick: Option<fn() -> Value>,
}

const fn card(command: &'static str, title: &'static str, about: &'static str, needs: &'static [Need], quick: Option<fn() -> Value>) -> Card {
    Card { command, title, about, needs, quick }
}

fn captions_two_lines() -> Value {
    json!({"lines": 2})
}

fn no_params() -> Value {
    json!({})
}

fn mix_youtube() -> Value {
    json!({"platform": "youtube"})
}

use Need::*;

/// The cards in groups, in the order the panel shows them.
const GROUPS: [(&str, &[Card]); 5] = [
    (
        "Speech and Captions",
        &[
            card(
                "ai.editByText",
                "Edit by Text",
                "Delete words in the transcript and the picture and sound are cut with them.",
                &[Transcript],
                Some(no_params),
            ),
            card("transcript.generate", "Transcribe", "Turn speech into text with word timing, on this computer or in the cloud.", &[Sequence, Speech], None),
            card(
                "transcript.createCaptions",
                "Captions from the Transcript",
                "Captions in short readable lines, timed to the words.",
                &[Transcript],
                Some(captions_two_lines),
            ),
            card("captions.translate", "Translate Captions", "A caption track in another language.", &[Captions, TextKey], None),
            card("ai.dub", "Dub", "The speech in another language: translated captions spoken by a voice.", &[Captions, TextKey], None),
            card("transcript.removePauses", "Remove Pauses", "Cut the pauses between phrases, as long as you choose.", &[Transcript], None),
            card("audio.removeSilences", "Remove Silences", "Cut the quiet stretches by the sound alone, no transcript needed.", &[Sequence], None),
            card("transcript.removeFillers", "Remove Filler Words", "Find the ums and ahs and cut the ones you pick.", &[Transcript], None),
            card("transcript.removeRetakes", "Remove Retakes and Stumbles", "Keep the last take of repeated phrases.", &[Transcript], None),
        ],
    ),
    (
        "Automatic Editing",
        &[
            card(
                "ai.cutIntoShorts",
                "Cut into Shorts",
                "The best moments as vertical shorts with captions, the speaker in frame.",
                &[Transcript, TextKey],
                None,
            ),
            card("highlights.find", "Find Highlights", "The moments worth a short, marked on the timeline.", &[Transcript, TextKey], None),
            card("sequence.makeVertical", "Make Vertical", "A 9:16 copy of the sequence that follows the speaker's face.", &[Sequence], Some(no_params)),
            card("clip.autoZoom", "Auto Zoom", "Punch in on the louder or more important phrases.", &[Sequence], None),
            card("clip.reframeToFace", "Reframe to Face", "Keep the speaker in frame in a vertical or square sequence.", &[Selection], None),
            card("clip.removeBackground", "Remove Background", "The person or subject cut out of the video, on this computer.", &[Selection], Some(no_params)),
            card("multicam.autoSwitch", "Auto Multi-Camera", "Switch to whoever is speaking.", &[Multicam], None),
            card("broll.auto", "Auto B-roll", "Footage that fits what is said, above the phrase.", &[Transcript], None),
            card("stickers.byWords", "Emoji by Words", "An emoji on the phrases that mention one.", &[Transcript], None),
            card("export.allFormats", "Export in All Formats", "16:9, 9:16, 1:1 and 4:5 copies, exported together.", &[Sequence], None),
        ],
    ),
    (
        "Sound",
        &[
            card("audio.autoMix", "Auto Mix", "Speech even, music under the voice, the mix at YouTube's loudness.", &[Sequence], Some(mix_youtube)),
            card("ai.textToSpeech", "Text to Speech", "A voice reads your script.", &[Sequence], None),
        ],
    ),
    (
        "Brand and Publishing",
        &[
            card("graphics.brandKit", "Brand Kit", "Logo, colours, fonts and captions in one click.", &[Sequence], None),
            card("youtube.generate", "YouTube Title and Chapters", "Title, description and chapters from what is said.", &[Transcript, TextKey], None),
            card("ai.thumbnail", "YouTube Thumbnail", "The frame at the playhead with a big title, 1280×720.", &[Sequence], None),
            card("ai.generateImage", "Generate Image", "A picture from a description.", &[ImageKey], None),
        ],
    ),
    (
        "Automation",
        &[
            card("ai.assistant", "AI Assistant", "Say what you want; it plans the steps and you approve them.", &[TextKey], None),
            card("ai.recipes", "Recipes", "Record a series of steps and run it again with one click.", &[], None),
            card("batch.start", "Batch Process", "The same steps on many clips, and the inbox folder.", &[], None),
        ],
    ),
];

/// What `need` is missing, if anything.
fn missing(app: &JiloApp, need: Need) -> Option<String> {
    let s = &app.session;
    let has_key = |p: jilo_ai::Provider| jilo_engine::ai::key(s, p).is_some();
    match need {
        Sequence => s.active_sequence().is_none().then(|| tl!("Open a sequence").to_string()),
        Transcript => jilo_engine::transcript::sequence_words(s).is_empty().then(|| tl!("Transcribe first").to_string()),
        Speech => match s.prefs.ai.speech_provider() {
            Some(p) if s.prefs.ai.local_only => Some(tlf!("{provider} is off: only on this computer", provider = p.name())),
            Some(p) => (!has_key(p)).then(|| tlf!("Needs a {provider} key", provider = p.name())),
            None => (!jilo_engine::transcript::speech_available()).then(|| tl!("Not in this build").to_string()),
        },
        TextKey => {
            let p = s.prefs.ai.text_provider();
            if s.prefs.ai.local_only {
                Some(tl!("Cloud only: turned off").to_string())
            } else {
                (!has_key(p)).then(|| tlf!("Needs a {provider} key", provider = p.name()))
            }
        }
        ImageKey => {
            let p = s.prefs.ai.image_provider();
            if s.prefs.ai.local_only {
                Some(tl!("Cloud only: turned off").to_string())
            } else if p == jilo_ai::Provider::Custom {
                // a server on this computer may need no key; it does need its address
                jilo_ai::images::custom_base(&s.prefs.ai.custom_image_url).is_err().then(|| tl!("Set the custom service's address").to_string())
            } else {
                (!has_key(p)).then(|| tlf!("Needs a {provider} key", provider = p.name()))
            }
        }
        Captions => s.active_sequence().is_none_or(|q| q.caption_tracks.iter().all(|t| t.captions.is_empty())).then(|| tl!("Make captions first").to_string()),
        Multicam => s
            .active_sequence()
            .is_none_or(|q| !q.video_tracks.iter().flat_map(|t| t.items.iter()).any(|i| i.multicam.is_some_and(|m| m.enabled)))
            .then(|| tl!("Needs a multi-camera clip").to_string()),
        Selection => s.state.selection.is_empty().then(|| tl!("Select clips").to_string()),
    }
}

/// The preset being named: (command, name typed so far).
fn naming_id() -> egui::Id {
    egui::Id::new("aiTools.naming")
}

pub fn show(app: &mut JiloApp, ui: &mut egui::Ui, rect: egui::Rect) {
    let t = app.tokens;
    let mut run: Option<(&'static str, Option<Value>)> = None;
    // read directly, not through a command: this runs every frame
    let all = jilo_engine::ai_presets::all(&app.session);
    let last: std::collections::HashSet<String> = app.session.ai_last_params.keys().cloned().collect();
    let mut naming: Option<(String, String)> = ui.data(|d| d.get_temp(naming_id()));
    let mut save: Option<(String, String)> = None;
    let mut delete: Option<String> = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(8.0)));
    egui::ScrollArea::vertical().id_salt("ai-tools").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let w = ui.available_width();
        for (group, cards) in GROUPS {
            ui.add_space(4.0);
            ui.label(RichText::new(crate::i18n::t(group)).strong().color(t.text_dim).size(11.5));
            for c in cards {
                let why: Vec<String> = c.needs.iter().filter_map(|n| missing(app, *n)).collect();
                let ready = why.is_empty();
                let frame = egui::Frame::new()
                    .fill(t.field_bg)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .stroke(egui::Stroke::new(1.0, t.field_border));
                let r = frame.show(ui, |ui| {
                    ui.set_width(w - 22.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::i18n::t(c.title)).strong());
                        let (dot, text) = if ready { (READY, tl!("Ready").to_string()) } else { (WAITING, why[0].clone()) };
                        let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, dot);
                        ui.label(RichText::new(text).size(11.0).color(t.text_dim));
                    });
                    ui.add(egui::Label::new(RichText::new(crate::i18n::t(c.about)).size(11.5).color(t.text_dim)).wrap());
                    ui.horizontal(|ui| {
                        if let Some(q) = c.quick {
                            let b = ui.add_enabled(ready, egui::Button::new(tl!("Do It")).small());
                            app.auto.add(&format!("aiTools.do.{}", c.command), b.rect, c.title);
                            if b.clicked() {
                                run = Some((c.command, Some(q())));
                            }
                        }
                        let label = if c.quick.is_some() { tl!("Customize…") } else { tl!("Open…") };
                        let b = ui.add(egui::Button::new(label).small());
                        app.auto.add(&format!("aiTools.open.{}", c.command), b.rect, c.title);
                        if b.clicked() {
                            run = Some((c.command, None));
                        }
                    });
                    // presets: one click runs the feature with them; right-click deletes one of yours
                    let mine: Vec<&jilo_engine::ai_presets::AiPreset> = all.iter().filter(|p| p.command == c.command).collect();
                    let last = last.contains(c.command);
                    if !mine.is_empty() || last {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                            ui.label(RichText::new(tl!("Presets:")).size(11.0).color(t.text_dim));
                            for p in &mine {
                                let id = p.id.clone();
                                let builtin = id.starts_with("builtin:");
                                let name = p.name.as_str();
                                let text = if builtin { crate::i18n::t(name).to_string() } else { name.to_string() };
                                let b = ui.add_enabled(ready, egui::Button::new(RichText::new(text).size(11.0)).small().corner_radius(10.0));
                                app.auto.add(&format!("aiTools.preset.{id}"), b.rect, name);
                                if b.clicked() {
                                    run = Some((c.command, Some(p.params.clone())));
                                }
                                if !builtin {
                                    b.context_menu(|ui| {
                                        if ui.button(tl!("Delete Preset")).clicked() {
                                            delete = Some(id.clone());
                                            ui.close();
                                        }
                                    });
                                }
                            }
                            if last {
                                let b = ui.add(egui::Button::new(RichText::new(tl!("Save as Preset…")).size(11.0)).small());
                                app.auto.add(&format!("aiTools.savePreset.{}", c.command), b.rect, "Save as Preset");
                                b.clone().on_hover_text(tl!("Keep the settings this feature last ran with, under a name."));
                                if b.clicked() {
                                    naming = Some((c.command.to_string(), String::new()));
                                }
                            }
                        });
                    }
                    if let Some((cmd, name)) = naming.as_mut().filter(|(cmd, _)| cmd == c.command) {
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(name).hint_text(tl!("Preset name")).desired_width(170.0));
                            app.auto.add("aiTools.presetName", r.rect, "Preset name");
                            let ok = !name.trim().is_empty();
                            let b = ui.add_enabled(ok, egui::Button::new(tl!("Save")).small());
                            app.auto.add("aiTools.presetSave", b.rect, "Save");
                            if b.clicked() || (ok && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                save = Some((cmd.clone(), name.trim().to_string()));
                            }
                            if ui.add(egui::Button::new(tl!("Cancel")).small()).clicked() {
                                save = Some((String::new(), String::new()));
                            }
                        });
                    }
                });
                app.auto.add(&format!("aiTools.card.{}", c.command), r.response.rect, c.title);
            }
        }
        ui.add_space(6.0);
        let b = ui.add(egui::Button::new(tl!("AI Settings…")).small());
        app.auto.add("aiTools.settings", b.rect, "AI Settings");
        if b.clicked() {
            run = Some(("ai.settings", None));
        }
    });
    if let Some((cmd, name)) = save {
        naming = None;
        if !cmd.is_empty() {
            app.ui.status = match app.session.execute("ai.presets.save", json!({"command": cmd, "name": name})) {
                Ok(_) => tlf!("Preset “{name}” saved.", name),
                Err(e) => e.to_string(),
            };
        }
    }
    if let Some(id) = delete {
        app.ui.status = match app.session.execute("ai.presets.delete", json!({"id": id})) {
            Ok(_) => tl!("Preset deleted.").to_string(),
            Err(e) => e.to_string(),
        };
    }
    ui.data_mut(|d| match &naming {
        Some(n) => {
            d.insert_temp(naming_id(), n.clone());
        }
        None => d.remove::<(String, String)>(naming_id()),
    });
    if let Some((cmd, params)) = run {
        let ctx = ui.ctx().clone();
        let r = crate::menus::invoke(app, &ctx, cmd, params.clone().unwrap_or_else(|| json!({})));
        match (r, params) {
            // one click: say what happened
            (Ok(v), Some(_)) => app.ui.status = summary(cmd, &v),
            (Ok(_), None) => {}
            // a cloud feature asks first, then runs in the background as from its dialog
            (Err(e), Some(p)) if e.contains("CONFIRM_CLOUD") => {
                let title = GROUPS.iter().flat_map(|(_, cards)| cards.iter()).find(|c| c.command == cmd).map_or(cmd, |c| c.title);
                crate::panels::ai_tasks::start(app, cmd, p, crate::i18n::t(title), tl!("what this feature works on"));
            }
            (Err(e), _) => app.ui.status = e,
        }
    }
}

fn summary(cmd: &str, v: &Value) -> String {
    match cmd {
        "audio.autoMix" => match v["gainDb"].as_f64() {
            Some(g) => tlf!("Mixed: {gain} dB on the Mix fader", gain = format!("{g:+.1}")),
            None => tl!("Mixed.").to_string(),
        },
        "transcript.createCaptions" => tl!("Captions made from the transcript.").to_string(),
        _ => tl!("Done.").to_string(),
    }
}
