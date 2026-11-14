# Songwriting workflow

This guide walks through making a song in Cadence from the demo project to an exported file. It reflects the egui interface: a transport bar on top, an instrument sidebar on the left, a multitrack arranger, a per-track pattern editor, and a mixer.

## 1. Start a song

- On first launch Cadence opens the included demo, **Afterglow** — an 8-bar C-minor groove (Cm9 → A♭maj7 → E♭maj7 → B♭9) at 104 BPM with five tracks. **File → New from Demo** reloads it anytime; if a valid recovery copy exists, Cadence offers to restore it on launch.
- **File → New Song** (⌘/Ctrl+N) creates an *Untitled song*: 8 bars at 110 BPM, one drum track.
- **File → Open…** (⌘/Ctrl+O) opens a saved `.cadence` file; passing a filename on the command line (`cadence song.cadence`) opens it directly.
- **File → Import MIDI…** builds a project from a `.mid`/`.midi` file.
- New, Open, Import MIDI and Quit all check for unsaved changes first: **Save**, **Don't Save**, or **Cancel** (Esc/backdrop counts as Cancel — a cancelled save keeps the dialog open). The status bar shows *Unsaved changes* / *All changes saved*.

## 2. Set the song's foundation

Project-level controls set the frame everything else sits in:

- **Tempo** — 30–300 BPM (drag the value or double-click to type).
- **Swing** — delays every other sixteenth; the slider runs 0–100% of the maximum (0.6). Double-click resets to 0.
- **Song length** — a bars field in the arranger toolbar (1–128); placing a clip past the end grows the song automatically.
- **Master volume** — the final output level, with stereo peak meters beside it.
- **Metronome** and **Loop** toggles live under the **Transport** menu (loop shortcut: **L**).

## 3. Add tracks

