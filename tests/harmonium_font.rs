//! Checks the real "Cadence Harmonium.sf2" built by
//! `scripts/build-harmonium-sf2.py` (never committed). Ignored by default;
//! run it after building the font:
//!
//! ```sh
//! python3 scripts/build-harmonium-sf2.py
//! cargo test --test harmonium_font -- --ignored
//! ```
//!
//! `CADENCE_TEST_HARMONIUM=/path/to/Cadence Harmonium.sf2` points it at
//! another copy. The font is loaded through Cadence's own bundled-font loader
//! (`load_library_with`) and played with the same `rustysynth` synthesizer
//! the engine uses. This binary has a single test because it sets the
//! bundled-font environment variables in-process for the full-song render.

use std::path::PathBuf;

use cadence_studio::audio::render_project;
use cadence_studio::font::{load_library_with, BUNDLED_FONTS_DIR_ENV, FONTS_DIR_ENV};
use cadence_studio::model::{create_track, FontRef, Instrument, Note, Project};
use rustysynth::{Synthesizer, SynthesizerSettings};

const NAME: &str = "Cadence Harmonium";
const PROGRAM: u8 = 20;
const RATE: usize = 44_100;

fn font_path() -> PathBuf {
    std::env::var_os("CADENCE_TEST_HARMONIUM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(".cache/soundfonts")
                .join(format!("{NAME}.sf2"))
        })
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

/// Holds `key` for `seconds` on a fresh synth (no reverb/chorus, like the
/// engine) and returns the left channel.
fn hold(font: &std::sync::Arc<rustysynth::SoundFont>, key: i32, seconds: f32) -> Vec<f32> {
    let mut settings = SynthesizerSettings::new(RATE as i32);
    settings.enable_reverb_and_chorus = false;
    let mut synth = Synthesizer::new(font, &settings).unwrap();
    synth.process_midi_message(0, 0xC0, i32::from(PROGRAM), 0);
    synth.note_on(0, key, 100);
    let n = (seconds * RATE as f32) as usize;
    let mut left = vec![0.0; n];
    let mut right = vec![0.0; n];
    synth.render(&mut left, &mut right);
    left
}

