# Exporting: WAV and MIDI

Cadence exports through the native **File** menu (and headlessly via CLI flags). Playback, WAV export, and MIDI export all derive from the same project data and `Project::events()` flattening, so **mute/solo state, clip placement, swing, and song length are honored identically** in what you hear and what you export — with the MIDI caveat noted below.

## Export WAV…

**File → Export WAV…** (⌘/Ctrl+E) renders the full song — every audible track through its instrument voice (built-in or embedded pack preset), mixer level/pan, and effects — to a `.wav` file via the native save dialog.

- The render runs through the **same `Renderer` as live playback** (`src/audio/`), including per-track drive/filter/fader/pan, shared reverb and tempo-synced ping-pong delay sends, master gain, and the look-ahead limiter — so the export sounds like what you hear, minus the monitor path.
- Output is **44.1 kHz, 16-bit stereo** with TPDF dither (`export_wav` in `src/audio/export.rs`).
- Only audible tracks are rendered: muted tracks are skipped, and if any track is soloed, only soloed tracks sound. Master volume applies.
- After the last bar the render continues for the **effect tail** — reverb/delay rings decay on their own, up to 12 seconds, ending early once the output has been silent for ~0.25 s. A 50 ms fade protects the hard limit.
- Rendering happens on a **worker thread** with monotonically increasing progress (0→1), so the GUI stays responsive during long songs.
- The file is streamed to a `.partial` sibling and renamed into place only on success — a failed export never leaves a truncated `.wav` at the target path.
- Notes are clipped at pattern and song boundaries, matching playback exactly.

Use this for sharing a finished mix or dropping the song into another app as audio.

### Headless rendering

The same renderer is available without the GUI:

```bash
cadence --render-demo output.wav          # the built-in demo song
cadence --render song.cadence output.wav  # any saved .cadence project
```

## Export MIDI… / Import MIDI…

**File → Export MIDI…** writes a **format-1 Standard MIDI File at 480 PPQ** (`src/midi.rs`, via `midly`):

- A conductor track carries the song name, tempo, and 4/4 time signature; each Cadence track becomes a MIDI track with its name.
- Melodic tracks get General MIDI program changes (piano→E.Piano, bass→Synth Bass, pluck→Harp, pad→warm Pad, lead→saw Lead) and round-robin channels; **drum tracks go to channel 10**. Track volume/pan export as CC7/CC10.
- **Every track is exported, including muted ones** — muting is a mix state, not song data. Pack-preset tracks export as their track instrument's GM program.
- Notes sit on the **straight grid**: swing is a playback feel and is intentionally not baked into the file. Overlapping same-pitch notes on one channel are resolved by shortening the earlier note (a MIDI channel can't retrigger a held note).

**File → Import MIDI…** accepts format-0 and format-1 files with metrical (PPQ) timing and builds a new project:

- Each (MIDI track, channel) pair containing notes becomes a Cadence track; channel 10 becomes drums mapped onto the eight kit voices. Other channels pick an instrument from the GM program, or from register (average pitch < 48 → bass, else piano) when there's no program change.
- Track names, tempo (first tempo event wins), volume/pan controllers, velocities, and note durations are carried over; positions keep full tick precision, so imported notes can sit **between** sixteenth steps — editing snaps them back to the grid.
- Dense songs are split into multiple patterns/clips automatically; the song length is derived from the last note (1–128 bars).

Import limitations (rejected or dropped, by design): **SMPTE-timecoded and format-2 files are rejected**; only the first tempo is used; time signatures other than 4/4 are ignored (Cadence is 4/4); pitch bend, SysEx, and controllers other than volume/pan are dropped; at most 32 parts and 128 bars are kept; files over 8 MB are refused.

Use MIDI to move a composition into a DAW or hardware for further production. Because MIDI carries notes rather than audio, the file reflects pattern data, not the synthesized sound, mixer, or effects.

## Caveats

- WAV export is a **mixdown only** — there is no way to export individual tracks or stems.
- MIDI export carries note data only — no mixer, effect, or synth settings; imported MIDI produces a fresh project (built-in instruments, swing 0), not the original mix.
- If no audio device is present, playback is unavailable but export still works — rendering is offline DSP, not a loopback of the sound card.
