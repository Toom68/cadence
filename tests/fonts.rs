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
//! Everything else runs without a real font: the catalog scan, directory
//! resolution, the missing-font fallback and the bundled ("built-in") font
//! path are covered here and in unit tests, using a tiny generated `.sf2`
//! (`common::tiny_sf2`). Tests never read the developer font cache: library
//! tests pass explicit directories and CLI tests set `CADENCE_SOUNDFONT_DIR`.

mod common;

use cadence_studio::audio::render_project;
use cadence_studio::font::{
    font_key, gm_program_name, load_library_from, load_library_with, user_fonts_dir,
    BUILTIN_FONT_NAME, BUILTIN_FONT_NAMES, BUNDLED_FONTS_DIR_ENV, HARMONIUM_FONT_NAME,
};
use cadence_studio::model::{create_track, FontRef, Instrument, Note, Project};
use cadence_studio::persistence::save_project;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

fn write_font(dir: &Path, name: &str) {
    std::fs::write(dir.join(format!("{name}.sf2")), common::tiny_sf2()).unwrap();
}

#[test]
fn tiny_test_font_parses() {
    let dir = tempfile::tempdir().unwrap();
    write_font(dir.path(), "Tiny");
    let library = load_library_from(dir.path());
    assert!(library.errors.is_empty(), "{:?}", library.errors);
    assert_eq!(library.fonts.len(), 1);
    assert_eq!(library.fonts[0].presets, 1);
    assert!(!library.has_builtin(), "user fonts are not built-in");
}

#[test]
fn bundled_fonts_come_first_and_are_marked_builtin() {
    let bundled = tempfile::tempdir().unwrap();
    let fallback = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    write_font(bundled.path(), BUILTIN_FONT_NAME);
    // A second bundled dir with the same font (e.g. the dev cache next to a
    // packaged copy) is skipped silently.
    write_font(fallback.path(), BUILTIN_FONT_NAME);
    write_font(user.path(), "Zed");
    write_font(user.path(), "Alpha");
    // A user font can't shadow the built-in one; it's reported instead.
    write_font(user.path(), BUILTIN_FONT_NAME);
    let library = load_library_with(
        &[
            bundled.path().to_path_buf(),
            PathBuf::from("/definitely/not/a/real/dir"),
            fallback.path().to_path_buf(),
        ],
        user.path(),
    );
    let names: Vec<&str> = library.fonts.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, [BUILTIN_FONT_NAME, "Alpha", "Zed"]);
    assert!(library.has_builtin());
    assert!(library.is_builtin(BUILTIN_FONT_NAME));
    assert!(!library.is_builtin("Alpha"));
    assert_eq!(
        library.display_name(BUILTIN_FONT_NAME),
        "Cadence GM (built-in)"
    );
    assert_eq!(library.display_name("Alpha"), "Alpha");
    assert_eq!(library.errors.len(), 1, "{:?}", library.errors);
    assert!(library.errors[0].contains("built-in font already uses this name"));
    assert_eq!(library.dir, user.path());
    for info in &library.fonts {
        assert!(library.get(info.key).is_some(), "{} is playable", info.name);
    }
    // The built-in font resolves by the name songs store.
    assert!(library
        .get(FontRef::new(BUILTIN_FONT_NAME, 0).key())
        .is_some());
}

