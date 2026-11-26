# The built-in SoundFonts ("Cadence GM" and "Cadence Harmonium")

Cadence ships two SoundFonts, both listed as "(built-in)", read-only, and
protected from user fonts with the same name:

- **Cadence GM**: a General MIDI SoundFont, so SoundFont tracks (the 128 GM
  programs in the instrument library, quick-add's SOUNDFONT rows) work right
  after installing, with no setup (below).
- **Cadence Harmonium**: a real sampled harmonium, offered as the
  "Harmonium" instrument (see [The harmonium](#the-harmonium-cadence-harmonium)).

In the app the GM font is listed as **Cadence GM (built-in)**, and songs
store the name `Cadence GM`, so a song that uses it plays the same on any
machine with Cadence installed.

| | |
|---|---|
| Font | **GeneralUser GS v2.0.3** by S. Christian Collins |
| License | GeneralUser GS License v2.0 ([`GeneralUser-GS-LICENSE.txt`](GeneralUser-GS-LICENSE.txt), verbatim) |
| Source | <https://github.com/mrbumpy409/GeneralUser-GS> (the author's repository; it exists "to offer an easier source for automated packaging") |
| Pinned URL | `https://raw.githubusercontent.com/mrbumpy409/GeneralUser-GS/684543d5e5efaef08d02be50dcda8d552478fa60/GeneralUser-GS.sf2` |
| SHA-256 | `9575028c7a1f589f5770fccc8cff2734566af40cd26ed836944e9a5152688cfe` |
| Size | 32,319,396 bytes (30.8 MB); 287 presets (GM + GS variations), 13 drum kits |
| Installed as | `Cadence GM.sf2` + `GeneralUser-GS-LICENSE.txt` in `soundfonts/` |

The machine-readable pin is [`pin.txt`](pin.txt). The scripts, the CI cache key
and a unit test (`font::tests::packaging_pin_installs_the_builtin_font_name`)
all read it. The harmonium's pin is [`harmonium-pin.txt`](harmonium-pin.txt)
(`font::tests::packaging_pin_installs_the_harmonium_font_name`).

## Licensing

The license allows use "without restriction for your own music creation,
private or commercial", and says: "Please feel free to use it in your
software projects, and to modify the SoundFont bank or its packaging to suit
your needs." Bundling it in a sold app installer is allowed. Its terms are:

- Ship the license text with the font. It goes in `soundfonts/` next to the
  font and in `licenses/`, and its text is in `THIRD-PARTY-NOTICES.txt`
  (Help → About → Third-Party Notices), which
  `scripts/gen-third-party-notices.py` regenerates.
- "Please do not link directly to my download files": that is about the
  author's website. We download from a pinned commit of the GitHub
  repository, which the author provides for automated packaging, and users
  never get a link to a download.
- The sample-provenance note in the license (some samples came from freely
  available banks, and none from commercial libraries) is the author's
  statement. Read it before release, as with any third-party asset.

## Why this font

| Candidate | License | Size (.sf2) | Verdict |
|---|---|---|---|
| **GeneralUser GS 2.0.3** | GeneralUser GS License v2.0 (free use in software, commercial OK) | **31 MB** | **Chosen**: under the ~35 MB budget, actively maintained, sounds well above its size class, standards-compliant and plays well in rustysynth |
| MuseScore_General 0.2 | MIT | 206 MB (the 38 MB `.sf3` uses Ogg samples, and rustysynth only reads `.sf2`) | Too big |
| FluidR3_GM | MIT | ~141 MB | Too big, and older, plainer sound |
| TimGM6mb | GPL-2.0 | ~6 MB | Copyleft is a poor fit for a bundled asset in a sold app, and it sounds weaker |
| Arachno SoundFont 1.0 | Freeware, "primarily for private, non-commercial purposes" | ~136 MB | Not cleared for commercial redistribution |

## Measured with Cadence's loader (release build, Apple Silicon)

`cadence --list-instruments` with the font against an empty bundled dir:

- Parse time is about 20–50 ms warm (the first cold-disk run took about 1.2 s). It
  runs on the UI/loader thread, never the audio thread. Parsed bundled fonts
  are cached for the life of the process, so when the user font folder
  changes, the 31 MB file is not parsed again.
- Peak RSS is 44 MB with the font and 9.5 MB without, so the font adds about
  33 MB (rustysynth keeps the 16-bit sample data in memory).

## How the font gets into the app

The `.sf2` is **never committed**. It is fetched at packaging time:

```sh
scripts/fetch-soundfont.sh [DEST_DIR]          # macOS / Linux / Git Bash
pwsh scripts/fetch-soundfont.ps1 [-Dest DIR]   # Windows
```

Both scripts download the pinned URL once into `.cache/soundfonts/` (gitignored;
`CADENCE_SOUNDFONT_CACHE` overrides it), check the SHA-256, build
`Cadence Harmonium.sf2` next to it (see [How it is built](#how-it-is-built)), and
copy both fonts plus their licenses into `DEST_DIR`. A checksum mismatch always
fails, and the bad file is deleted. `mac-package.sh`, `windows-package.ps1` and
`windows-cross-package.sh` call them and place the fonts at:

- macOS: `Cadence.app/Contents/Resources/soundfonts/Cadence GM.sf2` and
  `.../Cadence Harmonium.sf2`
- Windows: `<install dir>\soundfonts\Cadence GM.sf2` and
  `<install dir>\soundfonts\Cadence Harmonium.sf2`, and the same in the
  portable zip (the Inno Setup script installs the staged folder recursively)

If the download or the harmonium build fails (offline, or no Python 3), a
local packaging run prints a warning and builds without the fonts. With
`REQUIRE_SOUNDFONT=1`, which CI sets, the build fails instead. The Inno Setup
script also refuses to build when it gets `/DRequireSoundFont=1` and either
font is missing. After packaging, a smoke test runs `cadence --list-instruments`
and expects `Cadence GM (built-in)` and `Cadence Harmonium (built-in)`.

## Developers

Run `scripts/fetch-soundfont.sh` once (it needs `python3` for the
harmonium). After that, `cargo run` (debug, or `--release` from inside the
repo) finds both fonts in `.cache/soundfonts`.
To use a font from another folder, set `CADENCE_SOUNDFONT_DIR=/some/dir`;
this replaces every other bundled-font location. Tests never depend on
the font. To exercise the real file:

```sh
CADENCE_TEST_FONT="$PWD/.cache/soundfonts/Cadence GM.sf2" cargo test --test fonts -- --ignored
```

## Changing the fonts

For the GM font, update `pin.txt` (URL, SHA-256, size, version), the license file, the
"BUNDLED SOUNDFONTS" entry in `scripts/gen-third-party-notices.py` and
`THIRD-PARTY-NOTICES.txt`, and this README. Keep `SOUNDFONT_FILE=Cadence GM.sf2`:
existing songs reference that name. A replacement should keep the standard GM
program and drum-kit layout so those songs still sound right.

For the harmonium, change `scripts/build-harmonium-sf2.py` or the sample list in
`harmonium-pin.txt`, rebuild with `--force --unpinned`, run the ignored
`harmonium_font` test, then put the new hash and size in `HARMONIUM_SHA256` /
`HARMONIUM_BYTES` and in the table above. Keep `HARMONIUM_FILE=Cadence Harmonium.sf2`
(songs reference the name) and the attribution in `Cadence-Harmonium-LICENSE.txt`,
in the "BUNDLED SOUNDFONTS" entry of `scripts/gen-third-party-notices.py`, and in
`THIRD-PARTY-NOTICES.txt`.

## The harmonium ("Cadence Harmonium")

A real harmonium, sampled note by note, that plays as one instrument: the
**Harmonium** row in the instrument library (the SAMPLED section of the All
and SoundFont banks; search "harmonium", "reed" or "drone") and the first row
of quick add's SOUNDFONT group. Tracks store `{"name": "Cadence Harmonium",
"program": 20}` (GM 20 is Reed Organ, so MIDI export maps it sensibly); the
font has one preset, so every program plays the harmonium. When the font is
missing (a dev build that never ran the fetch script) the row is simply not
offered, and songs that use it fall back to their preset/built-in sound.

| | |
|---|---|
| Recording | "Harmonium Samples - All Keys and Drones" by **Donya Quick**, recorded in the Euterpea Studio, Yale University — <https://freesound.org/people/donyaquick/sounds/330410/>, **CC0 1.0** |
| Edited samples | **tonejs-instruments** by Nicholaus P. Brosowsky — <https://github.com/nbrosowsky/tonejs-instruments>, `samples/harmonium/*.wav` at commit `622c2f1c32c8cfce4158ddc3eb26e518ddef37e5`; README: "Samples: [CC-by 3.0](https://creativecommons.org/licenses/by/3.0/)" (code is MIT); `sample-source-info.txt` names the Freesound recording above |
| License | **CC BY 3.0** ([`Cadence-Harmonium-LICENSE.txt`](Cadence-Harmonium-LICENSE.txt): attribution to both, and a note that Cadence converted the samples into a SoundFont) |
| Source files | 38 notes, C2–D5 (MIDI 36–74 except F#4), mono 16-bit 44.1 kHz WAV, 9–12 s each, ~32 MB in total; every file's SHA-256 and size is in [`harmonium-pin.txt`](harmonium-pin.txt) |
| Built font | `Cadence Harmonium.sf2`, 4,788,158 bytes, SHA-256 `ebdec9f9439955ea073510f918ecc6e1e492948b4b1bdcda359543ee1f1a79a4` (pinned as `HARMONIUM_SHA256`) |
| Installed as | `Cadence Harmonium.sf2` + `Cadence-Harmonium-LICENSE.txt` in `soundfonts/` (license also in `licenses/` and THIRD-PARTY-NOTICES) |

### How it is built

The `.sf2` is never committed. [`scripts/build-harmonium-sf2.py`](../../scripts/build-harmonium-sf2.py)
(Python 3 standard library only) builds it, and `scripts/fetch-soundfont.sh` /
`.ps1` run it after fetching the GM font:

1. Download each pinned WAV once into `.cache/soundfont-src/harmonium`
   (`$CADENCE_HARMONIUM_SRC_CACHE` overrides) and check its SHA-256 and size.
   A mismatch always fails (exit 3).
2. Per note: the root key comes from the file name. The pitch is measured by
   autocorrelation, and `chPitchCorrection` retunes each note to A440 (the
   instrument is about 2–17 cents sharp, 7 cents typical). Leading silence
   and most of the slow bellows ramp are trimmed: the sample starts where
   the note reaches 10% of its steady level, with a 5 ms fade-in.
3. Loop: in the steady part of the note, every start candidate (20 starts
   over 0.6 s) is compared against every end 0.45–1.1 s later. The score is
   the waveform difference over a ~2-period window (25–50 ms), computed on a
   16× decimated copy, plus the level mismatch between start and end and
   the level swing inside the loop. The best 12 are refined at full rate.
   The last 50 ms of the loop are then crossfaded into the 50 ms before its
   start, so the wrap continues the original waveform, and 8 guard samples
   follow the loop end. Each note keeps 1–2.5 s of audio, so the font is
   small.
4. Zones: one per note. `overridingRootKey` is the note, `sampleModes` = 1
   (loop while held), attack 5 ms, release 0.2 s, and key ranges split the
   gaps between recorded notes (C2 also covers keys 0–35, D5 covers 75–127,
   and F#4 plays the F4 sample).
5. Write the RIFF `sfbk` file (INFO with the attribution text, `smpl`, and
   `pdta` with preset "Harmonium" 0:20). All the math is integers and
   IEEE doubles, so the output is byte-for-byte reproducible. A build that
   doesn't match `HARMONIUM_SHA256` fails (exit 3), so every package ships
   the file that was tested.

Options: `--check` rebuilds in memory and compares with the existing file,
`--verbose` prints each note's loop and tuning, `--force` rebuilds even when
the cached file already has the pinned hash, and `--unpinned` writes a
non-matching build while you work on the script. The build takes about
12 s; with downloads it takes about 35 s.

### Validation

`tests/harmonium_font.rs` is ignored by default and runs in CI after the
fetch:

```sh
python3 scripts/build-harmonium-sf2.py
cargo test --test harmonium_font -- --ignored --nocapture
```

The test loads the font through Cadence's bundled-font loader and plays it
with rustysynth, the synth the engine uses. It checks:

- a held C3 for 6 s: RMS 0.0213 at 1 s and 0.0206 at 5 s;
- every recorded note at its root key, through two more seconds of
  looping: the RMS stays within 0.90–1.14× of the first loop pass, except
  D4 at 1.45×, a bellows swell inside its loop;
- that no sample-to-sample step after the loop wraps is larger than the
  first, unwrapped pass had (the largest increase is 3%), and that every
  loop-point jump in the sample data is no larger than the steps inside the
  loop;
- keys outside the recorded range (0, 24, 96, 127, and the F#4 gap) play and
  sustain;
- a full song render of the font track.
