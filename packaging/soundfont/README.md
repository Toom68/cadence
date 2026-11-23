# The built-in General MIDI SoundFont ("Cadence GM")

Cadence ships one General MIDI SoundFont so SoundFont tracks (the 128 GM
programs in the instrument library, quick-add's SOUNDFONT rows) work right
after installing, with no setup. In the app it is listed as
**Cadence GM (built-in)**, and songs store the name `Cadence GM`, so a song
that uses it plays the same on any machine with Cadence installed.

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
all read it.

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
`CADENCE_SOUNDFONT_CACHE` overrides it), check the SHA-256, and copy the font plus
license into `DEST_DIR`. A checksum mismatch always fails, and the bad file is
deleted. `mac-package.sh`, `windows-package.ps1` and `windows-cross-package.sh` call
them and place the font at:

- macOS: `Cadence.app/Contents/Resources/soundfonts/Cadence GM.sf2`
- Windows: `<install dir>\soundfonts\Cadence GM.sf2`, and the same in the
  portable zip (the Inno Setup script installs the staged folder recursively)

If the download fails, a local packaging run prints a warning and builds
without the font. With `REQUIRE_SOUNDFONT=1`, which CI sets, a failed download
fails the build instead. The Inno Setup script also refuses to build when it
gets `/DRequireSoundFont=1` and the font is missing. After packaging, a smoke
test runs `cadence --list-instruments` and expects `Cadence GM (built-in)`.

## Developers

Run `scripts/fetch-soundfont.sh` once. After that, `cargo run` (debug, or
`--release` from inside the repo) finds the font in `.cache/soundfonts`.
To use a font from another folder, set `CADENCE_SOUNDFONT_DIR=/some/dir`;
this replaces every other bundled-font location. Tests never depend on
the font. To exercise the real file:

```sh
CADENCE_TEST_FONT="$PWD/.cache/soundfonts/Cadence GM.sf2" cargo test --test fonts -- --ignored
```

## Changing the font

Update `pin.txt` (URL, SHA-256, size, version), the license file, the
"BUNDLED SOUNDFONT" entry in `scripts/gen-third-party-notices.py` and
`THIRD-PARTY-NOTICES.txt`, and this README. Keep `SOUNDFONT_FILE=Cadence GM.sf2`:
existing songs reference that name. A replacement should keep the standard GM
program and drum-kit layout so those songs still sound right.
