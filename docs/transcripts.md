# Transcripts and text-based editing

FilmCraft's Text panel ▸ **Transcript** tab shows the dialogue of the open sequence as text. Select
words to mark In/Out, then extract or lift them; remove filler words and long pauses in one step;
turn the transcript into captions. Every action is an engine command (`transcript.*`), so the CLI,
the control channel and MCP agents can do the same.

## Model

- A **transcript** belongs to a media item (`Project::transcripts`, saved in the `.fcproj` since
  schema v9). It lists **words** with media-time bounds (`Tick`s), an optional speaker index and a
  confidence, plus the speaker names and the language. Because the times are media time, the
  transcript stays valid however the clip is trimmed, moved, sped up or reused.
- The **sequence transcript** is derived, never stored (`filmcraft_edit::transcript::sequence_words`):
  audio tracks are read top first; a word is heard through the first enabled clip whose range
  covers the word's midpoint (duplicates of the same dialogue on lower tracks read once); disabled,
  reversed and frame-hold clips contribute nothing.
- Speaker names come from the clip transcripts, so renaming "Speaker 1" in every transcript renames
  it across the sequence.

## Commands

| Command | What it does |
|---|---|
| `transcript.generate` | Transcribe media items (`items`, else the Project selection, else the media of the sequence's audio clips). Params: `model` (default `whisper-base`), `language` (`auto` = detect), `diarize`, `maxSpeakers`. One undo step. |
| `transcript.set` | Store a transcript you bring (JSON: `language`, `speakers`, `words` with `text`/`start`/`end`/`speaker`); it is sorted and made well formed. |
| `transcript.delete` | Remove transcripts. |
| `transcript.inspect` | The sequence transcript: words (index, text, sequence times, speaker, clip), paragraphs, speakers, the word at the playhead. |
| `transcript.search` | Word-index ranges matching a phrase (case and punctuation ignored; the last word may be a prefix). |
| `transcript.select` | Mark In/Out around words `from..=to` (frame-snapped outward) and move the playhead there. |
| `transcript.extract` / `transcript.lift` | Extract (ripple) or lift the words' frames on the targeted tracks. |
| `transcript.renameSpeaker` | Rename a speaker by name (every transcript) or by index in one `item`. |
| `transcript.removeFillers` | Ripple-delete filler words (`fillers`, default um/uh/erm/…; phrases such as "you know" allowed). |
| `transcript.removePauses` | Ripple-delete pauses longer than `minSeconds`, keeping `keepSeconds` of air on both sides. |
| `transcript.createCaptions` | Lay the words out as captions on a new caption track (`maxChars`, `lines`, `minSeconds`, `maxSeconds`, `gapFrames`). |
| `transcript.models` / `transcript.downloadModel` | List the speech models (size, licence, installed) / download one. |

## Speech recognition

Recognition goes through the `Transcriber` trait (`crates/speech`). The built-in recogniser is
OpenAI's Whisper, run in pure Rust on [candle](https://github.com/huggingface/candle) on the CPU,
with timestamp decoding, language detection and word times from cross-attention alignment (see the
`filmcraft_speech::whisper` module docs). Speakers are labelled by clustering per-chunk MFCC
statistics (`filmcraft_speech::diarize`); no model is involved.

Both are **optional features**, off by default and never built for the web:

- `whisper` (on `filmcraft-speech`, `filmcraft-engine`, and the `filmcraft` / `filmcraft-cli`
  apps, where it also enables downloads): candle inference.
- `download` (`speech-download` on the engine): HTTPS downloads with rustls + RustCrypto and the
  operating system's certificate verifier.

Without `whisper`, and with no recogniser installed, `transcript.generate` and Transcribe Sequence
are disabled, with "speech-to-text is not available in this build" as the reason (`describe`,
`command_list {"enabled_only": true}` and the menus show it); with Automatically transcribe clips
on, `file.import` reports the same reason as a `transcription: …` entry in its `errors`.
Without `speech-download`, `transcript.downloadModel` is disabled the same way. Transcripts can
still be imported with `transcript.set` and edited with every other command. Hosts and tests can
install any recogniser in `Session::transcriber`, which enables transcription in any build.

### Models

Weights are **never** bundled or committed. They are downloaded on request into
`<data dir>/models/<id>/` (see `filmcraft_engine::autosave::default_data_dir`), each file pinned to a
revision of OpenAI's Hugging Face repositories and checked against its SHA-256:

| Id | Languages | Licence |
|---|---|---|
| `whisper-tiny` | multilingual | MIT (OpenAI); HF conversion Apache-2.0 |
| `whisper-base` (default) | multilingual | MIT (OpenAI); HF conversion Apache-2.0 |
| `whisper-small` | multilingual | MIT (OpenAI); HF conversion Apache-2.0 |
| `whisper-large-v3-turbo` | multilingual, 128 mel bands, the best for Russian and Tajik (1.6 GB) | MIT (OpenAI) |

Measured on 2026-10-10 (Apple M4 Pro, Metal): `whisper-large-v3-turbo` recognised English and
Russian test sentences (macOS speech synthesis) with 0 % WER and word times, about 4× faster than
real time.

### Cloud engines

Settings ▸ AI (or `engine` on `transcript.generate` / `transcript.recognize`) can choose Google
Gemini, OpenAI or ElevenLabs (Scribe, `scribe_v1`: a time for every word and speaker labels) instead
of Whisper (`filmcraft_engine::recognition`). Nothing is sent before the
user agreed for that provider (the Text panel asks once; agents pass `confirm: true`); the panel says
how much audio would go out. The recording goes window by window as 16-bit mono 16 kHz WAV
(Gemini: 2 minutes, OpenAI `whisper-1`: 10 minutes) with 3 s of overlap, 3 windows at a time; every
answer is checked against the sound (cut off, speech left without words, cues that do not align)
and recognised again in halves when incomplete (`filmcraft_speech::chunked`). The model's cue text
is the truth: its words take times from the model's word times (`filmcraft_speech::cues`), cues are
kept to 84 characters / 7 s, and the result becomes an ordinary transcript (`source`
`gemini:<model>` or `openai:<model>`), with word bounds tightened by level and speakers from the
model's labels or FilmCraft's own labelling. `transcript.recognize` runs it as a background job
(progress, Stop) and keeps the transcript until `transcript.generate` with the same settings adds it
as one undo step; the report gives the windows, retries, uncovered seconds and seconds sent.

### Providers

Settings ▸ AI keeps one API key per provider (`<data dir>/ai.json`, owner-only, or the environment)
and the engine of each kind of work:

| Provider | Key (environment) | Speech | Texts | Images | Voices |
|---|---|---|---|---|---|
| Google Gemini | `GEMINI_API_KEY`, `GOOGLE_API_KEY` | yes | yes | `gemini-2.5-flash-image` | |
| OpenAI | `OPENAI_API_KEY` | `whisper-1` | yes | `gpt-image-1` | |
| Anthropic Claude | `ANTHROPIC_API_KEY` | | `claude-opus-5-5` (structured outputs; refusals answered by a fallback model) | | |
| ElevenLabs | `ELEVENLABS_API_KEY`, `XI_API_KEY` | `scribe_v1` | | | `eleven_multilingual_v2` |

"Texts" are caption translation, highlights, YouTube texts and zooms by meaning; every model can be
changed in Settings ▸ AI. Each provider is asked about once before anything is sent to it.

### Generated images

AI ▸ Generate Image… (`ai.generateImage`) draws a picture from a description with the image
engine of Settings ▸ AI (OpenAI or Gemini), in the sequence's shape (or `shape`), saves it with the
project's media (Scratch Disks ▸ Captured, else next to the project), imports it as a still and,
with `place`, puts it on the timeline at the playhead on the first free video track (a new one when
none is free), scaled to the frame. It runs as a cloud task: consent, a job with progress and Stop,
undoable.

### ElevenLabs narration voices

With an ElevenLabs key, the Text to Speech panel offers the account's voices (Load ElevenLabs
voices…; `tts.cloudVoices`, then `tts.voices` lists them under `cloud` as `elevenlabs:<voice id>`).
They speak any language with the voice model of Settings ▸ AI ▸ Voices (`eleven_multilingual_v2`).
Each piece of speech between `[pause …]` markers is spoken on its own and the pauses are kept as
silence; vocal pitch and pace apply only to the voices on this computer. The panel asks once before
the narration's text goes to ElevenLabs (`confirm` on `tts.preview`, `tts.create`, `tts.edit`,
`tts.render`).

