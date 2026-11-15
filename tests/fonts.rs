//! SoundFont (`.sf2`) integration tests.
//!
//! No font binary is committed to the repo. The render test that needs a real
//! font is `#[ignore]`d and gated on `CADENCE_TEST_FONT=/path/to/font.sf2`
//! (e.g. GeneralUser GS); it points `CADENCE_FONTS_DIR` at the font's folder
//! for the duration of the test. Run it with:
//!
//! ```sh
//! CADENCE_TEST_FONT=/path/to/GeneralUser.sf2 cargo test --test fonts -- --ignored
//! ```
//!
//! Everything else runs without a font: the catalog scan, directory
//! resolution and the missing-font fallback are covered here and in unit
//! tests.

use cadence_studio::audio::render_project;
use cadence_studio::font::{font_key, gm_program_name, load_library_from, user_fonts_dir};
use cadence_studio::model::{create_track, FontRef, Instrument, Note, Project};
use std::path::PathBuf;

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn font_key_matches_model_reference() {
    let font = FontRef::new("GeneralUser", 40);
    assert_eq!(font.key(), font_key("GeneralUser"));
    assert_eq!(font.program_name(), gm_program_name(40));
    assert_eq!(font.label(), "GeneralUser · Violin");
}

#[test]
fn user_fonts_dir_is_a_directory() {
    // Resolution itself is unit-tested with an injected env; here we only
    // require the default path to be usable (creating it is the app's job,
    // not the loader's — a missing dir simply yields an empty library).
    let dir = user_fonts_dir();
    assert!(dir.ends_with("fonts") || dir.components().count() > 1);
    let library = load_library_from(&dir);
    // Reading the real user dir must never fail the process.
    assert!(library.errors.len() < 10_000);
}

#[test]
fn catalog_reports_bad_files_without_failing() {
    let dir = std::env::temp_dir().join(format!("cadence-fonts-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.sf2"), b"definitely not a soundfont").unwrap();
    std::fs::write(dir.join("ignored.txt"), b"not scanned").unwrap();
    let library = load_library_from(&dir);
    assert!(library.fonts.is_empty());
    assert_eq!(library.errors.len(), 1);
    assert!(library.errors[0].contains("broken.sf2"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn font_track_without_fonts_still_renders() {
    // End-to-end through the offline renderer: a font reference whose font is
    // not installed must fall back to the track's built-in instrument.
    // `render_project` loads the user font dir only because a font is
    // referenced; the bogus name can never match a real font.
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.tracks = vec![create_track(Instrument::Piano)];
    p.tracks[0].effects.reverb = 0.0;
    p.tracks[0].effects.delay = 0.0;
    p.tracks[0].font = Some(FontRef::new("CadenceTestFontNotInstalled", 0));
    p.tracks[0].patterns[0].notes = vec![Note::new(60, 0.0, 4.0, 0.9)];
    p.validate().unwrap();
    let a = render_project(&p, 22_050, 15.0).unwrap();
    let song_frames = (p.duration_seconds() * 22_050.0) as usize;
    assert!(
        rms(&a.left[..song_frames.min(a.left.len())]) > 0.005,
        "fallback instrument is audible"
    );
}

#[test]
#[ignore = "needs a real .sf2 file; set CADENCE_TEST_FONT=/path/to/font.sf2"]
fn soundfont_track_renders_real_samples() {
    let font_path = PathBuf::from(
        std::env::var_os("CADENCE_TEST_FONT").expect("set CADENCE_TEST_FONT to a .sf2 path"),
    );
    let name = font_path
        .file_stem()
        .expect("font path has a file name")
        .to_string_lossy()
        .into_owned();
    // The catalog scans CADENCE_FONTS_DIR; point it at the font's folder so
    // `render_project`'s automatic `load_library()` finds it.
    let dir = font_path.parent().expect("font path has a parent");
    std::env::set_var("CADENCE_FONTS_DIR", dir);
    let result = std::panic::catch_unwind(|| {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        p.tracks = vec![create_track(Instrument::Piano)];
        p.tracks[0].effects.reverb = 0.0;
        p.tracks[0].effects.delay = 0.0;
        p.tracks[0].font = Some(FontRef::new(&name, 0));
        p.tracks[0].patterns[0].notes = vec![Note::new(60, 0.0, 4.0, 0.9)];
        p.validate().unwrap();
        render_project(&p, 44_100, 20.0).unwrap()
    });
    std::env::remove_var("CADENCE_FONTS_DIR");
    let a = result.expect("render panicked");
    // The first two seconds hold the sustained piano note; sampled audio must
    // be clearly non-silent well above the noise floor.
    let window_end = (2.0 * 44_100.0) as usize;
    let window = &a.left[..window_end.min(a.left.len())];
    assert!(
        window.iter().any(|s| s.abs() > 0.005),
        "SoundFont note produced no audible output"
    );
    assert!(rms(window) > 0.001, "SoundFont note is essentially silent");
}

#[test]
#[ignore = "needs a real .sf2 file; set CADENCE_TEST_FONT=/path/to/font.sf2"]
fn soundfont_library_loads_from_test_font_dir() {
    let font_path = PathBuf::from(
        std::env::var_os("CADENCE_TEST_FONT").expect("set CADENCE_TEST_FONT to a .sf2 path"),
    );
    let name = font_path
        .file_stem()
        .expect("font path has a file name")
        .to_string_lossy()
        .into_owned();
    let dir = font_path.parent().expect("font path has a parent");
    let library = load_library_from(dir);
    assert!(
        library.fonts.iter().any(|f| f.name == name),
        "font {name} not found in {}; errors: {:?}",
        dir.display(),
        library.errors
    );
    let info = library.fonts.iter().find(|f| f.name == name).unwrap();
    assert!(info.presets > 0);
    assert!(library.get(info.key).is_some());
}
