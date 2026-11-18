mod common;

use cadence_studio::audio::{export_stems, stem_skip_reason, EXPORT_SAMPLE_RATE};
use cadence_studio::font::FontLibrary;
use cadence_studio::model::{create_track, Instrument, Note, Project};
use cadence_studio::persistence::save_project;

use common::{assert_riff_wave, read_wav, tiny_project};

/// One bar: the tiny-project piano ("Keys") plus a bass track, one note each.
fn two_track_project() -> Project {
    let mut project = tiny_project();
    project.tracks[0].name = "Keys".into();
    let mut bass = create_track(Instrument::Bass);
    bass.name = "Bass".into();
    bass.patterns[0].notes = vec![Note::new(36, 0.0, 4.0, 0.9)];
    project.tracks.push(bass);
    project
}

#[test]
fn stems_write_one_wav_per_audible_track() {
    let dir = tempfile::tempdir().unwrap();
    let project = two_track_project();
    let out = dir.path().join("stems");

    let written = export_stems(
        &project,
        &FontLibrary::empty(),
        &out,
        EXPORT_SAMPLE_RATE,
        |_| {},
    )
    .unwrap();
    assert_eq!(written.len(), 2);
    assert!(written[0].ends_with("Keys-01.wav"), "{:?}", written[0]);
    assert!(written[1].ends_with("Bass-02.wav"), "{:?}", written[1]);

    let keys = read_wav(&written[0]);
    let bass = read_wav(&written[1]);
    assert_riff_wave(&written[0]);
    assert!(keys.peak() > 0.01, "keys stem silent");
    assert!(bass.peak() > 0.01, "bass stem silent");
    assert_ne!(keys.samples, bass.samples, "stems must differ");

    // Progress covers the whole batch and ends at 1.0.
    let mut seen = Vec::new();
    export_stems(
        &project,
        &FontLibrary::empty(),
        &dir.path().join("again"),
        EXPORT_SAMPLE_RATE,
        |p| seen.push(p),
    )
    .unwrap();
    assert!(seen.windows(2).all(|w| w[1] >= w[0]), "progress regressed");
    assert_eq!(*seen.last().unwrap(), 1.0);
}

#[test]
fn muted_tracks_are_skipped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = two_track_project();
    project.tracks[1].mute = true;

    assert_eq!(stem_skip_reason(&project, 1), Some("muted"));
    let written = export_stems(
        &project,
        &FontLibrary::empty(),
        dir.path(),
        EXPORT_SAMPLE_RATE,
        |_| {},
    )
    .unwrap();
    assert_eq!(written.len(), 1);
    assert!(written[0].ends_with("Keys-01.wav"));
    assert!(read_wav(&written[0]).peak() > 0.01);
}

#[test]
fn soloed_tracks_limit_the_stem_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = two_track_project();
    project.tracks[0].solo = true;

    assert_eq!(stem_skip_reason(&project, 1), Some("not soloed"));
    let written = export_stems(
        &project,
        &FontLibrary::empty(),
        dir.path(),
        EXPORT_SAMPLE_RATE,
        |_| {},
    )
    .unwrap();
    assert_eq!(written.len(), 1);
    assert!(written[0].ends_with("Keys-01.wav"));
}

#[test]
fn render_stems_cli_writes_per_track_wavs() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("two.cadence");
    let mut project = two_track_project();
    project.tracks[1].mute = true;
    save_project(&project, &song).unwrap();
    let out_dir = dir.path().join("stems-out");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args([
            "--render-stems".as_ref(),
            song.as_os_str(),
            out_dir.as_os_str(),
        ])
        .env(
            "CADENCE_DATA_DIR",
            std::env::temp_dir().join("cadence-cli-test"),
        )
        .output()
        .expect("run cadence binary");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stem = out_dir.join("Keys-01.wav");
    assert_riff_wave(&stem);
    assert!(read_wav(&stem).peak() > 0.01);
    assert!(
        !out_dir.join("Bass-02.wav").exists(),
        "muted track must not get a stem"
    );
    // The report names the skipped track and why.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("skipped \"Bass\" (muted)"), "{stdout}");
}
