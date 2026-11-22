# Architecture

Cadence is a **single native Rust crate** — `cadence-studio` — producing a library (`cadence_studio`) and one binary (`cadence`). There is no Electron, no web runtime, and no backend: the interface is `eframe`/`egui`, audio is `CPAL`, MIDI is `midly`, files are `serde` JSON, and dialogs are native `rfd` pickers. Once built, the app works fully offline.

```
┌─ src/main.rs ─────────────────────────────────────────────────┐
│ eframe window (title "Cadence", app icon) · CLI modes:        │
│   song.cadence · --render-demo/--render/--check ·             │
│   --new-pack/--validate-pack/--install-pack/--list-instruments│
└───────────────▲────────────────────────────────────────────────┘
                │ creates
┌───────────────┴────────────────────────────────────────────────┐
│ src/gui/mod.rs + gui/** — eframe::App implementation           │
│   CadenceApp: menu bar + status/toasts (shell), transport bar, │
│   sidebar library/inspector, arranger (clips/automation/       │
│   audio lanes), drum step grid + piano roll + keyboard, mixer, │
│   dialogs, theme, undo/redo, dirty-close guard, autosave       │
└───────┬───────────────────────────────┬────────────────────────┘
        │ edits / reads                  │ rfd dialogs, pack files
        ▼                                ▼
┌─ src/model.rs ──────────┐   ┌─ src/persistence.rs ────────────┐
│ Project / Track /       │   │ load_project · save_project     │
│ Pattern / Clip / Note / │   │ (atomic temp+rename) ·          │
│ AudioClip / Bus /       │   │ save_recovery / load_recovery   │
│ Instrument (7 built-in  │   │ (.cadence = serde JSON,         │
│  + packs/font/sample) · │   │  app-data dir, 20 MB cap) ·     │
│ preset · SongEvent ·    │   │ audio store for imported WAVs   │
│ validate · events()     │   │                                  │
└───────┬─────────────────┘   └──────────────────────────────────┘
        │ &Project snapshot                 ▲ preset JSON / .sf2
        ▼                                   │ catalog / audio store
┌─ src/audio/** ────────────────────────────┴───────────────────┐
│ AudioEngine — owns the live CPAL stream                        │
│   play / pause / stop / set_project / preview / set_metronome  │
│   record: computer-keyboard/pad notes quantized into patterns  │
│   status() → TransportStatus (playing, step, peaks, voices)    │
│ Shared Renderer: built-in + pack-preset + .sf2 + WAV-sample    │
│ voices, per-track drive/filter/fader/pan, 4 subgroup buses,    │
│ track automation + live arpeggiator + glide, reverb +          │
│ ping-pong delay sends, swing, master limiter — on the callback │
│ export_wav() / export_stems(): same DSP offline → 16-bit WAV   │
└───────────────────────────────────────────────────────────────┘
        ▲                                   ▲
        │ note data                          │ .mid bytes
┌───────┴───────────┐               ┌───────┴────────┐
│ src/midi.rs       │               │ midly crate   │
│ export_midi() →   │               │ import/export │
│ Vec<u8>           │               │               │
│ import_midi() →   │               │               │
│ Project           │               │               │
└───────────────────┘               └────────────────┘

┌─ src/packs.rs (used by model + gui + audio voice) ─────────────┐
│ JSON instrument packs: validate · catalog · install · template │
│ embedded packs + user dir; presets embedded into Track.preset  │
│ optional embedded PCM `sample` blocks (≤2 MiB decoded)         │
└───────────────────────────────────────────────────────────────┘

┌─ src/font.rs + src/audio/font.rs ──────────────────────────────┐
│ SoundFont (.sf2) instruments: user font dir + $CADENCE_FONTS_  │
│ DIR, GM program names/catalog, SF2 parsing and voice           │
└───────────────────────────────────────────────────────────────┘
```

## Key design decisions

**One data model, three consumers.** `Project` (in `src/model.rs`) is the single source of truth. The GUI edits it, the audio engine plays and exports it via `Project::events()`, and persistence/MIDI serialize it. Musical positions are `f64` **sixteenth-note steps** (16 per bar); the UI snaps edits to whole steps while MIDI import may produce fractional positions.

**Extensible instruments without plugins.** `src/packs.rs` defines data-driven JSON synth presets (`InstrumentPreset`: oscillators, ADSR envelope, pitch/filter envelopes, LFO, filter, bit depth, sample-and-hold, optional drum `kit` or embedded PCM `sample`). The app ships thirteen embedded banks (196 presets — `8-bit`, seven `psy-*` banks, `drum-kits`, `guitar`, `neon`, `one-shots`, `mallets`) plus user-installed ones; the catalog re-scans the pack folder about every 2 s and a user pack with the same id replaces the embedded pack wholesale. `src/font.rs` adds `.sf2` SoundFont instruments (128 GM programs per installed font), and `File → Import Sample…` wraps a WAV as a `sample.*` preset. A track stores a full copy of its preset (`Track::preset`) or its font reference (`Track::font`), so songs play identically wherever the source is installed — `instrument` still picks the editor (drum grid vs. piano roll) while the preset supplies the sound.