The left **sidebar** has two tabs: **Instruments** (the library) and **Track** (the selected track's inspector). In the library, each row has **+** to add a track and **▶** to audition the sound (double-click also adds); **Edit → Add Track** does the same from the menu. Up to 32 tracks per song — the counter shows `n/32`. Each track has a name, color, instrument, volume, pan, mute/solo, and effects. A new track starts with one empty pattern and a clip at the start of the song. Tracks can be reordered, duplicated, or deleted from the sidebar.

The library also lists **JSON instrument packs** (grouped by pack, with a search field and an *Import Pack…* / *Reload* footer) — see [instrument-packs.md](instrument-packs.md).

| Instrument | Character |
| --- | --- |
| Studio drums | Eight synthesized drum voices, from deep kicks to crisp hats |
| Electric piano | Warm, bell-like electric keys with a soft tremolo |
| Analog bass | Round analog bass with a filter that opens on every note |
| Glass pluck | Bright plucked-string tones for melodies and arpeggios |
| Warm pad | Slow, wide chords from softly detuned oscillators |
| Mono lead | A clear monophonic lead with glide and vibrato for hooks |

All instruments are synthesized in-app — no samples, no binary plugins (JSON instrument packs and a future VST3/AU host are covered under Limitations).

## 4. Write patterns

Selecting a track opens its **pattern editor**. A track can hold many patterns; the pattern selector switches between them, and clips on the timeline decide *when* each pattern plays.

Pattern actions: **create**, **select**, **rename**, **duplicate**, **change length** (½, 1, 2, 3, 4, 8, 12, or 16 bars), and **clear**. ⌘/Ctrl+1 switches to the **Pattern Editor** tab, ⌘/Ctrl+2 to the **Mixer**.

### Drum tracks — step grid

- Eight drum voices — **Kick, Snare, Closed hat, Open hat, Clap, Low tom, Crash, Ride** — × sixteenth-note steps.
- Click or drag across the rows to paint hits; hits carry the current velocity. **Shift+click** cycles accent levels; **right-click** erases.
- The **Fill** button lays down a four-on-the-floor kick pattern as a starting point.
- The strip under the grid is a velocity lane — drag it to shape hit loudness.

### Melodic tracks — piano roll

- Click empty grid space to draw a note; drag empty grid space to box-select notes; drag any selected note to move the group with relative timing and pitch preserved. Pull a note's right edge to resize; **Shift+click/drag** adds or toggles selection; **right-click** erases.
- The piano-key column on the left auditions pitches; scroll and zoom cover the visible range.
- The velocity lane under the roll shapes note loudness for the selection.

Everything snaps to the 1/16 grid. (Positions imported from MIDI may sit between steps; editing snaps them back.)

## 5. Arrange clips

The **arranger** shows every track's clips on a shared timeline with horizontal zoom (**=** / **−**, or **View → Zoom In/Out**; **View → Fit Song in Arranger** fits the whole song). Clips reference patterns — place the same pattern in several clips to build verses and choruses; editing the pattern updates every clip. Drag empty lane space to box-select clips across tracks; drag any selected clip to move the group along the original lanes with relative offsets preserved, or **Alt-drag** to copy the group. Clicking an unselected clip selects only it, while clicking a selected clip keeps the group and makes it primary; **Shift+click** toggles a clip in or out of the selection, **Shift+drag** on empty lanes adds to the box, empty click clears, and double-click empty lane space places the selected pattern snapped to the grid. **Right-click** a lane or clip for duplicate, repeat, make-unique, and delete. Drag the ruler to scrub the playhead. A snap selector (**Bar / Beat / 1/16**, default Bar) controls grid snapping, and **View → Follow playhead** keeps the view on the playhead during playback.

Drag a clip's **right edge** to change how many times its pattern loops. The clip's context menu also provides a loop count, up to 32 repetitions within the arrangement limits.

Drag the **top corners** to set fade-in and fade-out lengths. These fades scale the velocities of notes starting within the fade; notes already sounding retain their level. The ramps span the entire looped clip. Shortening a clip or its pattern automatically keeps its fades within the new span. These edits participate in undo/redo.

## 6. Mix

The **mixer** (per track, plus master):

- **Volume** (0–1) and **pan** (−1…+1)
- **Mute** / **Solo** — when any track is soloed, only soloed tracks sound
- **Effects** — filter cutoff (100 Hz–20 kHz), reverb, delay, drive (0–1)

## 7. Play and iterate

Press **Space** (or **Transport → Play**) to audition; **Home** returns to the start, **Stop** resets, **L** toggles loop. The playhead follows the engine's actual position while playing. Arrow keys nudge the selection (Shift+↑/↓ moves by octave); Delete/Backspace removes it; Esc deselects. Undo/redo (**⌘/Ctrl+Z**, **⌘/Ctrl+Shift+Z** or **⌘/Ctrl+Y**) covers pattern edits, arrangement changes, and mixer moves — the Edit menu shows what each undo step is.

Cadence autosaves a recovery copy ~1.5 s after edits. If the app quits or crashes, the next launch shows a **Recovery** dialog — **Restore Work** or **Discard** — that stays until you choose (closing the app keeps the autosave for next time).

## 8. Save and export

- **File → Save** (⌘/Ctrl+S) / **Save As…** (⌘/Ctrl+Shift+S) writes a `.cadence` JSON project (see [project-files.md](project-files.md)).
- **File → Export WAV…** (⌘/Ctrl+E) renders the mix to audio on a background thread with progress (see [export.md](export.md)).
- **File → Export MIDI…** writes a Standard MIDI File; **Import MIDI…** brings one back in.
- **Help → Shortcuts & Tips** (F1) shows the full shortcut list inside the app.

## Tips

- The demo *Afterglow* is a good reference for how patterns, clips, and mixing fit together — solo tracks to hear each part.
- Filter cutoff is in Hz, not a percentage — lower values darken a track.
- Mute/solo rules are shared between playback and export, so what you hear is what you export.
- Use `cadence --render-demo out.wav` to render the demo without opening the GUI.
