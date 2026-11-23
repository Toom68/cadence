# Project files: save, open, recovery, format

## The `.cadence` file

Cadence songs are saved as **pretty-printed JSON** (UTF-8; a leading BOM is tolerated on load) with the `.cadence` extension, serialized with `serde` from the Rust data model in `src/model.rs`. If the chosen filename lacks the extension, `.cadence` is appended automatically. A project document has `"version"` and this shape (all musical positions are in **sixteenth-note steps**; 16 steps = 1 bar; note that `start`/`duration` are fractional `f64` to allow MIDI-imported sub-step positions):

```jsonc
{
  "version": 1,
  "id": 1234567890,          // unique project id (u64)
  "name": "My song",
  "bpm": 110,                // 30–300
  "swing": 0,                // 0–0.6
  "bars": 8,                 // 1–128
  "master_volume": 0.8,      // 0–1
  "glue": 0,                 // master glue compression 0–1 (absent = off)
  "tracks": [
    {
      "id": 1234567891,
      "name": "Pocket drums",
      "color": [184, 161, 255],  // RGB bytes
      "instrument": "drums",     // drums|piano|bass|pluck|guitar|pad|lead
      "volume": 0.75,            // 0–1
      "pan": 0,                  // -1…1
      "mute": false,
      "solo": false,
      "bus": 1,                  // 0 = master, 1–4 = subgroup bus (absent = master)
      "glide": 0,                // portamento seconds, 0–2 (absent = off)
      "arp": { "enabled": true, "rate": "sixteenth", "octaves": 2, "direction": "up" },
      "automation": null,        // or { "volume": [...], "pan": [...] } breakpoints
      "font": null,              // or { "name": "GeneralUser", "program": 0 } (.sf2)
      "audio": false,            // true = audio lane playing audio_clips, not patterns
      "audio_clips": [],         // [{ "id", "file", "name", "start", "offset", "length", "loops", "fade_in", "fade_out" }]
      "effects": {
        "cutoff_hz": 18000,      // low-pass cutoff Hz, 100–20000
        "reverb": 0.06,          // 0–1
        "delay": 0,              // 0–1
        "drive": 0               // 0–1
      },
      "eq": { "low_db": 0, "mid_db": 0, "high_db": 0 },  // ±12 dB shelf/bell/shelf (absent = flat bypass)
      "patterns": [
        {
          "id": 1234567892,
          "name": "Main groove",
          "length": 16,          // steps
          "notes": [
            { "id": 1234567893, "pitch": 36, "start": 0, "duration": 1, "velocity": 0.9 }
          ]
        }
      ],
      "clips": [
        { "id": 1234567894, "pattern_id": 1234567892, "start": 0, "loops": 1, "fade_in": 0, "fade_out": 0 }
      ],
      "preset": null        // or an embedded instrument-pack preset object
    }
  ],
  "key": { "root": 0, "mode": "minor" },   // song key (root = pitch class 0–11)
  "markers": [ { "id": 1, "name": "Verse", "step": 0 } ],     // ≤ 64
  "chord_events": [ { "id": 2, "root": 0, "quality": "minor", "step": 0 } ],  // ≤ 64
  "buses": [ { "name": "Bus A", "volume": 0.8, "pan": 0, "mute": false, "solo": false } ]  // fixed ×4
}
```

`pitch` is a MIDI note number (0–127). `velocity` is 0–1. `Note.id`, `Pattern.id`, `Clip.id`, `Track.id`, and `Project.id` are `u64`s assigned by the model's `next_id()` counter. Field names follow the serde-serialized Rust structs — if the model renames fields, the file format changes with it (the `version` field exists for future migrations).

Clip `loops` is 1–32 and defaults to 1 for older projects. Its span is the pattern length multiplied by the loop count. `fade_in` and `fade_out` are optional step lengths, defaulting to 0 and bounded by that span. They scale note velocities at note starts across the whole clip; they do not alter held-note levels. Shortening a pattern or loop count clamps affected fades so the project remains valid and saveable.

### Pack presets inside a song

