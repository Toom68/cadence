mod common;

use std::process::{Command, Output};

use cadence_studio::model::Project;
use cadence_studio::persistence::save_project;

fn cadence(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args(args)
        .env(
            "CADENCE_DATA_DIR",
            std::env::temp_dir().join("cadence-cli-test"),
        )
        .output()
        .expect("run cadence binary")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version_and_help() {
    let out = cadence(&["--version".as_ref()]);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains(env!("CARGO_PKG_VERSION")));

    let out = cadence(&["--help".as_ref()]);
    assert!(out.status.success());
    let help = text(&out.stdout);
    assert!(help.contains("--render-demo") && help.contains("--check"));
}

#[test]
fn bad_arguments_exit_with_usage_error() {
    for args in [
        &["--bogus"][..],
        &["--render"],
        &["--check"],
        &["--check", "a", "b"],
    ] {
        let args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_ref()).collect();
        let out = cadence(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("USAGE"));
    }
}

#[test]
fn check_reports_valid_and_invalid_songs() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("demo.cadence");
    let demo = Project::demo();
    save_project(&demo, &song).unwrap();
    let out = cadence(&["--check".as_ref(), song.as_os_str()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let report = text(&out.stdout);
    assert!(
        report.contains("OK") && report.contains(&demo.name),
        "{report}"
    );
    assert!(
        report.contains(&format!("tracks:   {}", demo.tracks.len())),
        "{report}"
    );

    let broken = dir.path().join("broken.cadence");
    std::fs::write(&broken, b"{ nope").unwrap();
    let out = cadence(&["--check".as_ref(), broken.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).starts_with("error:"));

    let missing = dir.path().join("missing.cadence");
    let out = cadence(&["--check".as_ref(), missing.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn render_song_and_demo_write_wav_files() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("tiny.cadence");
    save_project(&common::tiny_project(), &song).unwrap();
    let wav = dir.path().join("tiny.wav");
    let out = cadence(&["--render".as_ref(), song.as_os_str(), wav.as_os_str()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    common::assert_riff_wave(&wav);
    assert!(common::read_wav(&wav).peak() > 0.01);

    let demo_wav = dir.path().join("demo.wav");
    let out = cadence(&["--render-demo".as_ref(), demo_wav.as_os_str()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    common::assert_riff_wave(&demo_wav);
    assert!(text(&out.stderr).contains("Wrote"));
}

#[test]
fn render_refuses_to_overwrite_its_input() {
    let dir = tempfile::tempdir().unwrap();
    let song = dir.path().join("tiny.cadence");
    save_project(&common::tiny_project(), &song).unwrap();
    let before = std::fs::read(&song).unwrap();
    let out = cadence(&["--render".as_ref(), song.as_os_str(), song.as_os_str()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(std::fs::read(&song).unwrap(), before);
}

/// Writes a short mono 220 Hz WAV to `path`.
fn write_tone(path: &std::path::Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..44_100u32 {
        let t = i as f32 / spec.sample_rate as f32;
        let s = (t * 220.0 * std::f32::consts::TAU).sin() * 0.6;
        w.write_sample((s * i16::MAX as f32) as i16).unwrap();
    }
    w.finalize().unwrap();
}

/// A song saved with "Save Copy with Audio…" (its WAV in the sibling
/// `<name> Audio` folder) checks and renders from the command line on a
/// computer whose audio store has never seen the file.
#[test]
fn check_and_render_pick_up_the_songs_audio_folder() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let song = dir.path().join("Travel.cadence");
    let folder = cadence_studio::persistence::song_audio_dir(&song);
    std::fs::create_dir_all(&folder).unwrap();
    write_tone(&folder.join("take.wav"));
    let mut project = Project::demo();
    for track in &mut project.tracks {
        track.mute = true;
    }
    let mut track = cadence_studio::model::create_audio_track("Audio");
    track
        .audio_clips
        .push(cadence_studio::model::AudioClip::new(
            "take.wav", "Take", 0.0, 16.0,
        ));
    project.tracks.push(track);
    save_project(&project, &song).unwrap();
    let run = |args: &[&std::ffi::OsStr]| {
        Command::new(env!("CARGO_BIN_EXE_cadence"))
            .args(args)
            .env("CADENCE_DATA_DIR", &data)
            .env_remove("CADENCE_PACKS_DIR")
            .output()
            .expect("run cadence binary")
    };

    let out = run(&["--check".as_ref(), song.as_os_str()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        !text(&out.stdout).contains("WARNING"),
        "{}",
        text(&out.stdout)
    );
    assert!(text(&out.stderr).contains("Found 1 audio file"));
    assert!(data.join("audio").join("take.wav").is_file());

    // A fresh store (another computer) also renders the clip audibly.
    std::fs::remove_dir_all(&data).unwrap();
    let wav = dir.path().join("travel.wav");
    let out = run(&["--render".as_ref(), song.as_os_str(), wav.as_os_str()]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(
        !text(&out.stderr).contains("WARNING"),
        "{}",
        text(&out.stderr)
    );
    assert!(common::read_wav(&wav).peak() > 0.05, "the clip is audible");
}