#[test]
#[ignore = "needs .cache/soundfonts/Cadence Harmonium.sf2 (python3 scripts/build-harmonium-sf2.py)"]
fn harmonium_font_sustains_without_clicks() {
    let path = font_path();
    assert!(
        path.is_file(),
        "{} is missing: run python3 scripts/build-harmonium-sf2.py",
        path.display()
    );
    let dir = path.parent().unwrap().to_path_buf();
    let empty = tempfile::tempdir().unwrap();

    // Cadence's loader sees it as a built-in font with one preset.
    let library = load_library_with(std::slice::from_ref(&dir), empty.path());
    assert!(library.errors.is_empty(), "{:?}", library.errors);
    assert!(library.is_builtin(NAME), "listed as built-in");
    let info = library.fonts.iter().find(|f| f.name == NAME).unwrap();
    assert_eq!(info.presets, 1);
    let font = library.get(info.key).unwrap().clone();
    let preset = &font.get_presets()[0];
    assert_eq!(preset.get_name(), "Harmonium");
    assert_eq!(
        (preset.get_bank_number(), preset.get_patch_number()),
        (0, 20)
    );
    assert_eq!(font.get_instruments()[0].get_regions().len(), 38);

    // A held C3 for 6 s: sustained (the loop keeps the level) and no click
    // where the loop wraps (no step larger than the unlooped attack has).
    let c3 = hold(&font, 48, 6.0);
    let at = |t: f32| (t * RATE as f32) as usize;
    let rms1 = rms(&c3[at(0.9)..at(1.1)]);
    let rms5 = rms(&c3[at(4.9)..at(5.1)]);
    let natural = max_step(&c3[at(0.2)..at(0.9)]);
    let looped = max_step(&c3[at(1.0)..]);
    eprintln!("C3: rms 1 s {rms1:.4}, rms 5 s {rms5:.4}, max step attack {natural:.4}, looped {looped:.4}");
    assert!(rms1 > 0.02, "C3 is audible ({rms1})");
    let ratio = rms5 / rms1;
    assert!(
        (0.75..=1.33).contains(&ratio),
        "C3 sustains: rms ratio {ratio}"
    );
    assert!(
        looped <= natural * 1.3,
        "C3 loop wrap clicks: {looped} > {natural}"
    );

    // Every recorded note: the loop wrap in the sample data is no bigger a
    // step than the loop body has, and a held note at its root key keeps
    // its level over two seconds of looping without a step larger than
    // the first (unwrapped) pass through the loop produced.
    let wave = font.get_wave_data();
    let headers = font.get_sample_headers();
    for region in font.get_instruments()[0].get_regions() {
        let key = region.get_root_key();
        let h = &headers[region.get_sample_id()];
        let (ls, le) = (h.get_start_loop() as usize, h.get_end_loop() as usize);
        let body: Vec<f32> = wave[ls..le].iter().map(|&v| f32::from(v)).collect();
        let wrap = (f32::from(wave[ls]) - f32::from(wave[le - 1])).abs();
        assert!(
            wrap <= max_step(&body),
            "key {key}: loop wrap step {wrap} > {}",
            max_step(&body)
        );
        let t_ls = (ls - h.get_start() as usize) as f32 / RATE as f32;
        let t_le = (le - h.get_start() as usize) as f32 / RATE as f32;
        let x = hold(&font, key, t_le + 2.3);
        let first = rms(&x[at(t_ls)..at(t_ls + 0.2)]);
        let late = rms(&x[at(t_le + 2.0)..at(t_le + 2.2)]);
        let natural = max_step(&x[at(t_ls)..at(t_le)]);
        let looped = max_step(&x[at(t_le)..]);
        eprintln!(
            "key {key:3}: loop {t_ls:.2}-{t_le:.2} s, rms {first:.4} -> {late:.4} ({:.2}), \
             max step {natural:.4} / {looped:.4}",
            late / first
        );
        assert!(first > 0.005, "key {key} is audible ({first})");
        let ratio = late / first;
        assert!((0.6..=1.6).contains(&ratio), "key {key} sustains: {ratio}");
        assert!(
            looped <= natural * 1.25 + 0.001,
            "key {key} loop wrap clicks: {looped} > {natural}"
        );
    }
    // Keys outside the recorded notes (the F#4 gap, far below and above)
    // still play and sustain.
    for key in [0, 24, 66, 96, 127] {
        let x = hold(&font, key, 4.0);
        let early = rms(&x[at(1.5)..at(1.7)]);
        let late = rms(&x[at(3.5)..at(3.7)]);
        eprintln!("key {key:3}: rms {early:.4} -> {late:.4}");
        assert!(early > 0.002, "key {key} is audible ({early})");
        assert!((0.5..=2.0).contains(&(late / early)), "key {key} sustains");
        assert!(x.iter().all(|v| v.is_finite()));
    }

    // The engine path: a song track on the font renders through
    // `render_project` with the bundled-font dir pointed at the font.
    std::env::set_var(BUNDLED_FONTS_DIR_ENV, &dir);
    std::env::set_var(FONTS_DIR_ENV, empty.path());
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 4;
    p.tracks = vec![create_track(Instrument::Lead)];
    p.tracks[0].effects.reverb = 0.0;
    p.tracks[0].effects.delay = 0.0;
    p.tracks[0].font = Some(FontRef::new(NAME, PROGRAM));
    // Three bars of C3 at 120 BPM: 6 s.
    p.tracks[0].patterns[0].length = 64;
    p.tracks[0].patterns[0].notes = vec![Note::new(48, 0.0, 48.0, 0.9)];
    p.validate().unwrap();
    let audio = render_project(&p, RATE as u32, 20.0);
    std::env::remove_var(BUNDLED_FONTS_DIR_ENV);
    std::env::remove_var(FONTS_DIR_ENV);
    let audio = audio.unwrap();
    let rms1 = rms(&audio.left[at(0.9)..at(1.1)]);
    let rms5 = rms(&audio.left[at(4.9)..at(5.1)]);
    eprintln!("song render: rms 1 s {rms1:.4}, rms 5 s {rms5:.4}");
    assert!(rms1 > 0.01, "song track plays the harmonium ({rms1})");
    let ratio = rms5 / rms1;
    assert!((0.75..=1.33).contains(&ratio), "song C3 sustains: {ratio}");
}