A track may carry `"preset"` — a full copy of a JSON instrument-pack preset (`InstrumentPreset`). When present it **replaces the built-in sound** of the track's `instrument` (which still chooses the editor: drum grid vs. piano roll). Because the whole preset is embedded in the `.cadence` file, the song plays identically on machines where the pack isn't installed. `preset` is optional (`null`/absent = built-in instrument). See [instrument-packs.md](instrument-packs.md).

### SoundFonts and audio clips

- `"font"` (`{"name": "<sf2 file stem>", "program": 0–127}`) makes the track play a sampled GM program from a `.sf2` SoundFont. `"Cadence GM"` is the General MIDI font that ships with Cadence (listed as "Cadence GM (built-in)"), so songs using it play the same on every install; other names are user-installed fonts. If the named font isn't available, the track falls back to its preset or built-in sound — so a font song stays portable in a degraded form.
- An **audio lane** (`"audio": true`) holds `"audio_clips"` — imported WAV clips whose `file` is a staged name in the app's **audio store** (`<app data dir>/audio/<content-hash>.wav`, staged by **File → Import Audio File…**, ≤ 48 MB each). The `.cadence` file references the staged name only — the audio bytes are *not* embedded, so a song's audio lanes play silently on machines where the file was never imported — unless the song travels with its audio folder (see [Moving a song with its audio](#moving-a-song-with-its-audio)). Imported audio is resampled and can be moved, looped and faded like pattern clips; a missing file is a silent clip, not a load error — but Cadence warns rather than staying quiet: the status bar reports it on open, the clip carries a "missing file" tag in the arranger, `--check` prints a `WARNING` line (exit code stays 0), and `--render`/`--render-stems` warn on stderr. The store is never reaped automatically (another song may reference the same file), so the import path warns once it grows past 2 GB. **File → Clean Up Unused Audio…** is the explicit way to shrink it: it lists the staged WAVs not used by the open song (including its undo history), the crash-recovery autosave or any song in Open Recent, shows the space that frees, and deletes them only after you confirm. Songs elsewhere on disk that still use those files would then play those clips silent, so keep copies made with *Save Copy with Audio…* for anything you want to move or archive.

### Moving a song with its audio

**File → Save Copy with Audio…** writes the song to a new `.cadence` file *and* a sibling folder named `<file name> Audio/` (for `Demo.cadence`: `Demo Audio/`) holding every WAV the song's clips use, under the same staged names. The open song (its file and unsaved state) is not changed. Move or share the `.cadence` file together with its folder.

When a song is opened, every referenced audio file missing from this computer's store is looked up first in that sibling `<file name> Audio/` folder, then in the song's own folder. A file found there is checked (it must decode as a WAV, and a `<content-hash>.wav` name only accepts content with that hash) and **copied into the audio store**, so the song keeps playing even after the folder is moved away. Anything still missing stays a silent clip with the usual warning. (The CLI `--render`/`--check` paths read only the store.)

## Validation limits

Opening a project runs `Project::validate()` (`src/model.rs`), which checks structure, ID uniqueness, and ranges, and rejects malformed files with an error rather than loading partial data. Limits:

| Field | Limit |
| --- | --- |
| Format version | Must be `1` |
| Tracks per song | 32 |
| Bars per song | 128 |
| Patterns per track | 256 |
| Clips per track | 1,024 |
| Audio clips per track | 256 |
| Notes per pattern | 8,192 |
| Total scheduled note events | 200,000 |
| Pattern length | 1–2,048 steps (the editor offers ½, 1, 2, 3, 4, 8, 12, or 16-bar presets) |
| Section markers | 64 |
| Chord events | 64 |
| Arpeggiator octaves | 3 |
| Clip loops | 32 |
| Note duration | ≥ 1/16 step (0.0625) |
| Names | ≤ 160 characters |
| Tempo | 30–300 BPM |
| Swing | 0–0.6 |
| File size on load | 20 MB |

A clip must reference a pattern that exists on the same track. Note starts and clip starts must be finite, non-negative, and below 2,048 steps; note end may overhang the pattern length (it is clipped at playback/export).

## Saving

**File → Save** (⌘/Ctrl+S) shows the native save dialog (via `rfd`) defaulting to `<song name>.cadence`. `save_project` in `src/persistence.rs`:

1. Validates the project, then writes to a uniquely named temporary file in the destination directory, `fsync`s it, syncs the directory, and **atomically renames** it over the target — a failed save never leaves a half-written project or corrupts the original.
2. Preserves the existing file's permissions; a read-only target is reported as an error rather than silently replaced.
3. Is all-or-nothing: on any error the previous file is left untouched.

**Save As** writes to a new path chosen in the dialog. Both paths are capped by the same 20 MB project limit.

## Recovery

Cadence keeps a **local recovery copy** of the working project (`save_recovery`/`load_recovery` in `src/persistence.rs`) so unsaved changes survive crashes and accidental quits. The copy is `autosave.cadence` inside a `recovery/` folder under the platform application-data directory (via the `directories` crate) — it is *not* a portable project file. Set `CADENCE_DATA_DIR` to override the base directory (useful for tests and portable installs).

- The GUI autosaves to recovery on a debounce (~1.5 s) after edits.
- On next launch, if a **valid** recovery file exists, a **Recovery** dialog offers **Restore Work** or **Discard**. The offer stays until you choose — Escape, a backdrop click, or closing the app keeps the autosave for next time, so it can't be lost by accident.
- A damaged autosave is **never loaded**: it is moved aside to `autosave.damaged.cadence` (so it isn't re-offered every launch) and the problem is reported.
- A successful save/close clears the autosave (`clear_recovery`).
- Use **Save** for a file you can move or share.

New, Open, MIDI import and Quit all pass through an **unsaved-changes** dialog when the song is dirty: **Save**, **Don't Save**, or **Cancel** (Escape/backdrop counts as Cancel). Choosing Save on an untitled song opens the save dialog; if the picker is cancelled or the write fails, the dialog stays open so nothing is silently lost. Don't Save discards the changes and proceeds.

## Open Recent and drag and drop

**File → Open Recent** lists the last 10 songs you opened or saved (newest first, no duplicates; songs that no longer exist are hidden). Picking one goes through the unsaved-changes dialog like Open. **Clear Recent** empties the list. It is stored as plain text in `<data dir>/recent-files.txt`.

Dropping files onto the window works by type: a `.cadence` song opens and a `.mid`/`.midi` file imports as a new song (both through the unsaved-changes dialog), each `.wav` becomes an audio clip on a new audio track at the playhead, a `.json` file installs as an instrument pack and an `.sf2` file is copied into the SoundFont folder (an installed font with the same name is never overwritten). Other files are refused with a status-bar message. Only one song can be dropped at a time, and audio dropped together with a song is skipped.

## MIDI files

`.mid`/`.midi` files are handled separately from `.cadence` projects: **File → Import MIDI…** converts them into a new project via `import_midi` (`src/midi.rs`). See [export.md](export.md) for details and caveats.

## Other files the app manages

- **Packs** — user instrument packs live in `<data dir>/packs` (or `$CADENCE_PACKS_DIR`); see [instrument-packs.md](instrument-packs.md).
- **SoundFonts** — the read-only built-in font `soundfonts/Cadence GM.sf2` inside the app (`Cadence.app/Contents/Resources/` on macOS, next to `Cadence.exe` on Windows; `$CADENCE_SOUNDFONT_DIR` overrides, developer builds also read `.cache/soundfonts` filled by `scripts/fetch-soundfont.sh`), plus `.sf2` files in the user font folder (`--list-instruments` prints it; `$CADENCE_FONTS_DIR` overrides). A user font can't reuse the built-in name.
- **Audio store** — staged WAVs in `<data dir>/audio` as described above; carry them with **Save Copy with Audio…**.
- **Recent songs** — `<data dir>/recent-files.txt`, one path per line.
- `CADENCE_DATA_DIR` overrides the base app-data directory for recovery, packs and the audio store.