### Testing

Unit and engine tests use a fake `Transcriber` (`FixedTranscriber`), so CI needs no model. The
end-to-end test `crates/speech/tests/whisper_model.rs` (feature `whisper`) runs only when weights are
in `target/models/<id>/` (or `$FILMCRAFT_MODELS_DIR`) and speech samples (mono 16 kHz f32 with a
reference `.txt`) are in `target/fixtures/speech/`; it reports the word error rate and otherwise
prints SKIPPED. Measured on 2026-10-01: `whisper-tiny`, English, 12.2 % WER over 797 words of
the local speech samples (LibriSpeech read speech and dialogue clips) (about 9.5 minutes for the run in a release build on an
Apple-silicon laptop CPU).

## Filler words

Remove filler words (Text panel ▸ Transcript; `transcript.removeFillers`) knows Russian, Tajik and
English (`filmcraft_edit::transcript::filler_words`, from Jilo Studio): hesitations ("э", "мм", "uh")
are safe to cut, discourse words ("ну", "типа", "яъне", "like", "you know") can carry meaning. The
language is the transcript's (most of its words), or `language`. `transcript.fillers` lists what
occurs with counts; the panel shows it with the hesitations chosen and the discourse words not, and
removes what stays chosen in one undo step. Without `fillers`, the command removes the hesitations.

