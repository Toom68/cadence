# Exporting: WAV, stems and MIDI

Cadence exports through the native **File** menu (and headlessly via CLI flags). Playback, WAV export, stems export, and MIDI export all derive from the same project data and `Project::events()` flattening, so **mute/solo state, clip placement, swing, and song length are honored identically** in what you hear and what you export — with the MIDI caveat noted below.

## Export WAV…

**File → Export WAV…** (⌘/Ctrl+E) renders the full song — every audible track through its instrument voice (built-in or embedded pack preset), mixer level/pan, and effects — to a `.wav` file via the native save dialog.

- The render runs through the **same `Renderer` as live playback** (`src/audio/`), including per-track drive/three-band EQ/filter/fader/pan, subgroup-bus routing, track automation, per-track arpeggiator/glide, shared reverb and tempo-synced ping-pong delay sends, master gain, the optional master glue compressor, and the look-ahead limiter — so the export sounds like what you hear, minus the monitor path. Audio lanes play their imported WAV clips in the mix too.
- Output is **44.1 kHz, 16-bit stereo** with TPDF dither (`export_wav` in `src/audio/export.rs`).
- Only audible tracks are rendered: a muted track is skipped, soloing limits the mix to soloed tracks, and the same gates apply per bus — a track on a muted or non-soloed bus stays silent. Master volume applies.
- After the last bar the render continues for the **effect tail** — reverb/delay rings decay on their own, up to 12 seconds, ending early once the output has been silent for ~0.25 s. A 50 ms fade protects the hard limit.
- Rendering happens on a **worker thread** with monotonically increasing progress (0→1), so the GUI stays responsive during long songs.
- The file is streamed to a `.partial` sibling and renamed into place only on success — a failed export never leaves a truncated `.wav` at the target path.
- Notes are clipped at pattern and song boundaries, matching playback exactly.

Use this for sharing a finished mix or dropping the song into another app as audio.

### Loudness normalization

Tick **File → Normalize WAV Export (-14 LUFS)** to have Export WAV… bring the mixdown to a target integrated loudness. It is **off by default**; the choice is remembered between launches (in the app's settings, never in the song). Stems and the CLI render paths are never normalized.

- Loudness is measured per **ITU-R BS.1770 / EBU R 128**: K-weighting (head shelf + RLB high-pass, derived for the export sample rate), 400 ms blocks with 75 % overlap, an absolute gate at -70 LUFS and a relative gate 10 LU below the ungated level (`src/audio/loudness.rs`). The effect tail is part of the measured file; silence is gated out.
- It is a **two-pass** render (`export_wav_with` in `src/audio/export.rs`): the song is rendered once without writing to measure it, then written with a gain applied **after the glue compressor and before the master limiter**. Rendering is deterministic, so both passes see identical audio.
- The **-1 dBFS ceiling always holds**: the limiter still runs after the gain. When a boost pushes the mix into the limiter, loudness rises more slowly than the gain, so up to three extra measurement passes refine the gain (secant steps) until the result is within 0.1 LU. A quiet or very peaky mix that cannot reach the target under the ceiling lands as close as it allows; boosts are capped at +24 dB so near-silence is never dragged up into hiss. A completely silent mix is written unchanged.
- The ceiling is a **sample-peak** limit; inter-sample (true) peaks of a heavily limited mix can sit slightly above -1 dBFS.
- The status line reports the measured loudness of the written file and the applied gain, e.g. `Exported WAV at -14.0 LUFS (+4.2 dB)`. Expect a normalized export to take roughly 2–5× as long as a plain one.

## Export Stems…

**File → Export Stems…** renders **every audible track to its own WAV** in a folder you pick (`export_stems` in `src/audio/export.rs`):

- Each stem is a full render through the playback DSP with **only that track audible** — the same signal it contributes to the mixdown, including its strip (drive/filter/fader/pan), its share of the reverb/delay sends, and the master chain. One shared SoundFont catalog is parsed once per batch.
- File names are `<track name>-NN.wav` — the sanitized track name plus its two-digit track number, so duplicate names can't collide.
- Audibility matches the mixdown exactly (`stem_skip_reason`): the track's bus gate is checked first (a muted bus silences its whole group, a soloed bus isolates its group), then the track's own mute/solo. Skipped tracks produce no file; if nothing is audible the export fails with an explanation instead of writing silence.
- Same output format and tail handling as the mixdown: 44.1 kHz, 16-bit stereo, effect tail included. Renders run on a worker thread with a single progress bar across the batch.

Use stems when a song needs per-track mixing or remixing in a DAW.

### Headless rendering

The same renderer is available without the GUI:

```bash
cadence --render-demo output.wav            # the built-in demo song
cadence --render song.cadence output.wav    # any saved .cadence project
cadence --render-stems song.cadence out/    # one WAV per audible track
```

## Export MIDI… / Import MIDI…

**File → Export MIDI…** writes a **format-1 Standard MIDI File at 480 PPQ** (`src/midi.rs`, via `midly`):

- A conductor track carries the song name, tempo, and 4/4 time signature; each Cadence track becomes a MIDI track with its name.
- Melodic tracks get General MIDI program changes (piano→E.Piano, bass→Synth Bass, pluck→Harp, guitar→Acoustic Guitar, pad→warm Pad, lead→saw Lead) and round-robin channels; **drum tracks go to channel 10**. Track volume/pan export as CC7/CC10.
- **Every track is exported, including muted ones** — muting is a mix state, not song data. Pack-preset and SoundFont tracks export as their track instrument's GM program; audio lanes carry no note data and produce an empty MIDI track.
- Notes sit on the **straight grid**: swing is a playback feel and is intentionally not baked into the file. Overlapping same-pitch notes on one channel are resolved by shortening the earlier note (a MIDI channel can't retrigger a held note).

**File → Import MIDI…** accepts format-0 and format-1 files with metrical (PPQ) timing and builds a new project:

- Each (MIDI track, channel) pair containing notes becomes a Cadence track; channel 10 becomes drums mapped onto the eight kit voices. Other channels pick an instrument from the GM program, or from register (average pitch < 48 → bass, else piano) when there's no program change.
- Track names, tempo (first tempo event wins), volume/pan controllers, velocities, and note durations are carried over; positions keep full tick precision, so imported notes can sit **between** sixteenth steps — editing snaps them back to the grid.
- Dense songs are split into multiple patterns/clips automatically; the song length is derived from the last note (1–128 bars).

Import limitations (rejected or dropped, by design): **SMPTE-timecoded and format-2 files are rejected**; only the first tempo is used; time signatures other than 4/4 are ignored (Cadence is 4/4); pitch bend, SysEx, and controllers other than volume/pan are dropped; at most 32 parts and 128 bars are kept; files over 8 MB are refused.

Use MIDI to move a composition into a DAW or hardware for further production. Because MIDI carries notes rather than audio, the file reflects pattern data, not the synthesized sound, mixer, or effects.

## Caveats

- Stems are rendered **with the mix chain applied** (each stem includes its send share of reverb/delay and the master chain) — they sum back to the mix rather than to dry, pre-fader signals.
- MIDI export carries note data only — no mixer, effect, or synth settings; imported MIDI produces a fresh project (built-in instruments, swing 0), not the original mix. Audio clips do not export to MIDI.
- If no audio device is present, playback is unavailable but export still works — rendering is offline DSP, not a loopback of the sound card.
