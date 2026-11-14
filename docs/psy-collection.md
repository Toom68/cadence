# The psychedelic collection (`psy-*` packs)

Seven bundled packs add **128 synthesized presets** aimed at psychedelic,
trance, electronic and downtempo writing — rolling basses, squelchy leads,
wide pads, detuned stabs and a full percussion vocabulary. Everything is pure
synthesis through the same engine as the built-in instruments: no samples, no
plugins, no hidden DSP. Preset ids are namespaced (`psy-bass.psy-roll`), packs
are embedded in the app, and songs embed the full preset so they stay portable.

| pack | presets | role |
|---|---|---|
| `psy-arps` | 16 | Percussive plucks, mallets, bells and metallic tones for programmed-feeling sequences |
| `psy-bass` | 24 | Sub, rolling, pluck, acid-squelch, rubber, Reese, dub, growl and distorted bass voices |
| `psy-brass` | 16 | Synthesized brass/horn/reed ensembles plus punchy electronic stabs |
| `psy-keys` | 16 | Tine EPs, clavinet, bells, celesta, reeds and drawbar/rotary organs |
| `psy-leads` | 16 | Singable leads: squelch, vowels, sync-style chirps, bells, choirs |
| `psy-rhythm` | 24 | Standalone synthesized percussion voices (see below) |
| `psy-space` | 16 | Evolving pads, drones, sweeps and noise textures for atmosphere |

## Standouts by category

- **Bass** — `psy-bass.psy-roll` (resonant detuned-saw roller for 16th-note
  lines), `psy-bass.psy-pump` (amplitude-LFO thump for four-on-the-floor psy),
  `psy-bass.psy-slide` (per-note scoop via the pitch envelope — a pitch
  envelope, not portamento), the `acid-*` family (resonant squelch, short
  gates for classic squawk), `psy-bass.reese`/`reese-sub` (detuned beating
  pairs), `psy-bass.dub`/`dub-space` (slow filter opening + long release built
  for echo), `sub-sine`/`sub-square`/`sub-saw` for weight.
- **Leads** — `psy-leads.trance-lead` (wide detuned-saw anthem voice),
  `squelch-lead` (resonant acid lead), `sing-lead` (soft vocal-style vibrato),
  `vowel-vox` (formant drift between vowel positions), `soft-sync`/`hard-sync`
  (pitch-drop chirps on note-on — *not* oscillator sync), `dark-lead` (octave-down
  dubby scoop), `laser-lead` (narrow-pulse zap), `cosmic-lead` (slow alien
  scoop).
- **Arps** — `psy-arps.acid-pluck` (the rolling-arp workhorse: resonant saw
  with a closing filter envelope), `resonant-pluck`, `seq-bass` (short round
  bass pluck for low sequences), `glass-pluck`, `bell-toll`, `glitch-pluck`
  (bitcrushed sample-held twitch), `thump` (kick-adjacent mono pluck).
- **Keys** — `psy-keys.tine-ep`/`wurli-keys`/`soul-keys` (synthesized electric
  keys — approximations, not branded emulations or samples), `clav-stab`
  (percussive clavinet chunk for syncopated stabs), `psy-organ`/`drawbar`
  (additive Hammond-style), `dub-organ` (filter sweep baked into every note),
  `rotary` (Leslie-style wobble), `glass-bell`/`celesta`/`harpsi`.
- **Brass** — `psy-brass.detuned-stab` (±45-cent queasy stab — very
  psychedelic), `rave-stab` (supersaw brightness), `stab-classic`
  (house/garage), `acid-stab` (low resonant squelch stab), `swell-stab`
  (reverse-feel riser stab), `brass-ens`/`low-brass`/`horn-ens`/`reed-ens`
  (section-style chords and lines).
- **Rhythm** — kicks: `kick-sub`, `kick-808`, `kick-techno`, `kick-deep`,
  `kick-acid`; snares: `snare-tight`, `snare-acid`, `snare-long`, `clap`,
  `rim`; hats/cymbals: `hat-closed`, `hat-open`, `cymbal`, `tick`; toms and
  textures: `tom-low`, `tom-high`, `floor-tom`, `psy-tom`, `conga`, `bongo`,
  `metallic`, `industrial`, `shaker`, `psy-ping`.
- **Space** — `cosmic-pad`/`ultra-wide`/`choir-cloud` (wide evolving pads),
  `dark-nebula` (slow bloom from a muffled start), `deepspace` (sub-weighted
  cinematic wash), `honey-pad` (warm mid pad), `sweep-up`/`sweep-down`
  (~14 s transition drones that open/close the filter), `wind-noise`,
  `alien-drone`, `pulse-machine`, `radio-sky`, `drone-root`/`dark-drone`.

## Percussion is not a drum kit

Every `psy-rhythm` preset is a **standalone pitched instrument**, like the
other presets — there is no fixed kit mapping. Each one responds to the MIDI
note you place in the piano roll, and most have a recommended register in
their description:

- Kicks sit around **C1–C2** (MIDI 24–36): `kick-deep` wants C1 for maximum
  weight; `kick-techno` is happiest on C2 for a tight four-on-the-floor hit.
