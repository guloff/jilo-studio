# Captions

FilmCraft keeps captions on **caption tracks** in the sequence, shown in their own area above the
video tracks (as Premiere does). Caption tracks are saved in the `.fcproj` (project format
version 2; version 1 projects open with no caption tracks).

## Model

- **Caption track** (`CaptionTrack`, `crates/project/src/caption.rs`): name, format (`Subtitle`,
  `CEA-608`, `CEA-708`, `Teletext`), language, output eye (shown in the Program monitor and
  burned in on export), lock, sync lock, and a **track style**:
  font family and style (bundled or installed; a missing family falls back to Inter, the default is
  Inter SemiBold), font size (pixels at 1080 lines, scaled to the frame), capitals, text colour,
  background box, its colour and corner radius, outline width and colour, drop shadow (colour,
  offset, softness), alignment (left/centre/right), position (top/middle/bottom) and margin (a
  fraction of the frame height, default 0.08; `captions.setStyle` clamps it to 0–0.45), line
  spacing. Styles saved before fonts, capitals, corners and shadows load unchanged.
- **Caption**: in/out in exact `Tick`s, text (lines separated by `\n`; `<i>`, `<b>`, `<u>` kept
  as written), optional speaker, and the WebVTT cue id and cue settings, kept for round trips.
  Captions on a track never overlap.

## Files

| Format | Import | Export | Notes |
|---|---|---|---|
| SubRip `.srt` | ✓ | ✓ | Forgiving reader (BOM, UTF-16, Windows-1252, CRLF, missing indexes, `,`/`.` ms, missing hours). |
| WebVTT `.vtt` | ✓ | ✓ | Keeps cue ids, cue settings, `<v Speaker>`, STYLE/REGION/NOTE blocks. `line:N%` and `align:` are honoured when drawing. |
| Scenarist SCC `.scc` | ✓ | ✓ | CEA-608 at 29.97 fps. Reads pop-on, roll-up and paint-on; writes pop-on with drop-frame (default) or non-drop timecode. |

`File ▸ Import` (or dropping a file) recognises caption files by extension and content and adds a
new caption track to the open sequence (a sequence is created when none is open). Cue times are
snapped to the sequence's frames. `File ▸ Export ▸ Captions…` writes the chosen caption track in
the format given by the file extension (`.srt`, `.vtt`, `.scc`).

Details of each format and the SCC encoder are in [`crates/captions/README.md`](../crates/captions/README.md).

## Editing

| Command | Menu / shortcut | Params |
|---|---|---|
| `captions.newTrack` | Sequence ▸ Captions ▸ Add New Caption Track… (⌥⌘A) | `format`, `name`, `language` |
| `captions.add` | Sequence ▸ Captions ▸ Add Caption at Playhead (⌥⌘C) | `track`, `text`, `time`, `durationSeconds` (3) |
| `captions.next` / `captions.previous` | Sequence ▸ Captions ▸ Go to Next/Previous Caption Segment (⌥⌘↓/↑) | |
| `captions.showAll` / `captions.hideAll` | Sequence ▸ Captions | |
| `captions.split` | Text panel ✂ | `caption`, `time` (default: every caption under the playhead) |
| `captions.merge` | Text panel | `captions` (consecutive, one track) |
| `captions.setText` | Text panel text field | `caption`, `text`, `speaker` |
| `captions.setTimes` | Text panel in/out fields | `caption`, `start…`/`end…` (`Time` ticks, `Frame`, `Seconds`, `Timecode`) |
| `captions.trim` | drag a caption edge in the timeline | `caption`, `edge` (`in`/`out`), `delta` or `deltaFrames` |
| `captions.move` | drag a caption in the timeline | `captions`, `delta` or `deltaFrames` |
| `captions.delete` | Edit ▸ Clear / Ripple Delete with captions selected | `captions`, `ripple` |
| `captions.select`, `captions.goTo` | click / number button | `captions`, `add` / `caption` |
| `captions.setTrack` | caption track header (eye, lock) | `track`, `name`, `format`, `language`, `enabled`, `locked`, `syncLock` |
| `captions.setStyle` | Caption Style panel, Text panel style strip | `track` (id, `"C1"`, or `"all"` for every caption track in one undo step), `font`, `fontStyle`, `size`, `uppercase`, `color`, `background`, `backgroundColor`, `boxRadius`, `outline`, `outlineColor`, `shadow`, `shadowColor`, `shadowOffset` `[x, y]`, `shadowBlur`, `align`, `anchor`, `margin`, `lineSpacing`, `reset` |
| `captions.stylePresets` | (query) | the built-in style presets: `id`, `name`, `style` (`crates/engine/src/caption_presets.rs`) |
| `captions.applyStylePreset` | Caption Style panel tiles | `preset`, `track` (id, `"C1"` or `"all"`): the preset replaces the style |
| `captions.deleteTrack` | | `track` |
| `captions.import` / `captions.export` | File ▸ Export ▸ Captions… | `path`, `format`, `track`, `dropFrame` |
| `captions.list` | (query) | `track` — tracks, styles and captions with timecodes (a `track` that names no caption track is an error) |

