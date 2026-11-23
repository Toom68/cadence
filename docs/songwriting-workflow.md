# Songwriting workflow

This guide walks through making a song in Cadence from the demo project to an exported file. It reflects the current interface: a menu bar and transport bar on top, an instrument sidebar on the left, the multitrack arranger in the upper panel, a tabbed editor below it (**Pattern Editor | Mixer | Keyboard**) with a draggable divider between the two, and a status bar with toasts at the bottom.

## 1. Start a song

- On first launch Cadence opens the included demo, **Afterglow** — an 8-bar C-minor groove (Cm9 → A♭maj7 → E♭maj7 → B♭9) at 104 BPM with five tracks. **File → New from Demo** reloads it anytime; if a valid recovery copy exists, Cadence offers to restore it on launch.
- **File → New Song…** (⌘/Ctrl+N) asks for a name, a song key and what to **Start from**: **Empty** (one drum track, 110 BPM), **Beat** (boom-bap drums and a bass groove, 92 BPM), **Pop song** (Verse/Chorus markers, a I–V–vi–IV chord lane — i–VI–III–VII in a minor key — with drums, bass, keys and pad, 16 bars), **Lo-fi** (78 BPM, swung, jazzy seventh chords on keys), **House** (124 BPM four on the floor, octave bass, pad) or **8-bit** (chiptune bass and keys from the bundled 8-bit pack). Templates are built in the chosen key from the same tools described below (chord lane, grooves, *Play song chords*, *Bassline from chords*) using only built-in sounds, so every part is ordinary, editable patterns and clips. The key feeds the chord strip and the Arpeggiator; it can be changed later in the **Edit → Arpeggiator…** dialog.
- **File → Open…** (⌘/Ctrl+O) opens a saved `.cadence` file; passing a filename on the command line (`cadence song.cadence`) opens it directly.
- **File → Import MIDI…** builds a project from a `.mid`/`.midi` file.
- New, New from Demo, Open, Import MIDI and Quit all check for unsaved changes first: **Save**, **Don't Save**, or **Cancel** (Esc/backdrop counts as Cancel — a cancelled save keeps the dialog open). The transport bar shows *Unsaved* / *Saved* next to the undo buttons and the status bar confirms saves.

## 2. Set the song's foundation

The transport bar holds the project-level controls:

- **Song name** — the text field on the left; also the default file name.
- **Position display** — bar.beat.sixteenth plus elapsed/total time; click it to swap which one is large.
- **Tempo** — 30–300 BPM (drag the value or double-click to type). The **Tap** button beside it sets the tempo from at least three taps as an undoable edit.
- **Swing** — delays every other sixteenth; the slider runs 0–100% of the maximum (0.6). Double-click resets to 0.
- **Song length** — a bars field with −/+ in the arranger toolbar (1–128); placing a clip past the end grows the song automatically.
- **Master volume** — the final output level, with stereo peak meters beside it that latch a clip warning.
- **Metronome**, **Loop** and **Record** — icon buttons in the transport bar, mirrored by the **Transport** menu (loop shortcut: **L**, record: **R**).

## 3. Add tracks

