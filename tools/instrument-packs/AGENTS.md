# Making a new instrument pack — agent guide

Turn "make an X pack" / "improve the 8-bit pack" into JSON + two CLI commands.
No Rust edits, no rebuild, no samples.

## Read only these three files

1. `packs/schema.json` — every field, type, range and default. Authoritative.
2. `packs/template/pack.json` — minimal working pack.
3. `packs/8-bit/pack.json` — real multi-preset example (skim for style).

## Write the pack (one JSON object)

- Top level: `"schema_version": 1`, slug `"id"` (e.g. `my-pack`), `"name"`,
  optional `"description"`/`"author"`, `"instruments"` array (1-128).
- Each instrument: namespaced slug `"id"` like `my-pack.lead` (**must contain
  a dot**, chars `a-z 0-9 - _ .` only), `"name"`, 1-4 `"oscillators"`, and
  optional `"description"`, `"envelope"`, `"filter_cutoff"`, `"bit_depth"`,
  `"sample_hold"`, `"gain"`, `"monophonic"`, `"audition_note"`,
  `"audition_duration"`.
- Oscillator: `"waveform"` = `sine|triangle|saw|square|pulse|noise`;
  `"level"` 0-1; `"frequency_ratio"` 0.125-16 (0.5 = octave down, 2 = up);
  `"detune_cents"` -100..100; `"pulse_width"` 0.05-0.95 fraction of cycle
  (pulse only, 0.5 = 50% duty).
- Envelope (seconds): `attack` 0-5, `decay` 0-10, `sustain` 0-1 (level),
  `release` 0.005-20.
- `bit_depth` 4-24 (lower = crunchier, 24 = clean), `sample_hold` 1-64,
  `filter_cutoff` 100-20000 Hz, `gain` 0-1, `monophonic` bool.
- `audition_note` integer 0-127 (default 60) and `audition_duration` 0.1-8 s
  (default 0.8): the library preview note/length and where a new track's
  piano roll opens. Presentation metadata only — never the rendered signal.
  Set them to the preset's real register (kicks ~24-36, bass ~28-48,
  keys/brass ~48-72, leads ~60-84, textures 2.5-4 s). Omit both for the
  defaults; older packs without them behave exactly as before.
- **Only schema fields are allowed** — unknown fields are rejected, so a typo
  like `samplehold` fails validation (that's the point: fix it).
- File max 2 MB.

## Validate and install (built binary — never rebuild for packs)

```sh
cargo run --release -- --new-pack my-pack my-pack.json   # optional scaffold
# ...edit my-pack.json...
cargo run --release -- --validate-pack my-pack.json      # must print OK
cargo run --release -- --install-pack my-pack.json       # -> <data dir>/packs/my-pack.json
```

Or use `target/release/cadence` directly once built. The user then clicks
**Reload Packs** in the app (or restarts) and adds instruments from the
library — the bank chooser and search sit above the scrolling list. Songs
embed the full preset, so they stay portable. Bundled examples: `8-bit/`
plus the seven `psy-*/` banks (128 experimental presets, all with audition
metadata).

## Improving a bundled pack (e.g. "make the 8-bit pack better")

Copy `packs/8-bit/pack.json`, edit it, and keep `"id": "eight-bit"`. On the
next catalog load your file **replaces the whole embedded pack** — no
rebuild. Keep instrument ids namespaced `eight-bit.*`. An invalid override is
rejected and the bundled pack stays as fallback, so validate before install.

## What fails validation (fix and retry)

- ids: only `[a-z0-9._-]`, instrument ids need a dot, unique across ALL packs
  (same preset id under a different pack id = duplicate, first wins).
- pack id becomes a filename: no `/`, `..`, Windows device names (`con`, `nul`).
- numbers must be finite and inside the ranges above.
- `--validate-pack` prints the exact bad field path — read it and fix.

Done when `--validate-pack` prints OK. That's the whole pipeline.
