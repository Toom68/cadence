# Instrument packs

Data-driven synthesis presets in pure JSON. No code, no rebuild — and sounds
travel with the song: oscillators, drum kits and (optionally) small embedded
PCM clips are all JSON fields. Install a pack and the app picks it up on
**Reload Packs**.

## Make a pack

1. Copy `template/pack.json` (or run `cargo run -- --new-pack <id> out.json`)
   and edit the JSON. `schema.json` describes every field; `8-bit/pack.json`
   is a complete real example.
2. Keep it small: `schema_version: 1`, a slug `id` like `my-pack`, a `name`,
   and 1-128 instruments. Each instrument needs a namespaced `id`
   (`my-pack.lead`), a `name`, and 1-4 `oscillators` (`waveform`, `level`,
   `frequency_ratio`, `detune_cents`, `pulse_width`). Envelope, filter, bit
   depth, sample-and-hold and gain all have defaults — omit what you don't
   need. Optional `audition_note` (MIDI 0-127, default 60) and
   `audition_duration` (0.1-8 s, default 0.8) set the library preview note
   and length and where a new track's editor opens — presentation metadata
   only, never the rendered sound. See `../docs` or ask: "make an 8 bit
   pack" style requests map directly onto this format.
3. Validate: `cargo run -- --validate-pack path/to/pack.json`
4. Install: `cargo run -- --install-pack path/to/pack.json`
   (writes `<id>.json` into the user pack dir, replacing any pack with the
   same id). Or just drop the file into the user pack directory yourself.
5. In the app: **Reload Packs**, then add the instrument from the library.

