# Cadence

Cadence is a focused, multi-track songwriting studio for macOS and Windows — a native Rust desktop app. Write drum patterns and melodies in a step sequencer and piano roll, arrange them into songs across multiple instrument tracks, mix built-in synthesized instruments with per-track effects, and export WAV or MIDI.

- **Native Rust desktop app** — an eframe/egui interface driven by a CPAL audio engine; no Electron, no web runtime, no Node.js needed to build or run
- **144 sounds plus JSON packs** — synthesized in Rust: 6 built-ins and 138 bundled pack presets across eight banks (10 chiptune `8-bit` sounds and 128 experimental `psy-*` instruments), plus unlimited user-installed JSON packs (pure JSON, no rebuild); no samples or binary plugins required
- **Local-first** — projects are plain JSON `.cadence` files; an automatic recovery copy protects against lost work; the app works fully offline after building

## Requirements

- A stable Rust toolchain installed via [rustup](https://rustup.rs). If `cargo` isn't on your PATH: `export PATH="$HOME/.cargo/bin:$PATH"` (`cargo --version` should work)
- macOS or Windows for the desktop app
- An audio output device for playback (the app still launches, edits, and exports without one)

## Quick start

```bash
cargo run            # build and launch the Cadence window
```

On first launch Cadence opens the included demo song, *Afterglow*. If a valid recovery copy exists it offers to restore it instead.

## Commands

| Command | What it does |
| --- | --- |
| `cargo build` | Debug build → `target/debug/cadence` |
| `cargo run` | Build and launch the GUI |
| `cargo build --release` | Optimized build → `target/release/cadence` |
| `cargo test --locked --all-targets` | Run unit and integration tests |
| `cargo fmt --all -- --check` | Verify formatting |
| `cargo clippy --locked --all-targets -- -D warnings` | Lint |
| `scripts/mac-package.sh` | macOS package: `Cadence.app`, `.zip`, and `.dmg` when `hdiutil` is available |
| `scripts/windows-package.ps1` | Windows package on Windows: release `Cadence.exe` plus a portable `.zip` |
| `scripts/windows-cross-package.sh` | Windows x64 package from macOS/Linux via the GNU target (needs `mingw-w64` and `rustup target add x86_64-pc-windows-gnu`; installs nothing itself) |

`Cargo.lock` is committed — build and test with `--locked` for reproducible results (as CI does). Package artifacts land in `dist/`: `Cadence-<version>-macos-<arch>.{zip,dmg}` and `Cadence-<version>-windows-x64.{zip}` (containing `Cadence.exe`). Two Windows paths exist: `windows-package.ps1` builds the native MSVC binary on Windows (the CI path), while `windows-cross-package.sh` cross-builds the GNU-target exe on macOS/Linux — the delivered `dist/` Windows artifacts here are the GNU cross-build. Packages are **not distribution-signed** (macOS ad-hoc, Windows unsigned) and the Windows exe has **not been run on Windows** — see Limitations and [docs/verification.md](docs/verification.md).

`scripts/mac-package.sh` also accepts `UNIVERSAL=1` (arm64 + x86_64 universal binary via `lipo`) and `SKIP_DMG=1` (zip only).

### Headless / CLI modes

The `cadence` binary also runs without the GUI — useful for rendering and validation in scripts and CI:

```bash
cadence --render-demo output.wav          # render the built-in demo song to WAV
cadence --render song.cadence output.wav  # render a saved project to WAV
cadence --check song.cadence              # validate a project file and print a summary
cadence song.cadence                      # open the app with a song
cadence --version / --help                # version and usage

# Instrument packs (JSON synth presets — see packs/README.md)
cadence --new-pack my-synths pack.json    # write a starter pack template
cadence --validate-pack pack.json         # check a pack file
cadence --install-pack pack.json          # install into the user pack folder
cadence --list-instruments                # list built-in + installed instruments
```

## Using Cadence

The short version: the app opens on the demo song — press **play** to hear it, pick a track in the sidebar, write or edit patterns in the track editor (drum step grid or piano roll), arrange pattern clips on the timeline, set tempo/swing/song length, then mix and export.

See [docs/songwriting-workflow.md](docs/songwriting-workflow.md) for the full walkthrough, [docs/project-files.md](docs/project-files.md) for saving/recovery/`.cadence` format, and [docs/export.md](docs/export.md) for WAV and MIDI details.

## Keyboard shortcuts

Shortcuts are app-wide (⌘ on macOS, Ctrl on Windows). Press **F1** in the app for the built-in shortcuts & tips dialog.

| Shortcut | Action |
| --- | --- |
| Space | Play / pause |
| Home | Return to start |
| L | Toggle loop |
| ⌘/Ctrl + N | New Song |
| ⌘/Ctrl + O | Open… |
| ⌘/Ctrl + S | Save |
| ⌘/Ctrl + Shift + S | Save As… |
| ⌘/Ctrl + E | Export WAV… |
| ⌘/Ctrl + Q | Quit |
| ⌘/Ctrl + Z | Undo |
| ⌘/Ctrl + Shift + Z (or ⌘/Ctrl + Y) | Redo |
| ⌘/Ctrl + D | Duplicate selection |
| ⌘/Ctrl + A | Select all notes or arranger clips (based on focus) |
| Delete / Backspace | Delete selection |
| Esc | Deselect / close dialog |
| Arrow keys | Nudge selection (Shift+↑/↓ nudges by octave) |
| = / − | Zoom in / out |
| ⌘/Ctrl + 1 / 2 | Pattern Editor / Mixer tab |
| F1 | Shortcuts & tips |

The **File** menu also has *New from Demo*, *Import MIDI…*, and *Export MIDI…*; **Transport** holds Play/Stop/Loop/Metronome; **View** holds zoom and *Fit Song in Arranger*.

## Export

- **File → Export WAV…** renders the current mix (honoring mute/solo and master volume) to a `.wav` file on a background thread with progress.
- **File → Export MIDI…** writes the song's notes to a `.mid` file for use in other DAWs; **File → Import MIDI…** brings an existing `.mid`/`.midi` back in as a project.

Details and limitations: [docs/export.md](docs/export.md).

## Packaging

- `scripts/mac-package.sh` builds `dist/Cadence.app` (ad-hoc signed, icon included), a `.zip`, and a `.dmg` when `hdiutil` is present; `UNIVERSAL=1` produces a universal binary.
- `scripts/windows-package.ps1` (or `pwsh scripts/windows-package.ps1`) builds `dist/Cadence-<version>-windows-x64/Cadence.exe` — a GUI-subsystem exe with the MSVC CRT statically linked, so the portable zip needs no redistributable — plus the `.zip`.

Packaging is honest: macOS bundles are **ad-hoc signed only** (no Developer ID, not notarized) and Windows executables are **unsigned** — no Authenticode certificate. Distributing these builds may trigger platform security prompts.

## Project files

A `.cadence` file is a plain-text JSON document describing tempo, swing, song length, and every track/pattern/clip/note. Cadence validates files on open and keeps a local recovery copy so unsaved work survives crashes. See [docs/project-files.md](docs/project-files.md).

## Limitations

- **Built-in synthesizers and JSON packs — no binary plugin hosting yet.** Sounds are synthesized in-app: six built-in timbres, 138 bundled pack presets (ten `8-bit` chiptune sounds and 128 experimental `psy-*` instruments in seven banks), and optional user-installed JSON instrument packs (see [docs/instrument-packs.md](docs/instrument-packs.md) and [packs/README.md](packs/README.md)). Loading VST3/AU/CLAP plugins is a designed-but-unimplemented roadmap item; Cadence does **not** load them today.
- **Packages are not distribution-signed and Windows is unverified at runtime.** macOS builds are ad-hoc signed only and Windows builds are unsigned — no Developer ID, notarization, or Authenticode, so platform security warnings can appear when sharing builds. The Windows x64 exe builds and packages cleanly (GNU cross-build; MSVC path in CI) but has not been executed on Windows or Wine — Windows 10+ is the expected target per Rust's platform support.
- **No audio recording or audio-file import.** Cadence is a pattern-based MIDI-style sequencer — you compose notes, you don't record takes.
- Fixed sixteenth-note grid (1/16 snap); positions are stored in sixteenth steps.
- Swing amount up to 0.6, tempo 30–300 BPM, 1–128 bars per song, up to 32 tracks.
- WAV export is a full mixdown only — no per-track stems.

## Documentation

| Document | Contents |
| --- | --- |
| [docs/songwriting-workflow.md](docs/songwriting-workflow.md) | Step-by-step guide to writing a song in Cadence |
| [docs/project-files.md](docs/project-files.md) | `.cadence` JSON format, save/open, recovery, validation limits |
| [docs/export.md](docs/export.md) | WAV and MIDI export/import behavior and caveats |
| [docs/instrument-packs.md](docs/instrument-packs.md) | JSON instrument packs: make, install, and where they live |
| [docs/architecture.md](docs/architecture.md) | How the Rust crate, audio engine, model, and GUI fit together |
| [docs/verification.md](docs/verification.md) | What has actually been verified — test results, native run evidence, limits |
| [docs/acceptance-checklist.md](docs/acceptance-checklist.md) | Manual acceptance checklist for verifying a build |

## Architecture at a glance

Single Rust crate `cadence-studio` — library `cadence_studio`, binary `cadence`:

```
src/main.rs        window setup, CLI modes (song open, --render*/--check, packs)
src/lib.rs         module wiring
src/model.rs       Project/Track/Pattern/Clip/Note data model + validation
src/audio/         CPAL playback engine, synth voices, mixer/FX, WAV renderer
src/midi.rs        Standard MIDI File import/export (midly)
src/packs.rs       JSON instrument packs (validate, catalog, install)
src/gui/           eframe/egui interface: transport, sidebar, arranger, editors, mixer
src/persistence.rs .cadence save/load, atomic writes, recovery autosave
packs/             bundled instrument packs (embedded via include_str!)
tools/instrument-packs/  agent guide for authoring new packs as pure JSON
```

Everything runs in-process: the GUI drives the audio engine through a shared `Project`, playback renders on the CPAL callback, and file I/O stays on the UI thread's worker paths. Full details in [docs/architecture.md](docs/architecture.md).
