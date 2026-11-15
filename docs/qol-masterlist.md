# Cadence QoL masterlist

Working list of quality-of-life features. Status: `[ ]` todo · `[~]` in progress · `[x]` done.
All items below shipped in the overnight build (release verified, app relaunched).

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

- [x] Collapsible instrument library: slim disclosure header (▸/▾), collapsed shows "N instruments — click to expand"; session-persisted via egui temp data
- [x] Whole-sidebar collapse: « button in the tab row shrinks the panel to a 30px rail with a » expand handle — returns real width to the arranger/editor (previously only the library text folded)
- [x] Fixed sidebar width ratchet: the tab row's « button overflowed the panel by a few px, which fed back into egui's stored panel width and made it grow ~4px/frame until the 380px cap — broke every geometry test; row width now reserves space for all three controls
- [x] Quick-add "+ Track" on the Track tab: IN THIS SONG recents / BUILT-IN / PACKS groups → one-click `add_track`

## Sounds / content

- [x] New pack: mallets & keys — 14 presets (kalimba, handpan, marimba, xylophone, glockenspiel, celesta, music box, gamelan, tubular bells, rhodes, wurlitzer, clavinet, + 12-piece mallet kit); embedded; **196 presets / 202 sounds total**

## Ops layer (pure functions, `src/gui/ops.rs`)

- [x] `ChordSpec`, `diatonic_chords`, `chord_notes`, `chord_midi_pitches`, `chord_snaps`
- [x] `stamp_chord` / `stamp_strum` / `strum_notes` / `StrumDirection`
- [x] `humanize_notes` (seeded velocity+timing) / `scale_velocities` (clip gain) — ops ready, no UI wired yet (future: humanize button)

## Follow-ups noted (not blocking)

- [ ] Humanize/clip-gain ops exist but have no UI yet — add buttons (e.g. in chord strip menu or Edit menu) if wanted
- [ ] Tap tempo creates one undo step per tap burst — could coalesce in history if spammy
- [ ] Status-bar messages still subtle — consider a more visible confirmation (toast/flash) if missed feedback continues
- [ ] Keyboard audition is fixed-length (~0.45s), no hold-to-sustain — engine note-off would enable it
- [ ] Selection mutual-exclusion: editor note selection still leaves `sel.clips` populated (copy/cut now handle it via focus; a deeper cleanup could clear stale selections at selection time)

## Explicitly out of scope (for now)

- System-pasteboard interop (OS clipboard) — internal clipboard only
- Audio-clip tracks / recording — largest scope, separate project
- VST/plugin hosting — not realistic offline
- Swing/groove timing — changes render pipeline; revisit deliberately
