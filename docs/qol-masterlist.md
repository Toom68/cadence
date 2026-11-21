# Cadence QoL masterlist

Working list of quality-of-life features. Status: `[ ]` todo · `[~]` in progress · `[x]` done.
The original batch shipped in the overnight build; the release-polish program
(shell, transport, sidebar, arranger, editor and mixer redesign) landed on top —
wording below reflects the UI as it exists now.

## Clipboard / editing correctness

- [x] Paste notes at mouse cursor in piano roll / drum grid; fallback to playhead; pasted notes selected
- [x] Paste transposes to cursor pitch: the copied shape shifts so its note nearest the pointer's row lands on it (clamped to MIDI range); drum grid pastes unshifted (rows are kit pieces, not pitch)
- [x] Quantize button in piano-roll + drum-grid toolbars: snaps selected notes (or all when nothing selected) to the Snap grid; undoable via `ops::quantize_notes`
- [x] Fix ⌘C/⌘V/⌘X never firing on desktop (egui-winit emits `Event::Copy/Paste/Cut`, not key presses)
- [x] Fix stale widget focus silently killing all editing shortcuts (`SurrenderFocusOn::Presses`)
- [x] Fix stale clip selection hijacking note copy/cut when editor is focused (focus-aware precedence, like delete/duplicate)

## Performance / playing

- [x] Computer-keyboard performance: `Z X C V B N M` white keys / `S D G H J` black keys, `Q`/`W` octave shift, `Shift` +12; auditions through the selected instrument; drum tracks map keys across kit pieces
- [x] Keyboard is a dedicated tab (⌘3 / View → Keyboard / bottom tab bar): full-area 3-octave piano for melodic tracks (click+drag glissando, held-key glow) or kit pads for drum tracks; no longer eats the bottom of the piano roll
- [x] Smart chords (BandLab-style): collapsible chord strip in the editor — 7 diatonic chords of the song key, 18-quality override, hover audition, click stamps at pointer/playhead
- [x] Strum: direction (Up/Down/Alt) + spacing (Block/1-64/1-32/1-16 note values) on chord stamps
- [x] Scale highlight: toggle tints piano-roll rows in the song key's scale
- [x] Nudge selected notes — already existed (arrows ±1 semitone/step, Shift ±12)

## Mixer / FX

- [x] Compact mixer mode v2: "Compact" toggle → fixed-height 168px strips (vertically centred, no longer stretched to panel height), 110px wide, tighter margins/gaps/controls, smaller corner radius — dense rectangles instead of tall blocks; master strip 120px; FX knobs still hidden (pedalboard covers FX)
- [x] FX pedalboard view: per-strip "FX" chip → Drive→Filter→Delay→Reverb pedal chain in engine order with knobs; honest — engine has no per-FX bypass, pedals dim at 0 amount

## Transport

- [x] Metronome: already existed (transport toggle, accented bar-1 clicks, export-clean) — agent verified + added regression test
- [x] Tap tempo: "TAP" chip in transport bar → undoable `project.bpm` edit; pure `bpm_from_taps` math + tests

## Sidebar / UI declutter

- [x] Collapsible instrument library: slim **Library** disclosure header (▸/▾) that also shows the track count (`n / 32 tracks`); session-persisted via egui temp data
- [x] Whole-sidebar collapse: « button in the tab row shrinks the panel to a 40px rail with a » expand handle — returns real width to the arranger/editor (previously only the library text folded)
- [x] Fixed sidebar width ratchet: the tab row's « button overflowed the panel by a few px, which fed back into egui's stored panel width and made it grow ~4px/frame until the 380px cap — broke every geometry test; row width now reserves space for all three controls
- [x] Full-width "**＋ Add track ▾**" on the Track tab (and the Track → Add Track menu): IN THIS SONG recents / BUILT-IN / PACKS / SOUNDFONT groups → one-click `add_track`

## Sounds / content

- [x] New pack: mallets & keys — 14 presets (kalimba, handpan, marimba, xylophone, glockenspiel, celesta, music box, gamelan, tubular bells, rhodes, wurlitzer, clavinet, + 12-piece mallet kit); embedded; **196 presets / 203 sounds total** (7 built-ins — the acoustic guitar joined the original six)

