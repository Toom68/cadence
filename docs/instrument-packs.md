# Instrument packs

Cadence ships 7 built-in instruments plus bundled pack presets — **203 sounds in total**: thirteen banks including `8-bit` chiptune, seven `psy-*` experimental banks, drum kits, guitars, neon leads, one-shot FX, and mallets & keys (196 pack presets). You are not limited to them: **instrument packs** are pure-JSON synthesis presets that add new instruments without rebuilding the app. A pack describes oscillators, envelopes, filters, bit depth, sample-and-hold, and output gain — or optionally an embedded WAV `sample` block — with no executable code. (Two more sound sources live outside packs: `.sf2` SoundFonts in the user font folder, and **File → Import Sample…** which wraps a WAV as a `sample.*` preset on a track.)

The full format reference lives in [`packs/README.md`](../packs/README.md) with a JSON schema at [`packs/schema.json`](../packs/schema.json) and a working example at [`packs/8-bit/pack.json`](../packs/8-bit/pack.json). A step-by-step agent guide for authoring packs (schema fields, validation, install pipeline) lives at [`tools/instrument-packs/AGENTS.md`](../tools/instrument-packs/AGENTS.md).

## Make and install a pack

```bash
cadence --new-pack my-synths pack.json   # write a starter template
# ...edit pack.json per packs/schema.json...
cadence --validate-pack pack.json        # check it; reports the first bad field
cadence --install-pack pack.json         # install into the user pack folder
cadence --list-instruments               # list built-in + pack instruments
```

Or skip the CLI: in the app, the instrument library's packs footer has **Import Pack…** (validates and installs a `.json` file) and **Reload**. Cadence also re-reads installed packs automatically about every 2 seconds, and pack load problems are listed there with a click for details.

## Using pack instruments

- Installed pack instruments appear in the sidebar's instrument library alongside the seven built-ins, bundled presets, and the SoundFont bank's 128 GM programs — each under its own collapsible section. The **bank chooser** above the list filters to **All**, **Built-in**, a single pack bank, or SoundFont families and shows per-bank sound counts; the **search** field matches instrument names, descriptions, and bank names (✕ clears it; **Reset** reappears while a filter is active). Each instrument is a card — color bar, name, description, a badge (KIT / PACK / FONT), a ghost play button to audition it, and **+** to add it as a track; double-clicking the card adds too, and cards drag straight onto the arranger lanes as new tracks.
- A preset may declare `audition_note` (MIDI note 0–127, default 60) and `audition_duration` (0.1–8 seconds, default 0.8). The library preview plays at that note and length, and adding the preset centers an empty piano roll on that register — a bass opens around C2 rather than C5. These fields are presentation metadata only: they never change the rendered sound, older packs without them keep the 60/0.8 defaults, and they round-trip into saved `.cadence` files with the embedded preset.
- A track's inspector (sidebar **Track** tab) can switch its sound between built-ins and any installed preset.
- The preset is **embedded in the `.cadence` file** when you save, so the song plays the same sound even where the pack isn't installed. Installing an updated pack does **not** change existing songs — each song keeps its embedded copy until you open the track inspector and choose *Update sound from installed pack*.
- `instrument` on a pack track still selects the editor (drum step grid vs. piano roll); the preset supplies the sound.
- A preset may instead be a **drum kit**: a `kit.pieces` list maps named one-shot voices to MIDI pitches (1-32 pieces, unique pitches, nearest pitch wins for unmapped notes). Kit presets omit the top-level `oscillators` and pair with the drum editor — see `packs/README.md` → "Drum kits".

## What a pack contains

A pack (`schema_version: 1`) has an `id` (slug, becomes the file name), a `name`, and 1–128 `instruments`. Each instrument preset needs:

- a namespaced `id` like `my-pack.lead` (lowercase `a-z 0-9 - _ .`, must contain a dot, unique across all loaded packs — a preset id reused under a *different* pack id is a duplicate: the first definition wins and the conflict is reported in the pack-errors list)
- a `name`
- 1–4 `oscillators`: `waveform` (`sine`, `triangle`, `saw`, `square`, `pulse`, `noise`), `level`, `frequency_ratio` (0.125–16; 0.5 = octave down, 2 = octave up), `detune_cents`, `pulse_width`

`envelope` (`attack`/`decay`/`release` in seconds, `sustain` 0–1), `filter_cutoff` (100–20000 Hz), `bit_depth` (4–24; lower = crunchier), `sample_hold` (1–64; 1 = off), `gain`, and `monophonic` all have defaults — omit what you don't need. The optional `audition_note`/`audition_duration` fields described above tune previews and editor centering only. Parsing is strict: unknown fields are rejected.

Optional voicing fields (all default to the classic behaviour when omitted, so older packs and songs load unchanged):

- `velocity_to_cutoff` (0–6 octaves, default 2) — velocity opens the low-pass filter: the cutoff is scaled by 2^(`velocity_to_cutoff` × (velocity − 0.8)), so velocity 0.8 plays exactly at `filter_cutoff`, harder notes are brighter and softer notes darker. 0 turns it off; a fully open filter (`filter_cutoff` 20000) is never scaled. Applies to oscillator presets, sample presets and kit pieces.

Limits: 2 MB per pack file, 128 instruments per pack, 4 oscillators per instrument.

## Where packs live

- **Embedded** — compiled into the binary (`packs/` in the source tree; currently `8-bit`, the seven `psy-*` banks, `drum-kits`, `guitar`, `neon`, `one-shots`, and `mallets` — 196 presets). A user pack file with the **same pack id** (e.g. `eight-bit`) replaces that embedded pack wholesale on the next catalog load — handy for iterating on a bundled pack without a rebuild; an invalid override is rejected and the bundled pack stays as fallback.
- **User** — `<pack-id>.json` files in the user pack directory: `$CADENCE_PACKS_DIR` if set, else `$CADENCE_DATA_DIR/packs`, else the platform app-data `Cadence/packs` folder.
- Catalog order is deterministic (embedded first, then user files by name); files that fail to load are reported as catalog errors, not silently dropped. The app re-scans the folder about every 2 seconds, so dropped-in or CLI-installed packs appear without a restart (**Reload** is the manual fallback).

## Honest scope

Packs are *presets*, not plugins: they carry synthesized definitions (and optionally a small embedded WAV `sample` block — see `packs/README.md`), but cannot run scripts, stream external audio files, or host VST3/AU binaries. With the current engine every new instrument is data-only — write the JSON, validate, install; no Rust changes or rebuild needed. For binary plugin instruments (VST3 on Windows, Audio Units on macOS), see the Limitations section of the README — that is a separate, unimplemented roadmap item.
