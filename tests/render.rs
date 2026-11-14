mod common;

use cadence_studio::audio::export_wav;
use cadence_studio::model::Project;

use common::{assert_riff_wave, read_wav, tiny_project};

fn seconds(project: &Project) -> f64 {
    project.total_steps() * 60.0 / project.bpm / 4.0
}

#[test]
fn demo_exports_valid_stereo_audio() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.wav");
    let demo = Project::demo();
    let mut progress = Vec::new();
    export_wav(&demo, &path, |p| progress.push(p)).unwrap();

    assert_riff_wave(&path);
    let wav = read_wav(&path);
    assert_eq!(wav.spec.channels, 2);
    let song = seconds(&demo);
    assert!(
        wav.seconds() >= song - 0.05 && wav.seconds() <= song + 15.0,
        "rendered {:.2}s for a {:.2}s song",
        wav.seconds(),
        song
    );
    assert!(wav.samples.iter().all(|s| s.is_finite()));
    assert!(wav.peak() <= 1.0, "clipped: peak {}", wav.peak());
    assert!(
        wav.peak() > 0.05,
        "demo is nearly silent: peak {}",
        wav.peak()
    );
    assert!(
        wav.rms() > 0.005,
        "demo is nearly silent: rms {}",
        wav.rms()
    );

    assert!(!progress.is_empty(), "no progress reported");
    assert!(
        progress.windows(2).all(|w| w[1] >= w[0]),
        "progress went backwards"
    );
    assert!(progress.iter().all(|p| (0.0..=1.0).contains(p)));
    assert!(*progress.last().unwrap() >= 0.99);

    // Every bar of the demo has sound in it (no dropped clips).
    let frames_per_bar = (wav.spec.sample_rate as f64 * 16.0 * 60.0 / demo.bpm / 4.0) as usize;
    for bar in 0..demo.bars as usize {
        let start = bar * frames_per_bar * 2;
        let end = ((bar + 1) * frames_per_bar * 2).min(wav.samples.len());
        let peak = wav.samples[start..end]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.01, "bar {} is silent", bar + 1);
    }
}

#[test]
fn rendering_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.wav");
    let b = dir.path().join("b.wav");
    let project = tiny_project();
    export_wav(&project, &a, |_| {}).unwrap();
    export_wav(&project, &b, |_| {}).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
}

#[test]
fn muted_and_zero_master_render_silence() {
    let dir = tempfile::tempdir().unwrap();

    let mut muted = Project::demo();
    for track in &mut muted.tracks {
        track.mute = true;
    }
    let path = dir.path().join("muted.wav");
    export_wav(&muted, &path, |_| {}).unwrap();
    assert_riff_wave(&path);
    assert!(read_wav(&path).peak() < 1e-4);

    let mut quiet = Project::demo();
    quiet.master_volume = 0.0;
    let path = dir.path().join("quiet.wav");
    export_wav(&quiet, &path, |_| {}).unwrap();
    assert!(read_wav(&path).peak() < 1e-4);
}

#[test]
fn solo_limits_output_to_soloed_track() {
    let dir = tempfile::tempdir().unwrap();
    let full = dir.path().join("full.wav");
    let solo = dir.path().join("solo.wav");
    let demo = Project::demo();
    export_wav(&demo, &full, |_| {}).unwrap();

    let mut soloed = demo.clone();
    soloed.tracks[0].solo = true;
    export_wav(&soloed, &solo, |_| {}).unwrap();

    let full = read_wav(&full);
    let solo = read_wav(&solo);
    assert!(solo.peak() > 0.01, "soloed track is silent");
    assert_ne!(full.samples, solo.samples, "solo had no effect");
}

#[test]
fn hard_pan_moves_energy_to_one_side() {
    let dir = tempfile::tempdir().unwrap();
    let mut project = tiny_project();
    project.tracks[0].effects.reverb = 0.0;
    project.tracks[0].effects.delay = 0.0;

    project.tracks[0].pan = -1.0;
    let left = dir.path().join("left.wav");
    export_wav(&project, &left, |_| {}).unwrap();
    let (l, r) = read_wav(&left).channel_rms();
    assert!(l > 0.001 && l > r * 4.0, "pan left: L={l} R={r}");

    project.tracks[0].pan = 1.0;
    let right = dir.path().join("right.wav");
    export_wav(&project, &right, |_| {}).unwrap();
    let (l, r) = read_wav(&right).channel_rms();
    assert!(r > 0.001 && r > l * 4.0, "pan right: L={l} R={r}");
}

#[test]
fn export_to_missing_folder_fails_without_panicking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing").join("out.wav");
    assert!(export_wav(&tiny_project(), &path, |_| {}).is_err());
    assert!(!path.exists());
}
