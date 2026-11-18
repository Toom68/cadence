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
fn tracks_on_a_muted_bus_are_skipped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = two_track_project();
    // Bass rides Bus A, which is muted: the group gate applies even though
    // the track itself is unmuted — and even when it is soloed.
    project.tracks[1].bus = 1;
    project.buses[0].mute = true;

    assert_eq!(stem_skip_reason(&project, 1), Some("on a muted bus"));
    let written = export_stems(
        &project,
        &FontLibrary::empty(),
        &dir.path().join("muted-bus"),
        EXPORT_SAMPLE_RATE,
        |_| {},
    )
    .unwrap();
    assert_eq!(written.len(), 1, "only the master-routed track gets a stem");
    assert!(written[0].ends_with("Keys-01.wav"));
    assert!(read_wav(&written[0]).peak() > 0.01);
    assert!(
        !dir.path().join("muted-bus").join("Bass-02.wav").exists(),
        "gated track must not leave a stem file"
    );

    // A soloed track on a muted bus stays skipped: the bus gate wins — and
    // the solo gates the master-routed track too, so nothing is exportable.
    project.tracks[1].solo = true;
    assert_eq!(stem_skip_reason(&project, 1), Some("on a muted bus"));
    assert_eq!(stem_skip_reason(&project, 0), Some("not soloed"));
    let err = export_stems(
        &project,
        &FontLibrary::empty(),
        &dir.path().join("solo-on-muted-bus"),
        EXPORT_SAMPLE_RATE,
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("Nothing to export"), "err: {err}");
}

#[test]
fn non_soloed_bus_tracks_are_skipped_while_another_bus_is_soloed() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = two_track_project();
    // Keys rides soloed Bus A; Bass rides Bus B; the third track goes
    // straight to the master. Bus solo gates every non-soloed route.
    project.tracks[0].bus = 1;
    project.tracks[1].bus = 2;
    let mut pad = create_track(Instrument::Pad);
    pad.name = "Pad".into();
    pad.patterns[0].notes = vec![Note::new(60, 0.0, 4.0, 0.9)];
    project.tracks.push(pad);
    project.buses[0].solo = true;

    assert_eq!(stem_skip_reason(&project, 0), None, "soloed bus is audible");
    assert_eq!(stem_skip_reason(&project, 1), Some("bus not soloed"));
    assert_eq!(
        stem_skip_reason(&project, 2),
        Some("bus not soloed"),
        "master-routed tracks are gated by a bus solo"
    );

    // Stem files are written only for the audible track.
    let out = dir.path().join("bus-solo");
    let written = export_stems(
        &project,
        &FontLibrary::empty(),
        &out,
        EXPORT_SAMPLE_RATE,
        |_| {},
    )
    .unwrap();
    assert_eq!(written.len(), 1);
    assert!(written[0].ends_with("Keys-01.wav"));
    assert!(read_wav(&written[0]).peak() > 0.01);
    for absent in ["Bass-02.wav", "Pad-03.wav"] {
        assert!(!out.join(absent).exists(), "{absent} must not exist");
    }
}

#[test]
fn render_stems_cli_writes_per_track_wavs() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("two.cadence");
    let mut project = two_track_project();
    project.tracks[1].mute = true;
    save_project(&project, &song).unwrap();
    let out_dir = dir.path().join("stems-out");
    let data_dir = std::env::temp_dir().join(format!("cadence-stem-cli-{}", std::process::id()));

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args([
            "--render-stems".as_ref(),
            song.as_os_str(),
            out_dir.as_os_str(),
        ])
        .env("CADENCE_DATA_DIR", &data_dir)
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
    std::fs::remove_dir_all(&data_dir).ok();
}
