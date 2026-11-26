# Acceptance checklist

Two parts: **Verified** items have automated-test or native-run evidence (see [verification.md](verification.md) for the details and test counts) and are checked off. **Manual checks** are a ~15-minute human pass on a real build — intentionally left unchecked rather than claimed.

## Verified by automated tests / native run

- [x] `cargo fmt --all -- --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean on every polish-program branch.
- [x] `cargo test --locked --all-targets` green — see [verification.md](verification.md) for the current counts.
- [x] Persistence, recovery, CLI, render/WAV and stems, MIDI interop, SoundFont catalog, audio-clip store, and pack-pipeline integration tests (`tests/`).
- [x] Headless GUI tests (`src/gui/tests.rs`): drum grid paint/erase/drag/accents, piano-roll draw/move/resize, marquee select + delete, velocity lane, arrow-key nudge, arranger clip drag + double-click placement, group copy, clip loop/fade handles, automation strips, chord lane, markers, mixer meters/peak latching/compact mode, record-to-pattern, audio-lane hits, unsaved-changes guard, toasts, dialogs, save/open and MIDI/WAV round-trips.
- [x] Selection/input coverage: plain/reverse/additive box select, cross-track group moves, Shift-click removal, Alt-copy metadata, one undo per multi-frame gesture, no accidental notes or dirty state.
- [x] Pack preset audit: every bundled preset validates, catalogs, and renders finite non-silent audio; showcase songs validate; the 8-bit presets remain byte-identical renders.
- [x] `--render-demo` produces a non-empty 44.1 kHz stereo WAV; packaged macOS builds were visually inspected and looped real CPAL playback.
- [x] Release artifacts build on both paths: `scripts/mac-package.sh` (`dist/Cadence.app` + zip + dmg, ad-hoc signed) and both Windows paths (`windows-package.ps1` MSVC, `windows-cross-package.sh` GNU — PE32+ GUI exe, system imports only, unsigned, CRC-ok).

## Manual checks still to run (~15 minutes per platform)

Run these on a release build of the machine you ship. **Windows has had a first native run only** — the unsigned MSVC `dist/Cadence-*-windows-x64/Cadence.exe` launched on Windows 11, opened the demo with audio ready and closed cleanly, but no human has done this full pass on Windows yet; the checklist below is that pass.

### Launch & shell (~2 min)

- [ ] Window opens titled "Cadence" with the app icon; demo song *Afterglow* (104 BPM, five tracks, section markers) plays on Space.
- [ ] Menu bar reads File · Edit · Track · Transport · View · Help; every menu opens; toasts appear bottom-right for saves/exports.
- [ ] Recovery offer appears only when a valid autosave exists; Restore Work / Discard behave as documented.
- [ ] Drag and double-click the arranger/editor divider; collapse buttons and ⌘/Ctrl+Shift+↑/↓ maximize and restore panels; window usable at minimum size (~1100×760).

### Transport & mixing (~3 min)

- [ ] Play/pause/stop/record buttons, position display click-swap, tempo drag/type, tap tempo (3+ taps), swing, master fader + stereo meters all respond; metronome and loop toggle audibly.
- [ ] Sidebar: Instruments/Track segmented tabs, 40 px collapse rail, library cards (ghost preview, + add, drag to lanes), search ✕, bank chooser, Reset, collapsible sections, footer Import pack / Reload / folder buttons.
- [ ] Track inspector sections TRACK / MIX / EFFECTS / PLAY / MANAGE all fold and edit; pan double-click centers.
- [ ] Mixer: per-strip meters and peak-hold ticks move live, clip LEDs latch until clicked, fader dB readouts, pan readouts (C/L/R), FX chip pedalboard, bus route selector, bus strips A–D, master peak box; Compact and Clear peaks toolbar buttons; double-click resets, Alt+wheel nudges.

### Writing & arranging (~5 min)

- [ ] Pattern editor: segmented Pattern Editor / Mixer / Keyboard tabs; rename via double-click or ⋯ menu; Length ▾ (½–16 bars); New/duplicate; ⋯ → Clear notes / Delete pattern; Add to arrangement stamps a clip.
- [ ] Drum grid paint/drag/Shift-accent/right-erase, key badges, Grooves ▾ (named grooves + Fill last bar), velocity lane; drum-kit presets show their own pieces.
- [ ] Piano roll click/drag-to-draw length, Alt+drag box-select / copy-drag, move/resize/erase, 1–6 note lengths, 0 / Shift+0 fit, Ghosts and Chord tones toggles, key-column audition, velocity lane, Note/Snap combos, Scale snap + Scale tint, Quantize/Humanize, Velocity ±10%, off-grid count in the Quantize tooltip.
- [ ] Chords strip on melodic tracks (Diatonic / Borrowed / Progressions tabs, Type/Inversion/Bass/Strum); hidden on drum tracks.
- [ ] Keyboard tab: piano glissando or drum pads; record (R or ●) writes notes into the pattern, live and step modes.
- [ ] Held live notes: a computer key, mouse-held piano key or MIDI key (and sustain pedal) on a sustaining sound (Pad, Lead, an organ preset, a SoundFont strings program) keeps sounding while held and releases on key-up; switching track/tab, opening a dialog or clicking away from the window ends held computer/mouse notes.
- [ ] Arranger: ruler four-strip interactions (loop drag, marker double-click/rename/drag, seek, chord lane); clip marquee/move/Alt-copy/edge-loop/corner-fades; header right-click menu incl. Show automation → click/draw VOL/PAN points; Import audio here on audio lanes; library cards drop as new tracks.
- [ ] Shared navigation: wheel scrolls, Shift+wheel horizontal, ⌘/Ctrl+wheel zooms at pointer, Alt+wheel row/lane heights, middle-drag pans, trackpad pan.

### Files, dialogs & help (~3 min)

- [ ] Save / Save As / unsaved-changes dialog (Save · Don't Save · Cancel; Esc cancels); reopen restores the project.
- [ ] Export WAV and Export Stems produce playable files honoring mute/solo; Export/Import MIDI round-trips.
- [ ] Import Audio File stages a WAV onto an audio lane; Import Sample creates a `sample.*` preset; Import Instrument Pack + Reload pick up a new pack (~2 s rescan).
- [ ] Harmonium: a fresh install lists "Cadence Harmonium (built-in)"; the library's Sampled **Harmonium** row (and quick add's first SOUNDFONT row) adds a "Harmonium" track that sustains held chords without clicks or pulsing.
- [ ] SoundFont bank: a fresh install lists "Cadence GM (built-in)" and its 128 GM programs play with no setup (macOS and Windows installer + portable zip); drop another `.sf2` in the font folder → it appears alongside; a missing font falls back to the preset sound.
- [ ] Help → Shortcuts & Tips (F1) table matches the real shortcuts; About shows version and font credits (Barlow Semi Condensed, IBM Plex Mono, DejaVu Sans).
- [ ] View → Theme: Graphite / Midnight / Paper recolor the whole app and persist across relaunch.

### Install & update (~3 min, once the release key is set up)

- [ ] Windows: `Cadence-*-windows-x64-setup.exe` installs without an admin prompt; Start menu entry launches Cadence; double-clicking a `.cadence` file opens it; Settings → Apps → Cadence uninstalls cleanly (songs and settings in your user folder stay).
- [ ] macOS: the `.dmg` opens with Cadence and an Applications shortcut; dragged to Applications, it launches after right-click → Open.
- [ ] Help → Check for Updates… on the newest version says "Cadence is up to date"; offline it explains it couldn't reach the server (Try Again / Download Page).
- [ ] With an older installed version: the status bar shows "Update x.y.z available"; Download & Install verifies and shows Restart & Update; with unsaved changes the Save prompt appears first; Cadence reopens on the new version (Help → About).
- [ ] Install When I Quit installs on the next quit without reopening; Skip This Version hides the button until a newer release; unticking "Check for updates automatically" stops the launch-time check.
- [ ] The portable zip copy offers Open Download Page instead of installing.

### Known limitations to confirm (not bugs)

- Sounds are built-in synths, JSON pack presets, `.sf2` SoundFonts, or embedded WAV samples — no VST3/AU/CLAP hosting.
- No audio *recording*; MIDI recording from the computer keyboard/Keyboard tab exists. WAV clips can be imported onto audio lanes.
- Sixteenth-note grid snap; no freehand timing.
- Audio clips live in the per-install audio store; `.cadence` files reference them by name, not content.
- Packages are unsigned (macOS ad-hoc, Windows no Authenticode) — expect platform security prompts when distributing.
