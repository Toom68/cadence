# AGENTS.md — working on Cadence

Cadence is a native, offline music sequencer written entirely in Rust (eframe/egui GUI,
CPAL audio, pure-Rust synthesis). There is no JavaScript/Electron code; do not add any.
A stray local `node_modules/` (ignored) may exist from an abandoned prototype and can be deleted.

## Toolchain and commands

Rust stable (CI pins 1.100.0; MSRV 1.88), edition 2021. If cargo is not on PATH:
`export PATH="$HOME/.cargo/bin:$PATH"`.

```sh
cargo fmt --all                                   # format (CI runs --check)
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets                 # unit + integration tests
cargo build --release --locked                    # target/release/cadence(.exe)
cargo run --release -- --render-demo demo.wav     # headless audio acceptance check
scripts/mac-package.sh                            # dist/Cadence.app + .zip (+ .dmg); UNIVERSAL=1 for arm64+x86_64
pwsh scripts/windows-package.ps1                  # dist/Cadence-<ver>-windows-x64/Cadence.exe + .zip (Windows CI)
scripts/windows-cross-package.sh                  # same zip via x86_64-pc-windows-gnu on macOS/Linux (needs mingw-w64 + rustup target)
```

Always run fmt, clippy and test before finishing. `Cargo.lock` is committed; build with `--locked`.
Dependencies are pinned to known APIs (eframe/egui 0.33.3, cpal 0.16.0, rfd 0.15.4, midly 0.5,
hound 3.5, serde 1, directories 6). Do not bump major/minor versions casually: newer eframe/cpal
releases change APIs. Prefer std over adding crates.

Packaging is honest: macOS bundles are ad-hoc signed only (no Developer ID, not notarized);
Windows executables are unsigned. Do not claim otherwise.

## Layout

| path | purpose |
|---|---|
| `src/main.rs` | binary `cadence`: window setup + CLI modes (see `cadence --help`) |
| `src/lib.rs` | library `cadence_studio`: module declarations, `launch_path()` |
| `src/model.rs` | song model (`Project`, `Track`, `Pattern`, `Note`, `Clip`), limits, `validate`, demo song |
| `src/audio/` | real-time `AudioEngine` and offline `export_wav` (same DSP) |
| `src/midi.rs` | Standard MIDI File import/export |
| `src/persistence.rs` | atomic `.cadence` save/load (20 MB cap), crash-recovery autosave, `data_dir()` |
| `src/packs.rs` | JSON instrument packs: schema types, validation, install, catalog |
| `src/gui/` | egui UI (`CadenceApp`) |
| `packs/` | bundled packs (embedded via `include_str!`), `schema.json`, template |
| `tests/` | integration tests (persistence, recovery, render/WAV, CLI, MIDI, pack pipeline) |
| `assets/` | app icons (`icon.icns`, `icon.ico`, `icon.png`, `icon-256.png` window icon) |
| `scripts/` | packaging scripts, icon generator |

Songs are pretty JSON `Project` files with extension `.cadence`. Positions are in 16th-note steps
(16 per bar). Every load path must call `Project::validate()`; never half-load damaged files.

## Local data and test isolation

User data (autosave `recovery/autosave.cadence`, installed packs `packs/`) lives in the platform
local data dir from `directories` (`ProjectDirs::from("studio","Cadence","Cadence")`).
Set `CADENCE_DATA_DIR=/some/dir` to override it — always do this in tests and never write to the
real user data dir from tests. `CADENCE_PACKS_DIR` (if set) overrides only the pack folder and takes
precedence, so tests should `env_remove` it. Recovery/pack tests that set the variable in-process must live in
their own test binary (one `#[test]`) or only pass it to child processes via `Command::env`.

## CLI (acceptance/automation paths, no GUI needed)

```
cadence [SONG.cadence]                 open the app, optionally with a song
cadence --render-demo OUT.wav          render the built-in demo
cadence --render SONG.cadence OUT.wav  render a song
cadence --check SONG.cadence           validate a song and print a summary
cadence --new-pack ID OUT.json         write a starter instrument pack
cadence --validate-pack PACK.json      validate a pack (reports the bad field)
cadence --install-pack PACK.json       validate + atomically install into <data dir>/packs/
cadence --list-instruments             built-in + pack instruments, pack load problems
```

Exit codes: 0 success, 1 operation failed (`error: ...` on stderr), 2 bad arguments.

## Cheap new instruments (no Rust changes, no rebuild)

New sounds are data, not code. A request like "make an 8-bit pack" should be done by writing a
JSON pack, not by editing DSP:

1. `cargo run --release -- --new-pack my-pack my-pack.json` (or copy `packs/template/` / `packs/8-bit/pack.json`).
2. Edit presets per `packs/schema.json`: up to 4 oscillators (`sine|triangle|saw|square|pulse|noise`,
   level, frequency_ratio, detune_cents, pulse_width), ADSR `envelope`, `filter_cutoff`, `bit_depth`,
   `sample_hold`, `gain`, `monophonic`. Preset ids are namespaced slugs (`my-pack.bright-lead`).
3. `cadence --validate-pack my-pack.json` until it prints `OK`.
4. `cadence --install-pack my-pack.json`, then "Reload Packs" in the app (or restart).
5. Add the instrument from the library; songs embed the full preset, so they stay portable.

To ship a pack with the app instead, add it under `packs/<name>/pack.json`, embed it in
`src/packs.rs` next to the 8-bit pack, and add/extend tests. Only touch `src/audio/` if the pack
needs a genuinely new synthesis feature (then extend the schema, validation and `schema.json` together).