Tracks are addressed by id or `"C1"`, `"C2"`… (top first). Edits are limited by neighbouring
captions and a one-frame minimum. Sync-locked caption tracks follow insert and extract edits on
the media tracks (a caption across an insert point is split). `Add Edit to All Tracks` (⇧⌘K)
also splits captions.

## Timeline and Text panel

- Caption rows: header with `C1…` badge, lock, output eye, name and format; caption blocks in
  violet showing their text. Click selects (⌘/⇧ adds), drag moves, edge drag trims, double-click
  opens the Text panel. Automation ids: `timeline.caption.<id>`,
  `timeline.captionTrack.C1[.locked|.enabled]`.
- **Text panel ▸ Captions**: search, track picker (also creates tracks of each format), add /
  split / merge / delete / export buttons, the list of segments with editable in/out timecodes
  and text (committed when the field loses focus), and the track style strip (size, colour, box,
  alignment, position). Automation ids: `text.tab.Captions`, `text.captions.add|split|merge|delete|export|search|track`,
  `text.captions.<id>.in|out|text|goto|row`, `text.captions.style.*`; its Styles… button opens the
  Caption Style panel.
- **Caption Style panel** (Graphics and Titles ▸ Caption Styles…, Window ▸ Caption Style): the track
  (or All caption tracks), a gallery of 26 presets — each tile a frame drawn by the burn-in renderer,
  so it looks as playback and export will — and every style field (font, style, size, ALL CAPS,
  colour, outline, box and corners, shadow, alignment, position, edge distance, line spacing,
  Reset Style). Automation ids: `captionStyle.track|all|reset|newTrack`,
  `captionStyle.preset.<id>`, `captionStyle.<field>`.

## Burn-in

The Program monitor shows the visible caption tracks. On export, `file.exportMedia`
`{"burnCaptions": true}` (Export mode: **Burn Captions Into Video**) draws them into every frame.
Captions are set in Inter SemiBold (OFL, bundled) by the text engine (`crates/text`: shaping,
kerning, bidi); italics/bold markup is not yet drawn differently. The captions of a nested sequence
are part of the nest's picture: they are drawn wherever the nest is shown, in the Program monitor
and in every export, whether or not the outer sequence shows or burns in its own.

## Translating captions

Text panel ▸ Captions ▸ Translate captions (`captions.translate {track, to}`) makes a new caption
track in another language with the text engine of Settings ▸ AI (Gemini or OpenAI): the captions go
in batches of 40 as `{"items": [{"id", "text"}]}`, translated for meaning in spoken style at about the
original length, ids and order kept, nothing merged or split. The new track sits under the source,
named "<language> (translated from <language>)", with the source's timing, speakers and style; a
caption the engine skipped keeps its text. Nothing is sent before the user's one-time consent for
the provider. The interface fetches the translation in the background (the status bar and the
Progress panel show it, with Cancel) and adds the track in one undo step (`background: true`, then
the same command again; see `filmcraft_engine::ai_tasks`).

