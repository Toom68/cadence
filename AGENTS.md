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
pwsh scripts/windows-package.ps1                  # dist/Cadence-<ver>-windows-x64/Cadence.exe + .zip + -setup.exe (Inno Setup 6; REQUIRE_INSTALLER=1 in CI)
scripts/windows-cross-package.sh                  # same zip via x86_64-pc-windows-gnu on macOS/Linux (needs mingw-w64 + rustup target)
scripts/release.sh X.Y.Z "what changed"           # bump + fmt/clippy/test + commit "X.Y.Z: ..."; prints the push/tag commands
```

Always run fmt, clippy and test before finishing. `Cargo.lock` is committed; build with `--locked`.
Dependencies are pinned to known APIs (eframe/egui 0.33.3, cpal 0.16.0, rfd 0.15.4, midly 0.5,
hound 3.5, serde 1, directories 6). Do not bump major/minor versions casually: newer eframe/cpal
releases change APIs. Prefer std over adding crates.

Packaging is honest: macOS bundles are ad-hoc signed only (no Developer ID, not notarized);
Windows executables and the installer are unsigned. Do not claim otherwise.

Releases and updates: cut a release with `scripts/release.sh X.Y.Z "message" [--notes FILE] [--push]`
from a clean `main` that equals `origin/main`. It bumps `Cargo.toml` + `Cargo.lock`, runs fmt --check,
clippy and the full tests (undoing the bump if any fail), commits `X.Y.Z: message`, and prints
`git push origin HEAD:main && git tag -a vX.Y.Z -m ... && git push origin vX.Y.Z` (`--push` runs them).
The annotated tag's message (message + notes file) becomes the in-app "what's new" text (`notes` in
`latest.json`, ≤ 4 KB). Pushing a `vX.Y.Z` tag (must equal `Cargo.toml`'s version) makes CI sign
every download with minisign (secret `MINISIGN_SECRET_KEY`), verify it against
`packaging/update-key.pub`, write `latest.json`, and publish the GitHub release. The app's
updater (`src/update.rs`) only installs files whose signature trusted comment is exactly
`cadence-update <version> <file>` — keep CI and `update::trusted_comment` in step. Never commit
a secret key. `latest.json` fields beyond `version`/`assets` (`sizes`, `notes`) must stay optional; the
updater ignores unknown fields. Before starting an installer the app writes `<data dir>/update-pending.json`
so the next launch can report an update that didn't finish. Tests must not touch the network: headless `CadenceApp` refuses update checks,
and `update.rs` tests use inline fixtures signed by a throwaway key.

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
| `src/font.rs` | SoundFont (`.sf2`) instruments: user font dir, GM program names, catalog |
| `src/gui/` | egui UI (`CadenceApp`) |
| `src/update.rs` | signed in-app updates: manifest, minisign check, system-curl download, installer hand-off (GUI side: `src/gui/updates.rs`) |
| `packaging/` | buyer `README.txt`, Inno Setup script `windows/cadence.iss`, `update-key.pub`, release-notes template |
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
cadence --check-update                 ask the release feed for a newer version (network)
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