- Snare bodies sit around **E2** (MIDI 40); rims on **D2** (38); floor toms
  on **F1–A1** (29–33), high toms on **D3+** (50+); congas on **C3–E3** (48–52),
  bongos on **G3–B3** (55–59).
- Hats, cymbals, clap, shaker, metallic and industrial hits work at any pitch
  (velocity shapes level); `tick` is brightest at C5 (72).

This is deliberate: you sequence each voice like a synth — pitch it, gate it,
layer it, run it through the track effects — instead of being locked to one
sampled kit. Add several `psy-rhythm` tracks (kick, snare, hats, toms) to build
your own kit, or use one percussion voice melodically.

## Brass is synthesized, not a brass library

The `psy-brass` pack contains **synthesized approximations** — resonant-filter
saw/square stacks with brass-style attacks, vibrato and filter snaps. They are
not recorded or sampled brass sections, and they do not emulate any specific
acoustic instrument or brand. Treat them as electronic brass colours: section
chords (`brass-ens`, `mid-brass`), mono lead (`brass-lead`), dark low voice
(`low-brass`), and the stab family for hits. For genuinely orchestral colour
use the pad/drone presets in `psy-space` instead.

## New modulation fields

> Requires **Cadence 1.1.0 or later** — the modulation and preview-metadata
> fields below are not understood by earlier releases.

The pack schema gained optional per-preset modulation (all validated, all with
safe defaults that keep legacy presets bit-identical):

- `pitch_envelope` — `amount_semitones` (−48…48) and `decay` seconds
  (0.005…10, T60). The note starts `amount` semitones away and glides back to
  pitch exponentially — scoops, chirps and plucks without portamento.
- `filter_envelope` — `amount_octaves` (−6…6) and `decay` (0.005…20, T60),
  applied in log2 cutoff space. Negative amounts start muffled and bloom
  open (pads, risers); positive amounts snap bright then settle.
- `filter_resonance` — 0…0.95, mapped safely onto the resonant SVF for squelch
  and whistle.
- `lfo` — `waveform` (`sine`/`triangle`), `rate_hz` (0.05…20), `pitch_semitones`
  (0…12), `filter_octaves` (0…4), `amplitude` (0…1 tremolo depth). Starts
  deterministically at note-on. There is **no tempo sync** — rates are plain
  Hz — and no arpeggiator anywhere in the engine.
- Per-oscillator `decay` (0.005…20 s, optional) lets one layer (e.g. a noise
  click) die fast while the body rings on.

## Preview metadata (`audition_note`, `audition_duration`)

Each preset may carry `audition_note` (MIDI 0–127, default 60) and
`audition_duration` (0.1–8 s, default 0.8). These tell the library preview and
editor where to centre the sound — e.g. kicks audition on C2, pads on a mid
chord tone. They affect **preview/editor centring only** and never change the
rendered instrument; they round-trip through saved `.cadence` files.

## Showcases, reels and the audit tool

`examples/bank_audit.rs` is the generator/QA harness for this collection:

```bash
cargo run --release --locked --offline --example bank_audit -- OUTDIR
cargo run --release --example bank_audit -- --write-songs   # songs only
```

A full run writes **only** into `OUTDIR` — audits, reels, song files and
previews. The checked-in copies under `examples/songs/` are regenerated only
by the explicit `--write-songs` mode, never by a plain audit run:

- `audit.csv` / `audit.json` — per-preset metrics (peak, RMS, DC, crest factor,
  durations) rendered through the real playback DSP with a role-appropriate
  phrase. These are **QA guards against silence, clipping and DC offsets — not
  a measure of musical quality**; judge the sounds by ear.
- `reels/psy-*.wav` + `reels/psy-*.manifest.json` — one listening reel per
  category. Each preset plays a short musical phrase (hits for percussion,
  held chords for pads, stabs plus sustains for poly voices, motifs for mono
  leads) at its `audition_note` register, separated by a clear gap. The
  manifest maps every preset id to its timestamp in the reel.
- `songs/*.cadence` + `previews/*.wav` — two original 16-bar showcase songs
  (the same files that are checked in under `examples/songs/`):
  - **Neon Circuits** — driving psychedelic/electronic, 140 BPM, A minor
    (Am7–Fmaj7–Cmaj7–G7): techno kick, rolling bass, offbeat organ stabs,
    detuned brass stabs, 16th-note acid arps, trance lead and wide pads, with
    intro/break/finale sections.
  - **Midnight Dub** — downtempo dub, 96 BPM, D minor (Dm–Bb–F–C, two bars per
    chord): 808 kick, half-time snare, dub bass with space chords, skanking
    dub-organ, soul keys, dark pad and a sparse dark lead.

Both songs embed the complete preset definitions in their tracks, so they open
and play correctly even without the packs "installed" — they *are* the packs,
frozen into the file.

`tests/psy_soundbank.rs` is the automated counterpart: it verifies all 128
presets parse with unique ids, catches duplicate synthesis definitions, and
renders every preset through the shared `render_project` DSP checking for
finite, non-silent, bounded output and envelopes that actually release.
