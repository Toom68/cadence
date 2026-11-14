# Verification status

What has actually been run and observed — and what hasn't. This file exists so claims elsewhere in the docs can be checked against evidence.

## Automated checks — all green

Run on macOS (arm64) against the combined source tree on November 15, 2026:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --locked --offline --all-targets -- -D warnings` | clean |
| `cargo test --locked --offline --all-targets` | **178 passed, 0 failed, 1 ignored** |
| Windows GNU target Clippy, all targets, `-D warnings` | clean |

Test breakdown: 143 library tests + 35 binary/integration tests. The single ignored test is the real-device audio lifecycle test, which needs a live output device and is skipped normally. Coverage includes:

- `tests/` — CLI surface, persistence round-trips and atomic writes, crash-recovery autosave, offline WAV render, MIDI interop, and the instrument-pack pipeline.
- `src/gui/tests.rs` — headless egui tests driving real frames: drum grid paint/erase/accents, piano-roll draw/move/resize, marquee + velocity lane, arrow-key nudge, arranger clip drag/place/loop/fade handles, unsaved-changes guard, dialogs, save/open and MIDI/WAV round-trips.
- Unit tests across `model`, `audio`, `midi`, `packs`, `persistence`.
- Clip loops and note-velocity fades, legacy clip-field defaults, and fade bounds after shortening a clip or a shared pattern. Regression checks include project validation and serialization round-trips.
- Plain drag-box selection across arranger lanes and piano-roll notes; reverse/additive selection, Shift-click removal, movement from a non-primary item, shared bounds, copying loop/fade metadata, atomic capacity rejection, and one undo per multi-frame group drag. Selection does not dirty the song, and a piano-roll blank drag does not insert a note.
- Follow-playhead suppression during selection gestures and safe movement of looped clips whose span exceeds the song limit.

An independent audio/model/packs/MIDI harness additionally ran **78 unit checks + 5 probes, all green**.

## Native app — observed on macOS

- The November 13 packaged app (arm64) was visually inspected: window, panels, and library render correctly. Looping playback through the real CPAL engine was observed: live voices (14 active) and `audioReady` status. The November 14 loop/fade changes have automated coverage; they have not had a fresh interactive GUI pass.
- An isolated November 15 native test build displayed the new selection hints and selected all 22 demo clips. Arrow-key movement moved the group and grew the song to nine bars; one undo restored the clean eight-bar song. Mouse drag-box and group-drag behavior has headless egui input coverage; a native mouse-gesture pass remains outstanding.
- The November 15 packaged macOS binary rendered the full Afterglow demo successfully: 22.95 seconds including the effect tail, with non-silent stereo audio at 44.1 kHz, using an isolated data directory.
- Both platform archives were rebuilt from the combined source on November 15. macOS signature verification and ZIP CRC checks pass; `dist/SHA256SUMS.txt` records the archive checksums.
- Release artifacts produced: `dist/Cadence.app`, `Cadence-1.0.0-macos-arm64.zip` (~3.1 MiB), `Cadence-1.0.0-macos-arm64.dmg` (~3.7 MiB); release binary ~6.8 MiB.
- macOS bundle is **ad-hoc signed only** — not Developer ID, not notarized.

## Windows x64 artifact — built and inspected (runtime unverified)

The GNU-target Windows build was produced on this Mac:

- `cargo check --locked --offline --target x86_64-pc-windows-gnu` — passed.
- `cargo build --release --locked --offline --target x86_64-pc-windows-gnu` — passed.
- `scripts/windows-cross-package.sh` ran successfully under macOS Bash 3.2 and produced `dist/Cadence-1.0.0-windows-x64/Cadence.exe` (~7.4 MiB) and `dist/Cadence-1.0.0-windows-x64.zip` (~3.4 MiB).
- `file` confirms a PE32+ x86-64 Windows GUI-subsystem executable; `objdump` shows imports only from Windows inbox/UCRT/API-set DLLs — **no libgcc, libstdc++, or libwinpthread runtime DLLs required**. The embedded `.rsrc` icon is present.
- ZIP integrity (CRC) checks pass for both the Mac and Windows archives.
- The exe is **unsigned** (no Authenticode). It has **not been run on Windows or Wine**; Windows 10+ is the expected target per Rust's official platform support. The native MSVC path (`scripts/windows-package.ps1`, static CRT via `.cargo/config.toml`) remains the CI/Windows-machine route — the delivered `dist/` artifacts are the GNU cross-build.

## Not yet verified

- **Windows runtime**: the x64 exe exists and is structurally sound, but nothing has executed it on Windows (or Wine). GUI behavior, audio device handling, and installer/SmartScreen prompts on Windows are all unverified.
- **Human GUI pass**: interactive editing feel, dialog flows, and mix judgment beyond the native observations and headless tests above — tracked in [acceptance-checklist.md](acceptance-checklist.md).
- **Distribution prompts**: what Gatekeeper/SmartScreen actually show for these unsigned builds depends on how they're distributed; not asserted here.
- The `cargo test` suite does not exercise a physical audio device (the one device-dependent test is ignored by default).
