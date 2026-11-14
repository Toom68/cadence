# Acceptance checklist

Two parts: **Verified** items have automated-test or native-run evidence (see [verification.md](verification.md) for the details) and are checked off. **Manual checks** still need a human on real hardware — they are intentionally left unchecked rather than claimed.

## Verified by automated tests / native run

- [x] `cargo fmt --all -- --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean.
- [x] `cargo test --locked --all-targets`: **178 passed, 0 failed, 1 ignored** (143 library + 35 binary/integration; the ignored test is the real-device lifecycle test that needs a live audio device).
- [x] Persistence, recovery, CLI, render/WAV, MIDI interop, and pack-pipeline integration tests (`tests/`).
- [x] Headless GUI tests (`src/gui/tests.rs`): drum grid paint/erase/drag/accents, piano-roll draw/move/resize, marquee select + delete, velocity lane, arrow-key nudge, arranger clip drag + double-click placement, unsaved-changes guard, Escape closes Help, audio-less Space warns but editing continues, save/open roundtrip, MIDI roundtrip, WAV export worker, preset track embedding/portability.
- [x] Clip loop/fade handle tests, legacy file compatibility, and regression checks that shortening loops or shared patterns keeps fades valid and saveable.
- [x] Headless input tests for plain/reverse/additive box selection, group movement across tracks and piano-roll notes, Shift-click removal, bounds, Alt-copy metadata/capacity, no accidental note insertion or dirty state, and one undo per multi-frame gesture.
- [x] Isolated November 15 native test build: select all 22 demo clips, nudge the group, and undo back to the clean eight-bar song.
- [x] `--render-demo` produces a non-empty WAV; November 13 packaged `.app` visually inspected, looped real CPAL playback showed live voices and `audioReady`. Latest loop/fade changes have automated coverage only.
- [x] Release artifacts built at `dist/Cadence.app` + `Cadence-1.0.0-macos-arm64.{zip,dmg}`.
- [x] Windows x64 GNU artifact built on macOS: `cargo check`/`build --release --target x86_64-pc-windows-gnu` passed; `scripts/windows-cross-package.sh` produced `dist/Cadence-1.0.0-windows-x64/Cadence.exe` (~7.4 MiB, PE32+ GUI subsystem, inbox/UCRT imports only, embedded icon) + `.zip` (~3.4 MiB); CRC integrity passes. **Not executed** — runtime still unverified.

## Manual checks still to run

Run these on each platform you ship. **Windows runtime is unverified** — a real `dist/Cadence-1.0.0-windows-x64/Cadence.exe` (GNU cross-build) now exists and is structurally inspected, but no Windows machine or Wine has executed it yet.

### Build & launch

- [ ] `cargo run` opens the window titled "Cadence" with the app icon on the target OS.
- [ ] First launch opens the demo song *Afterglow* (104 BPM, multiple tracks).
- [ ] Recovery offer appears only when a valid autosave exists; Restore Work / Discard behave as documented; a damaged autosave is quarantined and reported.
- [ ] Window usable at minimum size (~1100×760) and stays responsive while playing.

### Playback & transport

- [ ] Play/pause/stop; playhead tracks engine position; loop and metronome work.
- [ ] Tempo (30–300) and swing (0–0.6) changes are audible; master volume and peak meters respond.
- [ ] Mute silences a track; solo leaves only soloed tracks audible.
- [ ] With no audio device the GUI still opens, edits work, and **Retry audio** recovers when a device appears.

### Writing music (human pass)

- [ ] Sidebar adds tracks to the 32-track limit; reorder/duplicate/delete work.
- [ ] Drum grid painting, accents, Fill button, and velocity lane feel right.
- [ ] Piano roll draw/drag/resize/box-select/right-click erase; key-column audition; scroll/zoom.
- [ ] Pattern actions (create/select/rename/duplicate/length/clear) and arranger clip operations (place/drag/right-click menu/ruler scrub/snap selector/zoom/fit/follow).
- [ ] Clip right-edge loop handles and top-corner note-velocity fades feel usable; shortening loops/patterns and undo/redo preserve valid fades.
- [ ] Mixer faders/knobs/mute/solo audible per track plus master.
- [ ] Unsaved-changes dialog (**Save / Don't Save / Cancel**) on New/Open/Import MIDI/Quit; failed save keeps the dialog.
- [ ] Undo/redo across pattern, arrangement, and mixer edits.

### Files & recovery

- [ ] Save / Save As via native dialogs; reopen restores the full project.
- [ ] Quit-without-save flow; autosave restores on next launch; malformed `.cadence` shows an error without modifying the file.

### Export

- [ ] Export WAV produces a playable file of song length + effect tail, honoring mute/solo/master, with progress shown.
- [ ] Export MIDI round-trips through Cadence and imports into another DAW; import rejects malformed and SMPTE-time files cleanly.

### Packaging & packs

- [ ] `scripts/mac-package.sh` output launches from `dist/` (zip and dmg install paths).
- [ ] The cross-built `dist/Cadence-1.0.0-windows-x64/Cadence.exe` (or a `windows-package.ps1` MSVC build) launches on a real Windows machine and plays audio.
- [ ] Pack pipeline end-to-end in the GUI: Import Pack…, ~2 s auto-rescan, *Update sound from installed pack* flow.
- [ ] Unsigned-build behavior: observe what Gatekeeper/SmartScreen actually do on your distribution channel (not guaranteed either way for local builds).

## Known limitations to confirm (not bugs)

- Sounds are built-in synthesizers or JSON pack presets; there is no VST3/AU/CLAP plugin loading.
- No audio recording or audio-file import.
- Sixteenth-note grid snap; no freehand timing.
- WAV export is a stereo mixdown only — no stems.
- Windows support builds and packages on both paths (MSVC/CI and GNU cross-build) but has not been runtime-verified on Windows or Wine.
