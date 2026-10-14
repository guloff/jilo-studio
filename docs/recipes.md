# Recipes

A recipe is a series of editing steps saved under a name and run again with one click — "clean a
talking head", "karaoke captions for shorts". A step is any command with its parameters, so a
recipe can do anything the menus, the panels, the CLI or an agent can. Code:
`crates/engine/src/recipes.rs`; window: `crates/ui-egui/src/panels/recipes.rs` (AI ▸ Recipes…).

## Running

`recipes.run {name}` (or `{name, steps}` for an edited, unsaved copy) runs the steps one at a time
on the selection and the open sequence, driven with the other background work, so the window stays
responsive. A step that starts a background job — recognition, a cloud feature, an export — is
waited for before the next one; `transcript.generate` is first recognised in the background
(`transcript.recognize`), as Batch Process does. Steps that are off are skipped. A failing step
stops the run and says which step failed. When the run ends, all its edits are one undo step,
"Recipe: <name>". `recipes.status`, `recipes.cancel`; `wait: true` blocks until the end (CLI).

## Recording

`recipes.recordStart`, then edit as usual; `recipes.recordStop {name}` saves the commands run at the
top level as the steps (without `name` it returns them). Left out: moving around and selecting
(playhead, selection, captions navigation), undo and redo, files, settings and the recipes
commands. The ids of the clips, items, captions and sequences a step was run on are dropped, so the
recipe acts on whatever is selected when it runs; caption tracks named `"C1"` or `"all"` stay.

## Storage

Saved recipes are `<data dir>/recipes/<name>.json`:

```json
{"format": "jilo-recipe", "version": 1, "name": "My Shorts", "description": "",
 "steps": [{"command": "transcript.removeFillers", "params": {}, "enabled": true}]}
```

The built-in recipes (Clean Talking Head, Karaoke Captions, Shorts Pack) are defined in code; a
saved recipe of the same name replaces one, and deleting it brings the built-in one back.