The left **sidebar** has two tabs — **Instruments** (the library) and **Track** (the selected track's inspector) — and collapses to a 40 px icon rail. In the library:

- The **Library** header folds the list and shows the track count (`n / 32 tracks`). Each sound is a card with a color bar, name, one-line description and a badge (KIT / PACK / FONT); its play button auditions the sound, **+** adds it as a track, double-click adds too, and cards drag straight onto the arranger lanes.
- A **search** field ("Search sounds…") and a **bank chooser** filter the list — All, Built-in, one section per installed pack, and the SoundFont bank's 16 GM families. **Reset** appears only while a filter is active; sections collapse, and stay open while searching.
- The footer has **Import pack…**, **Reload** and a folder button (open the packs folder); the SoundFont bank plays the built-in General MIDI font **Cadence GM (built-in)** out of the box and offers **Open SoundFonts folder** for more — drop `.sf2` files there and they appear within a couple of seconds (**Add a SoundFont…** appears only if no font is available at all).
- The Track tab's **Add track** button opens the same grouped chooser inline (IN THIS SONG / BUILT-IN / PACKS / SOUNDFONT); **Track → Add Track** does it from the menu, including an **Audio Track** lane for imported WAV clips.

Up to 32 tracks per song. Each track has a name, color, sound, volume, pan, mute/solo, effects and an output route. A new track starts with one empty pattern and a clip at the start of the song. Reorder, duplicate or delete tracks from the inspector's MANAGE section, the header's right-click menu, or the **Track** menu.

The seven built-in instruments:

| Instrument | Character |
| --- | --- |
| Studio drums | Eight synthesized drum voices, from deep kicks to crisp hats |
| Electric piano | Warm, bell-like electric keys with a soft tremolo |
| Analog bass | Round analog bass with a filter that opens on every note |
| Glass pluck | Bright plucked-string tones for melodies and arpeggios |
| Acoustic guitar | Physical-modeling steel-string guitar |
| Warm pad | Slow, wide chords from softly detuned oscillators |
| Mono lead | A clear monophonic lead with glide and vibrato for hooks |

Beyond the built-ins, tracks can play bundled or user **JSON pack presets**, a **SoundFont** program (the built-in Cadence GM font or `.sf2` files in the user font folder — sampled GM sounds that fall back to the track's preset sound when the font isn't installed), or an imported **WAV sample** (**File → Import Sample…** embeds the clip as a `sample.*` preset). All instruments are synthesized or sampled in-app — no binary plugins (see Limitations in the README).

## 4. Write patterns

Selecting a track opens its **pattern editor**. A track can hold many patterns; the pattern selector combo ("name · length") switches between them, and clips on the timeline decide *when* each pattern plays. **Double-click the combo** (or ⋯ → Rename pattern) to rename inline — Enter commits, Esc cancels. The ⋯ menu also holds **Clear notes** and **Delete pattern**; **Length ▾** offers ½, 1, 2, 3, 4, 8, 12 or 16 bars; **＋** and the duplicate button make new patterns; **Add to arrangement** stamps the current pattern at the playhead. ⌘/Ctrl+1 switches to the **Pattern Editor** tab, ⌘/Ctrl+2 to the **Mixer**, ⌘/Ctrl+3 to the **Keyboard**.

### Drum tracks — step grid

- The built-in kit has eight voices — **Kick, Snare, Closed hat, Open hat, Clap, Low tom, Crash, Ride** — × sixteenth-note steps (drum-kit presets can carry up to 32 pieces).
- Click or drag across the rows to paint hits; hits carry the current velocity. **Shift+click** cycles accent levels (100% → 70% → 40%); **right-click** erases.
- Row labels show the piece name, the computer key that plays it, and a preview icon on hover.
- The **Grooves ▾** menu stamps a named groove into the current pattern: **Four on the floor**, **Backbeat rock**, **Half-time**, **Boom bap**, **Trap hats** (16ths rolling into 32nds), **House** (offbeat open hats), **Reggaeton / dembow**; these replace the pattern's hits and repeat on every bar of the pattern. **Fill last bar** rewrites only the second half of the last bar with a snare/tom fill, and **Kick on every beat** adds kicks without touching anything else. Pieces are matched to the track's kit rows by General-MIDI pitch, then by row name (a clap falls back to the snare, toms to each other); pieces the kit lacks are skipped and named in the status bar. Hit velocities follow the toolbar's velocity, and each stamp is one undo step.
- The strip under the grid is a velocity lane — drag it to shape hit loudness.

### Melodic tracks — piano roll

- Click empty grid space to draw a note of the toolbar's **Note** length, or drag from empty grid space to draw a note whose length follows the pointer (whole **Snap** cells); **Alt+drag** empty space box-selects notes (**Shift+drag** adds to the selection); drag any selected note to move the group with relative timing and pitch preserved, or **Alt+drag** it to copy the group (the copies move, one undo step). Pull a note's right edge to resize; **Shift+click/drag** adds or toggles selection; **right-click** erases.
- The piano-key column on the left auditions pitches (**⌘/Ctrl+click** a key selects every note of that pitch, **⌘/Ctrl+Shift+click** adds them); the velocity lane under the roll shapes note loudness for the selection.
- The roll toolbar holds **Note** length (1/32–1 bar; keys **1–6** pick it while the roll has focus, **Shift+1–6** give the selected notes that length) and **Snap** grid (1/32–1/4) combos, **Scale snap** (drawn notes and vertical drags land on the key's scale tones) and **Scale** (tints the key's rows), **Chord tones** (off by default: tints, chord by chord, the rows of the arranger chord-lane chord playing over each stretch of the pattern — the root strongest — so a melody can land on chord tones), **Ghosts** (the other melodic tracks' notes as faint, read-only outlines in their track colours, as they play against this pattern's clip — the one under the playhead, else its first), **Fit** (zoom to fit the pattern, also **0**; Shift+click or **Shift+0** fits the selected notes), **Quantize** and **Humanize** (also in the Edit menu), and **Velocity −10% / +10%** for the selection or the whole pattern.
- The collapsible **Chords** strip (melodic tracks only) has three tabs — **Diatonic** (the key's seven chords), **Borrowed** (parallel-key colors and secondary dominants) and **Progressions** (one-click loops stamped a chord per bar) — plus Type, Inversion, Bass and Strum controls. Hover a chip to audition; click to stamp at the pointer or playhead.
- **Play song chords on this track** (pattern ⋯ menu, or right-click the arranger's empty chord lane) writes the whole chord lane into the selected melodic track: each chord plays as a block chord until the next chord (or the song end), re-struck on every bar, using the chord strip's Inversion, Bass and Strum settings. It gets its own *Song chords* pattern and a clip from the first chord's bar to the song end, so it plays across the song. **Bassline from chords ▸** does the same for a bass part in one of four styles — **Roots** (a held root per bar), **Root–fifth**, **Eighths** or **Octave bounce** — with roots in the C2–B2 register (a chord-strip Bass degree, if set, becomes a pedal note). Running either again after editing the chord lane replaces the earlier part; clips of your own patterns are left alone. Each is one undo step; drum and audio tracks, or an empty chord lane, are refused with a note in the status bar.

Everything snaps to the 1/16 grid. (Positions imported from MIDI may sit between steps; editing snaps them back.) One shared navigation scheme works in the roll, the drum grid and the arranger: **wheel** scrolls vertically, **Shift+wheel** horizontally, **⌘/Ctrl+wheel** zooms time at the pointer, **Alt+wheel** zooms row heights (pitch rows 8–32 px, lanes 36–96 px), two-axis trackpads pan, and **middle-drag** pans both axes.

### Playing and recording notes

The **Keyboard** tab (⌘/Ctrl+3) is a playable instrument: a full-width piano on melodic tracks (click+drag glissando; computer keys `Z X C V B N M` white, `S D G H J` black, `Q`/`W` octave, `Shift` +12 — the same keys work while the pattern editor is focused) or a velocity-sensitive pad grid on drum tracks (a higher click is a louder hit). The toolbar sets octave and velocity.

Arm **Record** (the transport ● button, **R**, or Transport → Record) and notes you play are quantized to the Snap grid and written into the selected track's pattern — live while playing, or step-recorded at the playhead while stopped (it advances one step per note). Recording targets MIDI tracks; audio lanes only hold imported WAV clips. **Edit → Arpeggiator…** writes a generated chord run into the current pattern instead — pick the song key, chord, direction, rate, octaves and velocity.

#### MIDI keyboards and controllers

Plug in a MIDI keyboard (USB class-compliant devices need no driver) and play: notes sound on the selected track at the velocity you play them, on any tab and whether or not the transport is running. With **Record** armed they are recorded exactly like computer-keyboard notes — same Snap quantize, same one-undo-step take, the same count-in and step recording — with your velocity stored on each note. Holding the **sustain pedal** (CC 64) keeps recorded notes going until you lift it. A computer key and a controller key on the same pitch are tracked separately, so neither releases the other.

Choose the input in **File → Preferences… → MIDI input**: *All inputs* (the default — every connected device, including ones plugged in later), a single device, or *None*. The choice is remembered. Cadence rescans every few seconds (every second while Preferences is open), reconnects a device when it is plugged back in, and reports connects and disconnects in the status bar; unplugging ends any held notes. Live play-through uses the same short preview as the computer keyboard, so how long you hold a key affects the recorded length, not the preview. Only notes, velocity and the sustain pedal are used — pitch bend, mod wheel and other controllers are ignored.

## 5. Arrange clips

The **arranger** shows every track's clips on a shared timeline. Its toolbar holds the song-length field, the **Snap** selector (Bar / Beat / 1/16), zoom buttons and **Fit**, the **Follow** playhead toggle, a loop-region readout with ✕ while a region exists, **Marker**, **Split**, and a selection count. Horizontal zoom is **=** / **−**, **⌘/Ctrl+wheel**, or **View → Zoom In/Out**; **View → Fit Song in Arranger** fits the whole song.

Clips reference patterns — place the same pattern in several clips to build verses and choruses; editing the pattern updates every clip. Drag empty lane space to box-select clips across tracks; drag any selected clip to move the group, or **Alt-drag** to copy it. Drags also move up and down onto other lanes of the same kind (drum ↔ drum, melodic ↔ melodic, audio ↔ audio); the clip's pattern travels with it — moved when no clip left behind plays it, copied otherwise — and a lane the group can't land on is skipped. **⌘/Ctrl+V** pastes copied clips at the playhead; with the arranger focused and another track selected, the topmost clip lands on that track (relative lanes kept, falling back to the clips' own tracks when the kinds don't match). Clicking an unselected clip selects only it, while clicking a selected clip keeps the group; **Shift+click** toggles a clip, empty click clears, and double-click empty lane space places the selected pattern. Double-click a clip to open its pattern in the editor. **Right-click** a lane for *Import audio here* (audio lanes), *Paste clips here* (the topmost copied clip lands on that lane), *Place “pattern”*, *New empty pattern here*, *Add marker here*, the Structure menu, *Set song end here* and the time edits below; right-click a clip for *Open in editor*, *Split here* (B), *Duplicate after*, *Copy*, repeats, loops/fades, *Make unique copy* and *Delete*.

- Drag a clip's **right edge** to change how many times its pattern loops (up to 32). Drag the **top corners** to set fade-in/fade-out lengths — fades scale the velocities of notes starting inside them.
- The **ruler** is four strips: drag the thin top strip for a loop region (✕ on the band clears it), double-click the marker lane for a section marker (flags drag to move, double-click to rename; right-click a flag for *Rename…*, *Loop this section* — marker to the next marker or song end —, *Select section clips*, *Duplicate section after* and *Delete marker*), click/drag the bar numbers to scrub the playhead (snaps to beats; Shift for exact steps), and use the bottom **chord lane** to write the song's chord track — click adds the key's chord, chip click cycles quality, Shift+click the root, drag moves, right-click deletes. Hovering a chip auditions the chord through the selected melodic track (voiced like the chord strip); right-click the empty lane for *Play song chords on this track* and *Bassline from chords ▸*.
- **Insert / delete time**: right-click a lane or the ruler for *Insert bars at bar N ▸* and *Delete bars from bar N ▸* (1, 2, 4 or 8 bars); with a loop region set there are also *Insert silence for loop* (the loop then plays the new gap) and *Delete loop region and close gap*. Everything after the edit point shifts — pattern and audio clips, markers, chords, automation, the song end and the loop region; clips crossing an edge are cut first so the rest sounds the same. *Duplicate section after* on a marker uses the same insert, then copies the section's clips, chords and automation into the gap with a "Name 2" marker.
- **Track headers**: color strip, name, instrument subtitle, M/S chips; click selects, drag up or down to reorder the lanes (an accent line marks the drop slot), double-click opens track settings, right-click offers settings, duplicate, **Show automation**, *Track height ▸*, move up/down and delete.
- **Automation**: with a track's automation strip shown, click to add a breakpoint and drag points on its volume (×0–1.5 multiplier) or pan lane (VOL/PAN chips pick which); right-click the strip to clear the lane or hide it.
- **Audio lanes** play imported WAV clips instead of patterns — **File → Import Audio File…** or a lane's right-click menu stages a `.wav` (up to 48 MB) into the app's audio store and drops it as a clip that moves, loops and fades like pattern clips. The audio lives per-install, not in the `.cadence` file — see [project-files.md](project-files.md).
- Empty lanes hint how to fill them; dragging a sound card from the library onto the lanes inserts a new track at that spot.

## 6. Mix

The **Mixer** tab has a strip per track, four fixed subgroup **bus strips** (A–D), and the **master**:

- **Track strip**: color bar, name and instrument, M/S chips, the per-strip **FX** chip that opens a pedalboard (Drive → Filter → Delay → Reverb in engine order), an **output route** selector (Master or a bus), a **dB fader** (readout above, −∞ at the bottom), a live meter with a falling peak-hold tick and a clip LED that latches until clicked, and a **pan** knob (readout C/L*n*/R*n*).
- **Bus strips** group tracks: rename them, M/S, fader, meter and pan. Bus mute/solo gates the whole group — a soloed track on a muted bus stays silent, and soloing a bus isolates its group.
- **Master**: fader, stereo meters and a latched peak readout that flags the −1 dBFS limiter ceiling.
- Mixer toolbar: **Compact** narrows the strips, **Clear peaks** resets every hold and LED.
- Gestures: drag a fader or knob (Shift for fine), **double-click** resets to the model default (0.7 on tracks, 0.8 on buses/master), **Alt+wheel** nudges ±0.02, click a meter/peak to clear it, wheel/horizontal swipe scrolls the strip row.

The same four effects are also editable as knobs in the inspector's EFFECTS section; **Track** inspector sections cover MIX (volume/pan sliders, M/S) and PLAY (glide, and a per-track live **arpeggiator** — rate, octaves, direction — on pitched tracks).

## 7. Play and iterate

Press **Space** (or **Transport → Play**) to audition; **Home** returns to the start, **Stop** rewinds to the last seek (twice for the song start), **L** toggles loop. Arrow keys nudge the selection (Shift+↑/↓ moves by octave, Shift+←/→ by a bar; in the piano roll Alt+←/→ shortens or lengthens the selected notes by the Snap, and a transpose auditions the selection's lowest note); Delete/Backspace removes it; Esc deselects — or, mid-drag, cancels the drag and puts everything back. **⌘/Ctrl+C / X / V** copy, cut, or paste the selection — pattern notes paste at the mouse cursor when it's over the note grid (playhead otherwise), clips land on their own tracks with their left edge on the playhead, and a copied track pastes as a fresh deep copy appended at the end. Undo/redo (**⌘/Ctrl+Z**, **⌘/Ctrl+Shift+Z** or **⌘/Ctrl+Y**) covers pattern edits, arrangement changes, mixer moves and automation — the Edit menu shows what each step is, and the transport bar's undo/redo buttons sit on the right.

Cadence autosaves a recovery copy ~1.5 s after edits. If the app quits or crashes, the next launch shows a **Recovery** dialog — **Restore Work** or **Discard** — that stays until you choose (closing the app keeps the autosave for next time).

## 8. Save and export

- **File → Save** (⌘/Ctrl+S) / **Save As…** (⌘/Ctrl+Shift+S) writes a `.cadence` JSON project (see [project-files.md](project-files.md)).
- **File → Export WAV…** (⌘/Ctrl+E) renders the mix to audio on a background thread with progress; **Export Stems…** writes one WAV per audible track (see [export.md](export.md)).
- **File → Export MIDI…** writes a Standard MIDI File; **Import MIDI…** brings one back in. **Import Audio File…** stages a WAV clip onto an audio lane; **Import Sample…** turns a WAV into an instrument preset; **Import Instrument Pack…** installs a JSON pack.
- **Help → Shortcuts & Tips** (F1) shows the full shortcut list inside the app; **Help → About Cadence…** shows version and credits.

## Tips

- The demo *Afterglow* is a good reference for how patterns, clips, buses and mixing fit together — solo tracks to hear each part.
- Filter cutoff is in Hz, not a percentage — lower values darken a track.
- Mute/solo rules are shared between playback and export, and bus gates apply on top of track gates — what you hear is what you export.
- Faders read in dB; double-click any fader, knob or slider to reset its default.
- Use `cadence --render-demo out.wav` to render the demo without opening the GUI.
