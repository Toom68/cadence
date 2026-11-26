#![allow(dead_code)]

use std::path::Path;

use cadence_studio::model::{
    create_pattern, create_track, next_id, Clip, Instrument, Note, Project,
};

/// One bar, one piano track with a single held C4: quick to render.
pub fn tiny_project() -> Project {
    let mut project = Project::empty();
    project.name = "Tiny".into();
    project.bars = 1;
    project.tracks.clear();
    let mut track = create_track(Instrument::Piano);
    let mut pattern = create_pattern(Instrument::Piano, 16);
    pattern.notes = vec![Note {
        id: next_id(),
        pitch: 60,
        start: 0.0,
        duration: 4.0,
        velocity: 0.9,
    }];
    track.clips = vec![Clip {
        id: next_id(),
        pattern_id: pattern.id,
        start: 0.0,
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    }];
    track.patterns = vec![pattern];
    project.tracks.push(track);
    project.validate().expect("tiny project must be valid");
    project
}

/// Decoded WAV contents as interleaved f32 samples in -1..=1.
pub struct Wav {
    pub spec: hound::WavSpec,
    pub samples: Vec<f32>,
}

impl Wav {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.spec.channels as usize
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.spec.sample_rate as f64
    }

    pub fn peak(&self) -> f32 {
        self.samples
            .iter()
            .fold(0.0f32, |peak, s| peak.max(s.abs()))
    }

    pub fn rms(&self) -> f64 {
        let sum: f64 = self.samples.iter().map(|s| (*s as f64).powi(2)).sum();
        (sum / self.samples.len().max(1) as f64).sqrt()
    }

    /// RMS of the left and right channels separately.
    pub fn channel_rms(&self) -> (f64, f64) {
        let mut sums = [0.0f64; 2];
        for frame in self.samples.as_chunks::<2>().0 {
            sums[0] += (frame[0] as f64).powi(2);
            sums[1] += (frame[1] as f64).powi(2);
        }
        let n = self.frames().max(1) as f64;
        ((sums[0] / n).sqrt(), (sums[1] / n).sqrt())
    }
}

pub fn read_wav(path: &Path) -> Wav {
    let mut reader = hound::WavReader::open(path).expect("output must be a readable WAV file");
    let spec = reader.spec();
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.expect("valid float sample"))
            .collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.expect("valid int sample") as f32 / scale)
                .collect()
        }
    };
    Wav { spec, samples }
}

/// Checks the raw RIFF/WAVE container layout independently of `hound`.
pub fn assert_riff_wave(path: &Path) {
    let bytes = std::fs::read(path).expect("read wav");
    assert!(
        bytes.len() > 44,
        "WAV file is too short: {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[0..4], b"RIFF", "missing RIFF magic");
    assert_eq!(&bytes[8..12], b"WAVE", "missing WAVE form type");
    let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!(
        riff_size + 8,
        bytes.len(),
        "RIFF size does not match file length"
    );

    let mut offset = 12;
    let mut saw_fmt = false;
    let mut data_size = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        assert!(
            body + size <= bytes.len(),
            "chunk {:?} overruns file",
            String::from_utf8_lossy(id)
        );
        match id {
            b"fmt " => {
                saw_fmt = true;
                let channels = u16::from_le_bytes(bytes[body + 2..body + 4].try_into().unwrap());
                let rate = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().unwrap());
                assert_eq!(channels, 2, "export must be stereo");
                assert!(
                    (22_050..=192_000).contains(&rate),
                    "unexpected sample rate {rate}"
                );
            }
            b"data" => data_size = Some(size),
            _ => {}
        }
        offset = body + size + (size & 1);
    }
    assert!(saw_fmt, "missing fmt chunk");
    let data_size = data_size.expect("missing data chunk");
    assert!(data_size > 0, "data chunk is empty");
}

// `tiny_sf2` lives in its own file so the library's unit tests can share
// it (`src/audio/mod.rs` includes it by path).
mod sf2;
#[allow(unused_imports)] // not every test binary uses it
pub use sf2::tiny_sf2;