## Pauses by sound

For footage without a transcript, AI ▸ Speech and Captions ▸ Remove Silent Pauses (`audio.removeSilences`;
`audio.findSilences` only lists them) finds the pauses in the sequence mix by level: 10 ms RMS
frames below `thresholdDb` (−35 dBFS by default, −70…−10) for at least `minSeconds` (0.6 s, 0.2…5),
measured in 10-second blocks with FilmCraft's own detector (`filmcraft_audio_dsp::silence`), no
ffmpeg. Each pause keeps `keepSeconds` (0.15 s) at both ends, is snapped to frames and ripple-deleted,
all in one undo step; a sequence that is silent everywhere is left alone.

## Highlights

AI ▸ Automatic Editing ▸ Find Highlights… (also the Transcript toolbar; `highlights.find`) looks through the
sequence transcript for the best fragments with the text engine of Settings ▸ AI, as Jilo Studio
does: `viral` (a hook that works on its own as a short), `meaningful` (complete thoughts) or `facts`
(numbers, names, definitions), `count` 1–20 (5), `minSeconds` 5–180 (15), `maxSeconds` up to 600
(60). The transcript goes as cues (`[start-end] text`, a new cue after a 0.7 s pause, at a phrase end
past 15 characters, at most 7 s), titles and reasons come back in the transcript's language. The
answer is checked in code: best score first, boundaries snapped to whole cues (±0.5 s), cut to
`maxSeconds` at a cue end, and reversed, out-of-transcript, too short or overlapping fragments
dropped. Each highlight becomes a range marker (name = title, comment = score and reason; orange,
blue or green by mode) in one undo step. Nothing is sent before the user's one-time consent for the
provider; the interface searches in the background (status bar and Progress panel, with Cancel).

The results window lists the highlights (click a title to go there) and assembles the chosen ones
with `highlights.assemble {markers, as}`: `subsequences` (one sequence per highlight, in the
sequence's bin), `sequence` (all of them end to end) or `queue` (one export queue item per highlight,
named after it, next to the project unless `path` is given). Each is one undo step.

## Auto zooms

AI ▸ Automatic Editing ▸ Auto Zoom… (also the Transcript toolbar; `clip.autoZoom`) punches the picture in on stressed
phrases, as Jilo Studio does, with Scale keyframes on the clips' Motion effect (the selected video
clips, else V1's). The phrases come from the sequence transcript: `loudness` (the default, offline)
takes cues of at least 0.4 s louder than the median cue ×1.02 in the sequence mix, loudest first;
`meaning` asks the text engine of Settings ▸ AI for stressed phrases with a verbatim 2–8-word
quote and places each on the quoted words (by its first three words if the tail changed), so two
stresses of one long cue stay apart. A zoom lasts 0.8–4 s; `minGap` 2–60 s (6) keeps them apart and
caps the count at the sequence length divided by it. `intensity` 1.02–1.6 (1.15) multiplies the
clip's own scale; `style` `cut` holds it for the phrase, `smooth` eases in and out over up to 0.35 s.
The clips' Scale keyframes are replaced in one undo step; the cloud is used only with `meaning`,
after the one-time consent, in the background like the other cloud tasks.

## YouTube texts and chapters

