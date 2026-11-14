# Instrument packs

Data-driven synthesis presets in pure JSON. No code, no samples, no rebuild:
install a pack and the app picks it up on **Reload Packs**.

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

Eight packs ship compiled in: `8-bit/` (10 chiptune presets) and seven
`psy-*` banks — `psy-rhythm` (24), `psy-bass` (24), `psy-keys` (16),
`psy-brass` (16), `psy-leads` (16), `psy-arps` (16), `psy-space` (16) —
138 presets, 144 sounds together with the six built-ins. The `psy-*` packs
all set `audition_note`/`audition_duration` per preset so previews and new
tracks open in a useful register.

## Where packs live

- Embedded: compiled in via `include_str!` (the `8-bit/` and `psy-*/`
  banks above). A valid user pack with the same `id` overrides the embedded
  pack wholesale.
- User: `user_packs_dir()` — `$CADENCE_PACKS_DIR`, else
  `$CADENCE_DATA_DIR/packs`, else the platform app-data `Cadence/packs` dir.
- Catalog order is deterministic: embedded slots first (user overrides keep
  their slot), then remaining user files sorted by name.
  `PackCatalog.errors` lists files/packs that failed to load.