## Ops layer (pure functions, `src/gui/ops.rs`)

- [x] `ChordSpec`, `diatonic_chords`, `chord_notes`, `chord_midi_pitches`, `chord_snaps`
- [x] `stamp_chord` / `stamp_strum` / `strum_notes` / `StrumDirection`
- [x] `humanize_notes` (seeded velocity+timing) — wired: piano-roll **Humanize** button and **Edit → Humanize Notes**
- [x] `scale_velocities` (clip gain) — wired: piano-roll **Velocity −10% / +10%** buttons

## Release polish program (shipped)

- [x] App shell: menu bar (File · Edit · Track · Transport · View · Help), status bar with transient toasts, theme system (Graphite / Midnight / Paper, persisted)
- [x] Transport bar: song name, play/stop/record, position display (click swaps bars ↔ time), tempo drag/type + tap tempo, swing, master fader + stereo meters, undo/redo, dirty marker
- [x] Arranger/editor split: draggable divider (double-click maximizes the arranger, drag again restores), corner collapse buttons, ⌘/Ctrl+Shift+↑/↓ maximize each panel; segmented Pattern Editor | Mixer | Keyboard tabs
- [x] Sidebar redesign: instrument cards (color bar, description, badge, ghost preview, + add, drag to lanes), search + bank chooser + Reset, collapsible sticky sections, footer Import pack / Reload / folder, inspector sections TRACK · MIX · EFFECTS · PLAY · MANAGE
- [x] Mixer v3: palette-derived strip colors, live per-track/bus/master meters with 1.2 s peak-hold and click-to-clear clip LEDs, dB faders + pan readouts, per-strip FX pedalboard, bus route selector, Compact mode + Clear peaks toolbar
- [x] Shared wheel scheme: wheel scrolls, Shift+wheel horizontal, ⌘/Ctrl+wheel zooms at pointer, Alt+wheel row/lane heights, middle-drag pan — identical in arranger, piano roll and drum grid
- [x] Arranger ruler: four strips (loop region, marker lane, bar numbers, chord lane) with drag-loop, double-click markers, seek scrub and key-aware chord spelling
- [x] Pattern-editor header: combo rename (double-click), ⋯ menu (Rename / Clear notes / Delete pattern), Length ▾ (½–16 bars), Add to arrangement
- [x] Recording: R arms record; keyboard/pad/computer-key notes are quantized to the Snap grid and written into the selected pattern (live + step modes, note-off on key release)
- [x] Audio lanes: Import Audio File stages WAVs (≤48 MB) into the per-install audio store; clips drag/loop/fade like pattern clips
- [x] SoundFont bank: `.sf2` files in the user font folder expose all 128 GM programs; missing fonts fall back to the preset sound
- [x] Stems export: File → Export Stems… / `--render-stems` writes one WAV per audible track honoring mute/solo/bus gates
- [x] Track automation: VOL/PAN breakpoint lanes under clips (header right-click → Show automation)
- [x] MIDI recording + per-track live arpeggiator (PLAY section), glide/legato, Edit → Arpeggiator… stamped runs
- [x] Bundled fonts: Inter 4.1 (OFL) + DejaVu Sans 2.37 (Vera license) with license texts; About dialog credits

## Follow-ups noted (not blocking)

- [ ] Tap tempo creates one undo step per tap burst — could coalesce in history if spammy
- [ ] Selection mutual-exclusion: editor note selection still leaves `sel.clips` populated (copy/cut now handle it via focus; a deeper cleanup could clear stale selections at selection time)
- [ ] `SongKey::name()` still spells some roots with sharps in pickers; the arranger's chord lane is already key-aware — picker spellings are a code follow-up
- [ ] Graphite scrollbar contrast and Paper's search-field appearance flagged in the polish program; cosmetic, deferred

## Explicitly out of scope (for now)

- System-pasteboard interop (OS clipboard) — internal clipboard only
- Audio *recording* (takes) — WAV clip import/audio lanes and MIDI recording shipped; live capture remains a separate project
- VST/plugin hosting — not realistic offline
- Swing/groove timing — changes render pipeline; revisit deliberately
