<div align="center">

# Cadence

**A focused songwriting studio for electronic musicians — native, offline, and yours.**

[![Download the latest release](https://img.shields.io/badge/download-latest%20release-brightgreen?style=for-the-badge&logo=github)](https://github.com/Toom68/cadence/releases/latest)
[![Windows](https://img.shields.io/badge/Windows-10%2B%20x64-0078D4?style=flat-square&logo=windows&logoColor=white)](https://github.com/Toom68/cadence/releases/latest)
[![macOS](https://img.shields.io/badge/macOS-Apple%20Silicon%20%2B%20Intel-000000?style=flat-square&logo=apple&logoColor=white)](https://github.com/Toom68/cadence/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/Toom68/cadence/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/Toom68/cadence/actions)

![Cadence arranger](.github/screenshots/midnight-main.png)

</div>

Cadence is a multi-track music-making app for Windows and macOS, written in Rust. Write drum patterns and melodies in a step sequencer and piano roll, arrange them into songs on a timeline, drop imported WAVs onto audio lanes, mix with per-track effects and subgroup buses, and export WAV, per-track stems, or MIDI.

- **203 bundled instruments** — chiptune, psy-experimental banks, drum kits, guitars, neon leads, one-shot FX, mallets & keys — plus JSON instrument packs, `.sf2` SoundFonts, and `.wav` sample import
- **Audio lanes** for your own WAV clips — move, loop, and fade them like pattern clips
- **A real mixer** — per-track volume/pan/mute/solo, FX, subgroup buses, live peak meters
- **Local-first and offline** — songs are plain `.cadence` JSON files; crash recovery and autosave protect your work. No account, no subscription, no telemetry
- **Native and fast** — pure-Rust synthesis on a real-time audio engine; no Electron, no web runtime

## Download

Get the latest build from **[Releases](https://github.com/Toom68/cadence/releases/latest)** — no build tools needed:

| Platform | File | Notes |
| --- | --- | --- |
| Windows 10+ (x64) | `Cadence-*-windows-x64.zip` | Unzip and run `Cadence.exe`. Unsigned — SmartScreen warns once: **More info → Run anyway** |
| macOS (universal) | `Cadence-*-macos-universal.dmg` (or `.zip`) | Ad-hoc signed — right-click **Open** the first time |

An audio output device is needed to hear playback; editing, saving and WAV export work without one, and Cadence picks up a device that appears later. Pick your output from the audio indicator in the status bar.

## Screenshots

<table>
  <tr>
    <td><img src=".github/screenshots/midnight-mixer.png" alt="Mixer" /></td>
    <td><img src=".github/screenshots/midnight-keys-melodic.png" alt="Keyboard" /></td>
  </tr>
</table>

*One of three built-in themes (Midnight, Graphite, Paper).*

## Build from source

Requires a stable Rust toolchain via [rustup](https://rustup.rs). If `cargo` isn't on your PATH: `export PATH="$HOME/.cargo/bin:$PATH"` (`cargo --version` should work).

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

`Cargo.lock` is committed — build and test with `--locked` for reproducible results (as CI does). Package artifacts land in `dist/`: `Cadence-<version>-macos-<arch>.{zip,dmg}` and `Cadence-<version>-windows-x64.{zip}` (containing `Cadence.exe`). Two Windows paths exist: `windows-package.ps1` builds the native MSVC binary on Windows (the CI path), while `windows-cross-package.sh` cross-builds the GNU-target exe on macOS/Linux. Both scripts honor `CARGO_TARGET_DIR` and copy the bundled font licenses into `licenses/` plus a buyer `README.txt`, `THIRD-PARTY-NOTICES.txt` (every crate and bundled font license, also browsable in-app via Help → About → Third-Party Notices) and `EULA.txt` when one exists at the repo root — the scripts warn when it is missing. Regenerate the notices after dependency changes with `python scripts/gen-third-party-notices.py`. Packages are **not distribution-signed** (macOS ad-hoc, Windows unsigned). The native MSVC Windows package has been launched on Windows 11 (demo opened with audio ready, clean exit); a full human acceptance pass on Windows is still outstanding — see Limitations and [docs/verification.md](docs/verification.md).

`scripts/mac-package.sh` also accepts `UNIVERSAL=1` (arm64 + x86_64 universal binary via `lipo`) and `SKIP_DMG=1` (zip only).

### Headless / CLI modes

The `cadence` binary also runs without the GUI — useful for rendering and validation in scripts and CI:

```bash
cadence --render-demo output.wav              # render the built-in demo song to WAV
cadence --render song.cadence output.wav      # render a saved project to WAV
cadence --render-stems song.cadence out_dir/  # render every audible track to its own WAV
cadence --check song.cadence                  # validate a project file and print a summary
cadence song.cadence                          # open the app with a song
cadence --version / --help                    # version and usage

# Instrument packs (JSON synth presets — see packs/README.md)
cadence --new-pack my-synths pack.json    # write a starter pack template
cadence --validate-pack pack.json         # check a pack file
cadence --install-pack pack.json          # install into the user pack folder
cadence --list-instruments                # list built-in + installed instruments
```

## Using Cadence

The short version: the app opens on the demo song — press **play** to hear it, pick a track in the sidebar, write or edit patterns in the pattern editor (drum step grid or piano roll), arrange pattern and audio clips on the timeline, set tempo/swing/song length, then mix and export.

The window is a menu bar (File · Edit · Track · Transport · View · Help), a transport bar (song name, playback buttons, position display, tempo/tap, swing, master volume and meters, undo/redo), an instrument sidebar on the left, the arranger on top, and a tabbed editor below — **Pattern Editor | Mixer | Keyboard** — with a draggable divider between the two panels. A status bar at the bottom reports save state, audio state and export progress, with toasts for outcomes.

See [docs/songwriting-workflow.md](docs/songwriting-workflow.md) for the full walkthrough, [docs/project-files.md](docs/project-files.md) for saving/recovery/`.cadence` format, and [docs/export.md](docs/export.md) for WAV, stems and MIDI details.

## Keyboard shortcuts

Shortcuts are app-wide (⌘ on macOS, Ctrl on Windows). Press **F1** in the app for the built-in shortcuts & tips dialog.

| Shortcut | Action |
| --- | --- |
| Space | Play / pause |
| Home | Return to start |
| L | Toggle loop |
| R | Toggle record (MIDI/step recording into the selected track's pattern) |
| B | Split the clip at the pointer / playhead (arranger) |
| ⌘/Ctrl + N | New Song |
| ⌘/Ctrl + O | Open… |
| ⌘/Ctrl + S | Save |
| ⌘/Ctrl + Shift + S | Save As… |
| ⌘/Ctrl + E | Export WAV… |
| ⌘/Ctrl + Q | Quit |
| ⌘/Ctrl + Z | Undo |
| ⌘/Ctrl + Shift + Z (or ⌘/Ctrl + Y) | Redo |
| ⌘/Ctrl + D | Duplicate selection |
| ⌘/Ctrl + C / X / V | Copy / cut / paste selection (notes, clips, or a whole track) at the playhead |
| ⌘/Ctrl + A | Select all notes or arranger clips (based on focus) |
| Delete / Backspace | Delete selection |
| Esc | Deselect / close dialog |
| Arrow keys | Nudge selection (Shift+↑/↓ nudges by octave) |
| = / − | Zoom in / out |
| ⌘/Ctrl + 1 / 2 / 3 | Pattern Editor / Mixer / Keyboard tab |
| ⌘/Ctrl + Shift + ↑ / ↓ | Maximize the editor / the arranger (again to restore) |
| F1 | Shortcuts & tips |

The **File** menu also has *New from Demo*, the imports (*MIDI*, *Audio File*, *Sample*, *Instrument Pack*) and the exports (*WAV*, *Stems*, *MIDI*); **Edit** adds *Split Clip at Playhead*, *Add Marker at Playhead*, *Quantize Notes*, *Humanize Notes* and *Arpeggiator…*; **Track** adds, duplicates, deletes and reorders tracks (including audio tracks); **Transport** holds Play/Stop/Loop/Record/Metronome; **View** holds the tabs, zoom, *Fit Song in Arranger*, *Follow Playhead*, the panel sizes (drag or double-click the divider between the arranger and the editor too) and *Theme* — Graphite (default), Midnight, or Paper — which recolors the whole interface and is remembered between sessions.

## Export

- **File → Export WAV…** renders the current mix (honoring mute/solo, buses and master volume) to a `.wav` file on a background thread with progress.
- **File → Export Stems…** renders every audible track to its own `<track name>-NN.wav` in a folder you pick — the same per-track signal it contributes to the mix.
- **File → Export MIDI…** writes the song's notes to a `.mid` file for use in other DAWs; **File → Import MIDI…** brings an existing `.mid`/`.midi` back in as a project.

Details and limitations: [docs/export.md](docs/export.md).

## Packaging

- `scripts/mac-package.sh` builds `dist/Cadence.app` (ad-hoc signed, icon included), a `.zip`, and a `.dmg` when `hdiutil` is present; `UNIVERSAL=1` produces a universal binary.
- `scripts/windows-package.ps1` (or `pwsh scripts/windows-package.ps1`) builds `dist/Cadence-<version>-windows-x64/Cadence.exe` — a GUI-subsystem exe with the MSVC CRT statically linked, so the portable zip needs no redistributable — plus the `.zip`.

Packaging is honest: macOS bundles are **ad-hoc signed only** (no Developer ID, not notarized) and Windows executables are **unsigned** — no Authenticode certificate. Distributing these builds may trigger platform security prompts.

## Project files

A `.cadence` file is a plain-text JSON document describing tempo, swing, song length, key, section markers, chord events, buses, and every track/pattern/clip/note. Cadence validates files on open and keeps a local recovery copy so unsaved work survives crashes. Imported WAV clips live in a per-install audio store the project references by name — see [docs/project-files.md](docs/project-files.md).

## Limitations

- **Built-in synthesizers, JSON packs and SoundFonts — no binary plugin hosting yet.** Sounds are synthesized in-app (seven built-in timbres, 196 bundled pack presets across thirteen banks, optional user-installed JSON packs), sampled from user-installed `.sf2` SoundFonts, or embedded as small WAV clips — see [docs/instrument-packs.md](docs/instrument-packs.md) and [packs/README.md](packs/README.md). Loading VST3/AU/CLAP plugins is a designed-but-unimplemented roadmap item; Cadence does **not** load them today.
- **Packages are not distribution-signed.** macOS builds are ad-hoc signed only and Windows builds are unsigned — no Developer ID, notarization, or Authenticode, so platform security warnings (Gatekeeper, SmartScreen) can appear when sharing builds. The native MSVC Windows package has been launched on Windows 11 (demo with audio ready, clean exit, GUI screenshot-swept in all three themes); a full human acceptance pass and audio-device hot-plug on Windows are still unverified. Windows 10+ is the expected target per Rust's platform support.
- **No audio recording.** Cadence is a pattern-based sequencer — you compose notes and drop in finished WAV files; you don't record takes. MIDI recording from the computer keyboard or Keyboard tab exists (the record button or **R**), writing notes into the selected track's pattern.
- **Audio clips are per-install.** Imported WAVs are staged into the app's audio store and referenced by content-hash name; a `.cadence` file does not embed the audio, so a song's audio lanes only sound where the same files were imported.
- Fixed sixteenth-note grid (1/16 snap); positions are stored in sixteenth steps.
- Swing amount up to 0.6, tempo 30–300 BPM, 1–128 bars per song, up to 32 tracks, four fixed subgroup buses.

## Documentation

| Document | Contents |
| --- | --- |
| [docs/songwriting-workflow.md](docs/songwriting-workflow.md) | Step-by-step guide to writing a song in Cadence |
| [docs/project-files.md](docs/project-files.md) | `.cadence` JSON format, save/open, recovery, audio store, validation limits |
| [docs/export.md](docs/export.md) | WAV, stems and MIDI export/import behavior and caveats |
| [docs/instrument-packs.md](docs/instrument-packs.md) | JSON instrument packs: make, install, and where they live |
| [docs/architecture.md](docs/architecture.md) | How the Rust crate, audio engine, model, and GUI fit together |
| [docs/verification.md](docs/verification.md) | What has actually been verified — test results, native run evidence, limits |
| [docs/acceptance-checklist.md](docs/acceptance-checklist.md) | Manual acceptance checklist for verifying a build |

## Architecture at a glance

Single Rust crate `cadence-studio` — library `cadence_studio`, binary `cadence`:

```
src/main.rs        window setup, CLI modes (song open, --render*/--check, packs)
src/lib.rs         module wiring
src/model.rs       Project/Track/Pattern/Clip/Note/AudioClip/Bus data model + validation
src/audio/         CPAL playback engine, synth voices, mixer/FX, WAV & stem renderer
src/midi.rs        Standard MIDI File import/export (midly)
src/packs.rs       JSON instrument packs (validate, catalog, install)
src/font.rs        SoundFont (.sf2) instruments: user font dir, GM program names, catalog
src/gui/           eframe/egui interface: shell (menus/status/toasts), transport,
                   sidebar, arranger, editors, mixer, dialogs, theme
src/persistence.rs .cadence save/load, atomic writes, recovery autosave, audio store
packs/             bundled instrument packs (embedded via include_str!)
tools/instrument-packs/  agent guide for authoring new packs as pure JSON
```

Everything runs in-process: the GUI drives the audio engine through a shared `Project`, playback renders on the CPAL callback, and file I/O stays on the UI thread's worker paths. Full details in [docs/architecture.md](docs/architecture.md).

UI fonts: [Inter](https://rsms.me/inter/) 4.1 (SIL Open Font License 1.1) and [DejaVu Sans](https://dejavu-fonts.github.io/) 2.37 for symbols (Bitstream Vera license), bundled in `assets/fonts/` with their license texts.