#[test]
fn both_builtin_fonts_are_listed_and_protected() {
    let bundled = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    for name in BUILTIN_FONT_NAMES {
        write_font(bundled.path(), name);
        // Neither built-in name can be taken by a user font.
        write_font(user.path(), name);
    }
    write_font(user.path(), "Mine");
    let library = load_library_with(&[bundled.path().to_path_buf()], user.path());
    let names: Vec<&str> = library.fonts.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, [BUILTIN_FONT_NAME, HARMONIUM_FONT_NAME, "Mine"]);
    for name in BUILTIN_FONT_NAMES {
        assert!(library.is_builtin(name), "{name}");
        assert_eq!(library.display_name(name), format!("{name} (built-in)"));
    }
    assert_eq!(library.errors.len(), 2, "{:?}", library.errors);
    assert!(library
        .errors
        .iter()
        .all(|e| e.contains("built-in font already uses this name")));
    assert_eq!(library.featured_sounds().len(), 1);
    let gm: Vec<&str> = library.gm_fonts().map(|f| f.name.as_str()).collect();
    assert_eq!(gm, [BUILTIN_FONT_NAME, "Mine"]);

    // The CLI lists both, says what the harmonium plays, and reports a
    // missing built-in by name.
    let data = tempfile::tempdir().unwrap();
    let out = cadence(
        bundled.path(),
        data.path(),
        &["--list-instruments".as_ref()],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("Cadence GM (built-in)"), "{stdout}");
    assert!(stdout.contains("Cadence Harmonium (built-in)"), "{stdout}");
    assert!(stdout.contains("plays Harmonium"), "{stdout}");
    assert!(!stdout.contains("not found"), "{stdout}");
    std::fs::remove_file(bundled.path().join("Cadence Harmonium.sf2")).unwrap();
    let out = cadence(
        bundled.path(),
        data.path(),
        &["--list-instruments".as_ref()],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("built-in SoundFont not found: Cadence Harmonium"),
        "{stdout}"
    );
}

#[test]
fn broken_bundled_font_is_reported_and_user_fonts_still_load() {
    let bundled = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::write(bundled.path().join("Cadence GM.sf2"), b"truncated").unwrap();
    write_font(user.path(), "Mine");
    let library = load_library_with(&[bundled.path().to_path_buf()], user.path());
    assert_eq!(library.fonts.len(), 1);
    assert_eq!(library.fonts[0].name, "Mine");
    assert!(!library.has_builtin());
    assert_eq!(library.errors.len(), 1);
    assert!(
        library.errors[0].contains("(built-in)"),
        "{:?}",
        library.errors
    );
}

/// Runs the CLI with an isolated data dir and `soundfonts` as the bundled
/// font folder.
fn cadence(soundfonts: &Path, data: &Path, args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args(args)
        .env("CADENCE_DATA_DIR", data)
        .env(BUNDLED_FONTS_DIR_ENV, soundfonts)
        .env_remove("CADENCE_FONTS_DIR")
        .env_remove("CADENCE_PACKS_DIR")
        .output()
        .expect("run cadence binary")
}

#[test]
fn cli_lists_and_renders_with_the_bundled_font() {
    let root = tempfile::tempdir().unwrap();
    let with_font = root.path().join("soundfonts");
    let without_font = root.path().join("no-soundfonts");
    let data = root.path().join("data");
    std::fs::create_dir_all(&with_font).unwrap();
    std::fs::create_dir_all(&without_font).unwrap();
    write_font(&with_font, BUILTIN_FONT_NAME);

    let out = cadence(&with_font, &data, &["--list-instruments".as_ref()]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Cadence GM (built-in)"),
        "built-in font listed:\n{stdout}"
    );

    // A song referencing the built-in font by name renders through it: the
    // tiny font's sine differs from the fallback piano it replaces.
    let mut p = Project::empty();
    p.bpm = 120.0;
    p.bars = 1;
    p.tracks = vec![create_track(Instrument::Piano)];
    p.tracks[0].effects.reverb = 0.0;
    p.tracks[0].effects.delay = 0.0;
    p.tracks[0].font = Some(FontRef::new(BUILTIN_FONT_NAME, 0));
    p.tracks[0].patterns[0].notes = vec![Note::new(69, 0.0, 8.0, 0.9)];
    p.validate().unwrap();
    let song = root.path().join("gm.cadence");
    save_project(&p, &song).unwrap();
    let render = |soundfonts: &Path, name: &str| {
        let wav = root.path().join(name);
        let out = cadence(
            soundfonts,
            &data,
            &["--render".as_ref(), song.as_os_str(), wav.as_os_str()],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        common::read_wav(&wav)
    };
    let font = render(&with_font, "font.wav");
    let fallback = render(&without_font, "fallback.wav");
    assert!(font.peak() > 0.01, "bundled font is audible");
    assert!(fallback.peak() > 0.01, "fallback is audible");
    assert_ne!(
        font.samples, fallback.samples,
        "the bundled font, not the fallback, played"
    );
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