Field cheatsheet: `waveform` is `sine|triangle|saw|square|pulse|noise`;
`frequency_ratio` 0.125-16 (0.5 = octave down, 2 = octave up); `bit_depth`
4-24 (lower = crunchier, 24 = clean); `sample_hold` 1-64 (1 = off);
`pulse_width` 0.05-0.95 fraction of cycle (pulse only); `envelope` times are
seconds (`attack`/`decay`/`release`, `sustain` 0-1); `audition_note`
0-127 and `audition_duration` 0.1-8 s pick the preview note and length
(defaults 60 / 0.8 — set them to the preset's real register).

## Drum kits

A preset can be a whole drum kit instead of one melodic voice: add a `kit`
object with an ordered `pieces` array (1-32 pieces). Each piece has a display
`name` (1-64 chars), a MIDI `pitch` (0-127, unique within the kit) and a
`voice` — the same synthesis fields as a preset (`oscillators`, `envelope`,
`pitch_envelope`, `filter_envelope`, `filter_cutoff`, `filter_resonance`,
`lfo`, `bit_depth`, `sample_hold`, `gain`; every field optional with the
usual defaults, but `oscillators` needs 1-4 entries).

```json
{ "id": "my-kit.studio", "name": "Studio Kit",
  "kit": { "pieces": [
    { "name": "Kick", "pitch": 36, "voice": {
        "oscillators": [{"waveform":"sine","level":1.0,"frequency_ratio":1.0}],
        "envelope": {"attack":0.001,"decay":0.4,"sustain":0.0,"release":0.05},
        "filter_cutoff": 4000.0 } },
    { "name": "Snare", "pitch": 38, "voice": {
        "oscillators": [{"waveform":"noise","level":0.7},
                        {"waveform":"triangle","level":0.4,"frequency_ratio":1.8}],
        "envelope": {"attack":0.001,"decay":0.18,"sustain":0.0,"release":0.04},
        "filter_cutoff": 8000.0 } }
  ] } }
```

- Array order is the kit's display/row order — list pieces the way you want
  them shown (e.g. kick, snare, hats, cymbals).
- At note-on the engine plays the piece whose `pitch` matches exactly; any
  other note plays the NEAREST piece pitch (ties go to the lower pitch), so
  the whole MIDI range stays playable.
- A kit preset may omit the top-level `oscillators` entirely (they become
  inert metadata; if present they are still range-validated). `monophonic`
  does not apply to kits — pieces are one-shot polyphonic voices.
- Pair kit presets with the drum editor in the app so notes land on the
  step grid like the built-in kit.

## Samples

A preset can also carry an embedded audio clip — a `sample` block with
base64-encoded PCM instead of (or beside) oscillators:

```json
{ "id": "my-pack.piano-note", "name": "Piano Note",
  "sample": { "data_base64": "AAAA...", "sample_rate": 44100,
              "root_pitch": 60, "loop": false } }
```

- `data_base64` is mono PCM: little-endian signed 16-bit frames at
  `sample_rate`. Embedded, not referenced — a song or pack stays fully
  self-contained, so it plays on any machine without the source file.
- Decoded size is capped at 2 MiB (about 24 s of mono 44.1 kHz audio) and
  must hold a whole number of 16-bit frames; `sample_rate` 8000–192000;
  `root_pitch` 0–127 (default 60 = the note that plays at unity speed).
  `loop` (default false) loops the clip while the note is held; a one-shot
  ends when the clip does.
- Playback transposes by `2^((note - root_pitch)/12)` with linear
  interpolation, then runs the usual preset envelope/filter/bitcrush/gain
  tail, so those fields still shape the clip.
- A `sample` preset may omit `oscillators` entirely; any listed oscillators
  are still range-validated but inert. A kit piece `voice` can carry a clip
  the same way (sampled drum hits). When a preset has both `kit` and
  `sample`, the kit wins at preset level.
- The GUI's **File → Import Sample…** does this for you: pick a `.wav`,
  and it mono-mixes, caps at 2 MiB, stores the source rate and embeds the
  clip as a `sample.<name>` preset on the selected track (or a new one).
- Embedded clips count toward the 2 MB pack-file limit, so a pack with
  several clips can hit the cap quickly — prefer song-embedded presets
  (via the import) for large audio.

## Rules that bite

- Instrument ids: lowercase `a-z 0-9 - _ .`, must contain a dot
  (`eight-bit.pulse-lead`), unique across **all** loaded packs. First
  definition wins; duplicates are reported as catalog errors. The same preset
  id under a *different* pack id is always a duplicate — don't reuse it.
- **Improving a bundled pack:** give your pack the SAME `id` as a bundled one
  (e.g. `eight-bit`) and it replaces that whole embedded pack on catalog
  load — edited sounds take effect with no rebuild. Keep the embedded pack's
  slot position; an invalid override file is rejected and the bundled pack
  stays as fallback. First sorted file wins if several claim the same id.
- Pack ids become file names: no `/`, no `..`, not a Windows device name
  (`con`, `nul`, ...).
- Only schema fields are allowed — unknown fields (e.g. `samplehold`) are
  rejected at parse time so typos fail loudly. Fix the field name and
  re-validate.
- Max 2 MB per pack file, 128 instruments, 4 oscillators per preset.

## Bundled banks

Thirteen packs ship compiled in: `8-bit/` (10 chiptune presets), seven
`psy-*` banks — `psy-rhythm` (24), `psy-bass` (24), `psy-keys` (16),
`psy-brass` (16), `psy-leads` (16), `psy-arps` (16), `psy-space` (16) —
plus `drum-kits/` (4 multi-voice kits), `guitar/` (12), `neon/` (16
detailed electronic voices), `one-shots/` (12 FX) and `mallets/` (14
struck idiophones and vintage keys plus a 12-piece mallet kit): 196
presets, 203 sounds together with the seven built-ins. The `psy-*` packs
all set `audition_note`/`audition_duration` per preset so previews and
new tracks open in a useful register.

## Where packs live

- Embedded: compiled in via `include_str!` (the `8-bit/` and `psy-*/`
  banks above). A valid user pack with the same `id` overrides the embedded
  pack wholesale.
- User: `user_packs_dir()` — `$CADENCE_PACKS_DIR`, else
  `$CADENCE_DATA_DIR/packs`, else the platform app-data `Cadence/packs` dir.
- Catalog order is deterministic: embedded slots first (user overrides keep
  their slot), then remaining user files sorted by name.
  `PackCatalog.errors` lists files/packs that failed to load.
