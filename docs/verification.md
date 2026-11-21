# Verification status

What has actually been run and observed — and what hasn't. This file exists so claims elsewhere in the docs can be checked against evidence.

## Polish program — November 22, 2026 (current tree)

Since the 1.1.0 record below, a multi-worker release-polish program rebuilt the app shell (menus, transport bar, status bar with toasts, themes, arranger/editor split), the sidebar (instrument cards, bank chooser, collapsible sections, inspector), the arranger (four-strip ruler, markers, chord lane, automation lanes, audio lanes), the pattern editor (rename/overflow menu, chord-strip tabs, dedicated Keyboard tab, recording) and the mixer (live meters, dB faders, buses, clip LEDs). Each worker ran the standard gates on its own branch; UI evidence is screenshot-backed in the program's `shots/` folders (arranger, editor, sidebar, mixer, shell, design) plus a mixer performance log.

Re-run on the final merged tree after the QA pass (Windows 11, x86-64, MSVC, Rust 1.100.0):

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean |
| `cargo test --locked --all-targets` | **581 passed, 0 failed, 10 ignored** (529 library + 52 integration) |
| `cargo build --release --locked` | passed |

- All test binaries pass: main (4), cli (5), fonts (4, +2 ignored — need a real `.sf2`), midi_interop (2), packs_pipeline (5), persistence (13), psy_soundbank (6), recovery (1), render (6), stems (6).
- The QA pass fixed the integration failures found after merging (a clippy lint, a toast that swallowed input for one frame, the piano-roll height at 1360×860, a coordinate-bound test) plus nine smaller UI bugs, and rewrote the F1 help as the complete list of shortcuts and gestures.
- Visual sweep: 30 scenes × 3 themes (Graphite, Midnight, Paper) screenshotted and reviewed on the native Windows build.
- Offline renders of the demo and both example songs are bit-identical before and after the performance work (SHA-256 compared).
- **Windows runtime — first native run:** `scripts/windows-package.ps1` built the MSVC package (`Cadence-1.1.0-windows-x64.zip`, 5.2 MB, containing `Cadence.exe` and `licenses/` with the Inter and DejaVu font licenses). The packaged exe was launched on Windows 11 with an isolated data dir: it opened the demo song with the audio engine ready, rendered correctly, and closed cleanly. The exe is still **unsigned** (SmartScreen will warn). Not verified on Windows: a full human acceptance pass, audio-device unplug/replug, and what SmartScreen shows to a downloader.

## November 15, 2026 — release 1.1.0 (historical)

## Automated checks — all green

Run on macOS (arm64) against Cadence 1.1.0 on November 15, 2026:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | clean |
| `cargo test --locked --offline --all-targets` | **201 passed, 0 failed, 1 ignored** |
| Windows GNU target Clippy, all targets, `-D warnings` | clean |

Test breakdown: 160 library tests + 41 binary/integration tests. The single ignored test is the real-device audio lifecycle test, which needs a live output device and is skipped normally. Coverage includes:

- `tests/` — CLI surface, persistence round-trips and atomic writes, crash-recovery autosave, offline WAV render, MIDI interop, and the instrument-pack pipeline.
- `src/gui/tests.rs` — headless egui tests driving real frames: drum grid paint/erase/accents, piano-roll draw/move/resize, marquee + velocity lane, arrow-key nudge, arranger clip drag/place/loop/fade handles, unsaved-changes guard, dialogs, save/open and MIDI/WAV round-trips.
- Unit tests across `model`, `audio`, `midi`, `packs`, `persistence`.
- Clip loops and note-velocity fades, legacy clip-field defaults, and fade bounds after shortening a clip or a shared pattern. Regression checks include project validation and serialization round-trips.
- Plain drag-box selection across arranger lanes and piano-roll notes; reverse/additive selection, Shift-click removal, movement from a non-primary item, shared bounds, copying loop/fade metadata, atomic capacity rejection, and one undo per multi-frame group drag. Selection does not dirty the song, and a piano-roll blank drag does not insert a note.
- Follow-playhead suppression during selection gestures and safe movement of looped clips whose span exceeds the song limit.
- All 128 new presets validate, deploy through the embedded catalog, have unique synthesis definitions, and render finite, non-silent, bounded audio with decaying tails. The catalog contains 138 pack presets plus six built-ins (144 sounds).
- Pitch/filter envelopes, resonance, LFOs, oscillator decay, hostile parameter bounds, and block-size invariance. All ten original 8-bit presets also produce byte-identical WAVs compared with the pre-upgrade binary on fixed-ID projects.
- Sticky library search/bank filtering, clear/reset and stale-bank repair, preview metadata routing, useful empty-pattern centering, and preservation of nonempty pattern centering. File-flow tests use unique temporary directories, including their asynchronous WAV worker.