AI ▸ YouTube Title, Description and Chapters… (also the Transcript toolbar;
`youtube.generate`) writes, as Jilo Studio does, three title options (≤ 100 characters), a description
(≤ 5000), tags (the field ≤ 500, commas and quotes counted), three hashtags and chapters from the
sequence transcript, sent in blocks of at least 8 s (`[second] text`) to the text engine of
Settings ▸ AI, in the transcript's language or `language`. Angle brackets are removed (YouTube refuses
them). The chapters are checked in code: each starts on a cue (one closer than 10 s to the previous
moves on to the first cue 10 s later), the first at 0:00, none leaving less than 10 s to the end.
They become the sequence's chapter markers (replacing those there unless `replaceChapters` is
false), so they follow the edit, go into exported MP4/MOV files as a chapter track and can be moved
by hand. The texts are kept with the sequence (`youtube.texts` reads them, `youtube.set` edits them
with the same checks, one undo step). The description assembled for YouTube adds the chapters of the
current chapter markers when YouTube will take them (at least 3, each ≥ 10 s, a video of ≥ 30 s) and
the hashtags; past the limit the text is shortened, not the chapters. The window copies each field
or everything at once.

## Batch processing

AI ▸ Batch Process… (`batch.start`) runs the same steps over the sequences and clips selected in
the Project panel (up to 50; a clip becomes a sequence, as New Sequence From Clip), one after
another, as Jilo Studio's batches do: `transcribe` (only media without a transcript, with the batch's
engine — local Whisper by default — and language), `fillers`, `pauses`, `autoZoom` (by loudness),
`highlights`, `youtube`, `export` (an export queue item per sequence with the chosen preset, the
queue started at the end). The engines are checked when the batch starts (keys, network and the
consent given with `confirm`), and the dialog says what goes to the cloud before Start. The batch is
one job in the status bar and the Progress panel (Cancel stops it after the current step); a failed
step is noted and the item goes on, one item's failure never stops the others, and a summary lists
every item (`batch.status`). Each step is the same command as by hand, so every change is an undo
step, and the editor keeps its sequence while the batch works.

## Reframe to faces

AI ▸ Automatic Editing ▸ Reframe to Face… (`clip.reframeToFace`) frames the selected video clips on a face, as Jilo
Studio frames a talk for a vertical screen. The faces are found on this computer with Apple Vision
(nothing is sent; other systems say face detection is not available) on frames two a second (one a
second past ten minutes, 3000 at most), each frame searched in nine overlapping tiles so a small
face on a wide shot is found too. The analysis is a job in the status bar and the Progress panel and
is kept for the session, so framing the same clip again is instant. `target`: `speaker` (default) —
the face that stays near itself over the first frames with faces, not someone passing by; `group` —
everyone steady in frame, framed by the group's middle; `face` — the face nearest `x`, `y` (fractions
of the picture) at the playhead (or `seconds`), followed both ways; the dialog shows the frame at the
playhead with its faces boxed (`clip.facesAt`) and a click picks one. A face may move 6 % of the width
between frames; farther, the frame holds. The track is smoothed over `smoothing` seconds (1.5, 0–6)
with a dead zone, so a nod moves nothing. The clip gets Motion keyframes in one undo step: Scale to
fill the sequence frame (× `zoom` 100–400 %), and Position keys, thinned to the turns of the move,
that put the face at `anchorX`, `anchorY` of the frame (0.5, 0.4) without ever showing an edge. Scale
keys a clip already has (Auto Zoom) keep their zooms relative to the new scale.

## Limits

- Transcription runs synchronously inside the command (no background job or progress bar yet).
- Track items that refer to a subclip are looked up by the subclip's id, so a transcript made for
  the parent media is not shown through subclip clips yet.

## Retakes and stumbles

AI ▸ Speech and Captions ▸ Remove Retakes and Stumbles… lists the failed takes the transcript
shows (`transcript.findRetakes`, `filmcraft_edit::transcript::find_retakes`, on this computer):
a phrase of three words or more said again within 20 seconds (`maxSeconds`) — the earlier attempt,
up to where it restarts, is cut and the last take stays; two words said again right away (a false
start); a word said twice in a row (a stutter). Matching ignores case and punctuation and works in
any language. Each one has a checkbox; Remove cuts the checked ones in one undo step
(`transcript.removeRetakes {ranges}`; without `ranges`, every one found — the Clean Talking Head
recipe does that).