**Audio never blocks the UI.** `AudioEngine::new_with_device()` opens a CPAL stream on the preferred output device (host default otherwise; the pick is remembered in the data dir and listed in the status bar's audio indicator, which doubles as the output picker). Control methods (`play`, `pause`, `stop`, `set_project`, `preview`, `set_metronome`, `set_output_device`) are non-blocking and hand the engine a snapshot of the project. All synthesis, mixing, effects, swing scheduling, and the master limiter run on the real-time callback — no file I/O, no unbounded locks, no per-sample allocation. If no audio device exists, engine creation returns an error that the GUI surfaces while editing/export still work offline, and the app retries in the background until a device appears; a stream that dies mid-session is reopened by the audio thread on the same interval.

**Same DSP for playback and export.** `export_wav()` renders the project offline through the identical voice/mixer/FX pipeline (including effect tails and mute/solo/master rules) and streams the result to a `.wav` via `hound`; `export_stems()` reuses it per track, honoring the same audibility gates. The GUI runs both on a worker thread and reports progress, so the interface stays responsive during long renders. The `--render`/`--render-demo`/`--render-stems` CLI flags use the same path headlessly.

**Atomic, honest file handling.** `.cadence` files are serde-JSON. `save_project` writes to a temporary file and atomically renames over the target so a failed save can't corrupt the original; files are capped at 20 MB. `save_recovery`/`load_recovery` keep an autosave copy in the platform app-data directory (`directories` crate), and the GUI offers to restore it on next launch — damaged recovery files are never silently loaded as projects.

**Undo/redo in the GUI.** Edit actions go through a project-history layer in `CadenceApp` (snapshot-based undo/redo covering pattern edits, arrangement, and mixer moves). The GUI marks the document dirty and shows **Save / Don't Save / Cancel** before New/Open/MIDI-import/quit; the launch-time recovery offer (**Restore Work / Discard**) deliberately ignores Escape and backdrop clicks so the autosave can't be lost accidentally.

**MIDI is notes, not audio.** `src/midi.rs` uses `midly` for Standard MIDI File exchange: multi-track export with tempo, programs, velocities and durations (drums on channel 10); import builds a fresh `Project` and rejects malformed or unsupported (e.g. SMPTE-time) files cleanly.

## Module map

| Path | Responsibility |
| --- | --- |
| `src/main.rs` | Window creation, CLI argument handling, app icon |
| `src/lib.rs` | Crate root; module declarations |
| `src/model.rs` | Data model, factories, `Project::demo()`, validation, `events()` flattening, drum map |
| `src/audio/mod.rs` + `engine.rs`, `renderer.rs`, `voice.rs`, `dsp.rs`, `song.rs`, `export.rs`, `font.rs`, `meter.rs`, `perf_bench.rs` | `AudioEngine` (CPAL), shared offline/online `Renderer`, per-instrument + SoundFont + sample voices, synth DSP, scheduler, meters, `export_wav`/`export_stems` |
| `src/midi.rs` | `export_midi` / `import_midi` (format-1, 480 PPQ, `midly`) |
| `src/packs.rs` | JSON instrument packs: schema validation, catalog, install, template, `sample`/`kit` specs |
| `src/font.rs` | SoundFont `.sf2` instruments: user font dir, GM program names, catalog |
| `src/gui/mod.rs` + `shell.rs`, `transport.rs`, `sidebar.rs`, `arranger.rs`, `editor.rs`, `mixer.rs`, `dialogs.rs`, `files.rs`, `history.rs`, `theme.rs`, `ops.rs` | `CadenceApp` and all egui UI: menus/status/toasts, transport bar, library/inspector, arranger, pattern editor/keyboard, mixer, dialogs, file flows, undo history, themes, edit ops |
| `src/persistence.rs` | Load/save `.cadence`, atomic writes, recovery file, audio store (`audio/` dir), WAV decode |
| `packs/` | Bundled JSON packs embedded via `include_str!` + `schema.json` + template |
| `tools/instrument-packs/` | Agent/user guide for authoring packs as pure JSON |
| `tests/**` | Integration tests (CLI, persistence, recovery, render, MIDI, packs) + headless GUI tests in `src/gui/tests.rs` |
| `scripts/mac-package.sh`, `scripts/windows-package.ps1` | Native packaging |

## What is deliberately absent

- **No binary plugin host (yet).** Sounds come from the seven built-in synthesizers, data-driven JSON packs (bundled and user-installed), `.sf2` SoundFonts and embedded WAV samples — not VST3/AU binaries. A native plugin host is a roadmap item — see the Limitations section of the README.
- **No network, accounts, or cloud.** Everything is local files and in-process DSP.
- **No web technologies.** No HTML/JS/CSS ships in the final app; `egui` renders the whole interface.