## Soundbank audit and showcases

`cargo run --release --locked --offline --example bank_audit -- dist/soundbank` generated all seven category reels, per-preset CSV/JSON metrics, and two complete song previews. The default run left checked-in song hashes unchanged; `--write-songs` explicitly regenerates those source examples.

- Individual preset peaks: 0.0229–0.496; maximum absolute DC: 3.66e-8. These checks establish signal sanity, not subjective musical quality.
- **Neon Circuits**: 16 bars, 140 BPM, 15 tracks; 36.44-second WAV including effects tail, peak 0.441.
- **Midnight Dub**: 16 bars, 96 BPM, 12 tracks; 51.26-second WAV including effects tail, peak 0.334.
- Both native showcase projects validate and embed every sound. Standalone percussion opens in the piano roll, so notes outside the built-in kit pitches remain editable.

An independent audio/model/packs/MIDI harness additionally ran **78 unit checks + 5 probes, all green**.

## Native app — observed on macOS

- The November 13 packaged app (arm64) was visually inspected: window, panels, and library render correctly. Looping playback through the real CPAL engine was observed: live voices (14 active) and `audioReady` status. The November 14 loop/fade changes have automated coverage; they have not had a fresh interactive GUI pass.
- An isolated November 15 native test build displayed the new selection hints and selected all 22 demo clips. Arrow-key movement moved the group and grew the song to nine bars; one undo restored the clean eight-bar song. Mouse drag-box and group-drag behavior has headless egui input coverage; a native mouse-gesture pass remains outstanding.
- Those native observations concern 1.0. The 1.1 bank browser has headless frame/input coverage; a fresh native visual pass was not completed because Computer Use could not find the isolated second app's window.
- The November 15 packaged macOS binary rendered the full Afterglow demo successfully: 22.95 seconds including the effect tail, with non-silent stereo audio at 44.1 kHz, using an isolated data directory.
- Both platform archives were rebuilt from the combined source on November 15. macOS signature verification and ZIP CRC checks pass; `dist/SHA256SUMS.txt` records the archive checksums.
- Release artifacts: `dist/Cadence.app`, `Cadence-1.1.0-macos-arm64.{zip,dmg}`, `Cadence-1.1.0-windows-x64.zip`, and `Cadence-1.1.0-Psy-Collection.zip` (JSON banks, guide and native showcase songs).
- macOS bundle is **ad-hoc signed only** — not Developer ID, not notarized.

## Windows x64 artifact — built and inspected (runtime unverified)

The GNU-target Windows build was produced on this Mac:

- `cargo check --locked --offline --target x86_64-pc-windows-gnu` — passed.
- `cargo build --release --locked --offline --target x86_64-pc-windows-gnu` — passed.
- `scripts/windows-cross-package.sh` ran successfully under macOS Bash 3.2 and produced `dist/Cadence-1.1.0-windows-x64/Cadence.exe` and `dist/Cadence-1.1.0-windows-x64.zip`.
- `file` confirms a PE32+ x86-64 Windows GUI-subsystem executable; `objdump` shows imports only from Windows inbox/UCRT/API-set DLLs — **no libgcc, libstdc++, or libwinpthread runtime DLLs required**. The embedded `.rsrc` icon is present.
- ZIP integrity (CRC) checks pass for both the Mac and Windows archives.
- The exe is **unsigned** (no Authenticode). It has **not been run on Windows or Wine**; Windows 10+ is the expected target per Rust's official platform support. The native MSVC path (`scripts/windows-package.ps1`, static CRT via `.cargo/config.toml`) remains the CI/Windows-machine route — the delivered `dist/` artifacts are the GNU cross-build.

## Not yet verified

- **Windows runtime (partial)**: the native MSVC package has launched, played and closed cleanly on Windows 11 (see the polish-program entry above), and the GUI was screenshot-swept there. Still unverified on Windows: a full human acceptance pass, audio-device hot-plug, and installer/SmartScreen prompts. The historical GNU cross-built exe below has still never been run.
- **Human GUI pass**: interactive editing feel, dialog flows, and mix judgment beyond the native observations and headless tests above — tracked in [acceptance-checklist.md](acceptance-checklist.md).
- **Distribution prompts**: what Gatekeeper/SmartScreen actually show for these unsigned builds depends on how they're distributed; not asserted here.
- The `cargo test` suite does not exercise a physical audio device (the one device-dependent test is ignored by default).
