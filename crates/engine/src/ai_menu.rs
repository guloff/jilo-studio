//! The AI menu: every feature built on a neural network or a cloud model in one top-level menu.
//! The commands declare their place with the paths below and keep their ids (the control channel,
//! the CLI and MCP don't change); [`AI_MENU`] is their order. The frontend adds its own items after
//! these (Text to Speech…, AI Settings…).

use crate::commands::CommandSpec;

/// AI ▸ Speech and Captions: recognition, captions from it, translation, pauses and fillers.
pub(crate) const SPEECH: &[&str] = &["AI", "Speech and Captions"];
/// AI ▸ Automatic Editing: highlights, zooms, reframing, scene cuts.
pub(crate) const EDITING: &[&str] = &["AI", "Automatic Editing"];
/// AI ▸ Automatic Editing ▸ Remix.
pub(crate) const REMIX: &[&str] = &["AI", "Automatic Editing", "Remix"];
/// AI ▸ Voice and Sound: the mix on autopilot.
pub(crate) const SOUND: &[&str] = &["AI", "Voice and Sound"];
/// The AI menu itself.
pub(crate) const TOP: &[&str] = &["AI"];

/// The AI menu, top to bottom.
pub const AI_MENU: [&str; 22] = [
    "sequence.transcribe",
    "transcript.generate",
    "transcript.createCaptions",
    "captions.translate",
    "transcript.removePauses",
    "audio.removeSilences",
    "transcript.removeFillers",
    "transcript.removeRetakes",
    "transcript.delete",
    "highlights.find",
    "clip.autoZoom",
    "clip.reframeToFace",
    "multicam.autoSwitch",
    "broll.auto",
    "clip.sceneEditDetection",
    "clip.remix.enable",
    "clip.remix.properties",
    "clip.remix.revert",
    "audio.autoMix",
    "ai.generateImage",
    "youtube.generate",
    "batch.start",
];

/// Put the AI commands at the end of the registry in [`AI_MENU`] order (menus are built in
/// registry order).
pub(crate) fn apply_layout(v: &mut Vec<CommandSpec>) {
    for id in AI_MENU {
        if let Some(i) = v.iter().position(|c| c.id == id) {
            let c = v.remove(i);
            v.push(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ai_menu_lists_every_ai_command_in_order() {
        let in_ai: Vec<&str> = crate::command_specs().iter().filter(|c| c.menu.first() == Some(&"AI")).map(|c| c.id).collect();
        assert_eq!(in_ai, AI_MENU);
    }
}
