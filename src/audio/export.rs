//! Offline rendering through exactly the same [`Renderer`] used for live playback, and
//! streaming 16-bit WAV export with effect tails.

use super::renderer::{Renderer, MAX_BLOCK};
use super::song::SongData;
use crate::audio::dsp::Rng;
use crate::font::FontLibrary;
use crate::model::{Project, MAX_TRACKS};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Sample rate of exported WAV files.
pub const EXPORT_SAMPLE_RATE: u32 = 44_100;
/// Longest effect tail rendered after the last bar.
pub const MAX_TAIL_SECONDS: f64 = 12.0;
/// Output below this level (about -80 dBFS) counts as silence when ending the tail.
const TAIL_THRESHOLD: f32 = 1e-4;
const TAIL_QUIET_SECONDS: f64 = 0.25;
const FINAL_FADE_SECONDS: f64 = 0.05;

/// Decoded stereo audio from [`render_project`].
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedAudio {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

/// Pull-based offline render: the song (honouring mute/solo/master) followed by its tail,
/// with the limiter look-ahead removed so audio lines up with the grid.
struct OfflineRender {
    renderer: Renderer,
    sample_rate: f64,
    song_frames: u64,
    max_frames: u64,
    emitted: u64,
    skip: usize,
    quiet_run: u64,
    finished: bool,
    fade_from: Option<u64>,
}

impl OfflineRender {
    fn new(project: &Project, sample_rate: u32, max_seconds: f64) -> Result<Self, String> {
        project
            .validate()
            .map_err(|e| format!("Cannot render this song: {e}"))?;
        // Font tracks need the catalog; parsing .sf2 files is too slow to do
        // unconditionally, so load only when the song actually references one.
        // Missing fonts fall back to each track's preset/built-in sound.
        let fonts = project
            .tracks
            .iter()
            .any(|t| t.font.is_some())
            .then(|| Arc::new(crate::font::load_library()));
        Ok(Self::from_song(
            SongData::from_project(project),
            project.duration_seconds(),
            fonts,
            sample_rate,
            max_seconds,
        ))
    }

    /// A render of an already-built `song` snapshot lasting `song_seconds`
    /// before the effect tail. `fonts` is the SoundFont catalog for
    /// `Sound::Font` tracks, or `None` when the song references no fonts.
    fn from_song(
        song: SongData,
        song_seconds: f64,
        fonts: Option<Arc<FontLibrary>>,
        sample_rate: u32,
        max_seconds: f64,
    ) -> Self {
        let sr = sample_rate as f64;
        let song_frames = (song_seconds * sr).ceil() as u64;
        let tail = (MAX_TAIL_SECONDS * sr) as u64;
        let max_frames = (song_frames + tail).min((max_seconds.max(0.0) * sr) as u64);
        let mut renderer = Renderer::new(sample_rate as f32);
        if let Some(fonts) = fonts {
            renderer.set_fonts(fonts);
        }
        renderer.load(Box::new(song));
        renderer.play(0.0, false);
        let skip = renderer.latency();
        OfflineRender {
            renderer,
            sample_rate: sr,
            song_frames,
            max_frames,
            emitted: 0,
            skip,
            quiet_run: 0,
            finished: false,
            fade_from: None,
        }
    }

    fn progress(&self) -> f32 {
        if self.finished {
            return 1.0;
        }
        let expected = (self.song_frames as f64 + 3.0 * self.sample_rate)
            .min(self.max_frames as f64)
            .max(1.0);
        ((self.emitted as f64 / expected) as f32).min(0.99)
    }

    /// Fills `l`/`r` (length ≤ MAX_BLOCK) and returns frames produced; 0 when finished.
    fn next(&mut self, l: &mut [f32], r: &mut [f32]) -> usize {
        if self.finished {
            return 0;
        }
        while self.skip > 0 {
            let n = self.skip.min(l.len());
            self.renderer.render(&mut l[..n], &mut r[..n]);
            self.skip -= n;
        }
        let remaining = self.max_frames - self.emitted;
        let n = (l.len() as u64).min(remaining) as usize;
        if n == 0 {
            self.finished = true;
            return 0;
        }
        self.renderer.render(&mut l[..n], &mut r[..n]);
        let start = self.emitted;
        self.emitted += n as u64;

        if self.emitted >= self.song_frames && !self.renderer.is_playing() {
            let (pl, pr) = self.renderer.peaks();
            if pl.max(pr) < TAIL_THRESHOLD && self.renderer.active_voices() == 0 {
                self.quiet_run += n as u64;
            } else {
                self.quiet_run = 0;
            }
            if self.quiet_run as f64 >= TAIL_QUIET_SECONDS * self.sample_rate
                || self.renderer.is_quiet()
            {
                self.finished = true;
            }
        }
        // Never cut a long tail abruptly at the hard limit.
        let fade_len = (FINAL_FADE_SECONDS * self.sample_rate) as u64;
        let fade_from = *self
            .fade_from
            .get_or_insert(self.max_frames.saturating_sub(fade_len));
        if self.emitted > fade_from && self.max_frames > self.song_frames {
            for (i, (sl, sr)) in l[..n].iter_mut().zip(r[..n].iter_mut()).enumerate() {
                let pos = start + i as u64;
                if pos >= fade_from {
                    let g = 1.0 - (pos - fade_from) as f32 / fade_len.max(1) as f32;
                    *sl *= g.max(0.0);
                    *sr *= g.max(0.0);
                }
            }
        }
        if self.emitted >= self.max_frames {
            self.finished = true;
        }
        n
    }
}

/// Renders `project` offline at `sample_rate`, stopping after the effect tail or at
/// `max_seconds`, whichever comes first.
pub fn render_project(
    project: &Project,
    sample_rate: u32,
    max_seconds: f64,
) -> Result<RenderedAudio, String> {
    let sample_rate = sample_rate.clamp(8_000, 192_000);
    let mut job = OfflineRender::new(project, sample_rate, max_seconds)?;
    let mut out = RenderedAudio {
        sample_rate,
        left: Vec::new(),
        right: Vec::new(),
    };
    let mut l = [0.0f32; MAX_BLOCK];
    let mut r = [0.0f32; MAX_BLOCK];
    loop {
        let n = job.next(&mut l, &mut r);
        if n == 0 {
            break;
        }
        out.left.extend_from_slice(&l[..n]);
        out.right.extend_from_slice(&r[..n]);
    }
    Ok(out)
}

fn partial_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export.wav".into());
    path.with_file_name(format!(".{name}.partial"))
}

/// 16-bit conversion with TPDF dither; exact digital silence stays silent.
fn to_i16(x: f32, rng: &mut Rng) -> i16 {
    if x == 0.0 {
        return 0;
    }
    let dither = (rng.unit() - rng.unit()) as f64;
    let v = (x.clamp(-1.0, 1.0) as f64) * 32767.0 + dither;
    v.round().clamp(-32768.0, 32767.0) as i16
}

/// Streams an in-progress render to `path` as a 16-bit stereo WAV at `sample_rate` via a
/// temporary sibling renamed into place only on success. Shared by [`export_wav`] and
/// [`export_stems`]; `progress` receives monotonically increasing values in 0..=1.
fn write_rendered_wav(
    job: &mut OfflineRender,
    path: &Path,
    sample_rate: u32,
    mut progress: impl FnMut(f32),
) -> Result<(), String> {
    let tmp = partial_path(path);
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let result = (|| -> Result<(), String> {
        let mut writer = hound::WavWriter::create(&tmp, spec)
            .map_err(|e| format!("Could not create {}: {e}", path.display()))?;
        let mut rng = Rng::new(0x5eed);
        let mut l = [0.0f32; MAX_BLOCK];
        let mut r = [0.0f32; MAX_BLOCK];
        let mut reported = -1.0f32;
        progress(0.0);
        loop {
            let n = job.next(&mut l, &mut r);
            if n == 0 {
                break;
            }
            for (&sl, &sr) in l[..n].iter().zip(r[..n].iter()) {
                writer
                    .write_sample(to_i16(sl, &mut rng))
                    .map_err(|e| format!("Could not write audio: {e}"))?;
                writer
                    .write_sample(to_i16(sr, &mut rng))
                    .map_err(|e| format!("Could not write audio: {e}"))?;
            }
            let p = job.progress();
            if p - reported >= 0.01 {
                reported = p;
                progress(p);
            }
        }
        writer
            .finalize()
            .map_err(|e| format!("Could not finish {}: {e}", path.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("Could not save {}: {e}", path.display()))
    })();
    match result {
        Ok(()) => {
            progress(1.0);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Renders `project` offline through the playback DSP and writes a 44.1 kHz 16-bit stereo
/// WAV including reverb/delay tails. Mute, solo and master volume are honoured. The file is
/// streamed to a temporary sibling and renamed into place only on success. `progress`
/// receives monotonically increasing values in 0..=1, ending with 1.0.
pub fn export_wav(project: &Project, path: &Path, progress: impl FnMut(f32)) -> Result<(), String> {
    let max_seconds = project.duration_seconds() + MAX_TAIL_SECONDS;
    let mut job = OfflineRender::new(project, EXPORT_SAMPLE_RATE, max_seconds)?;
    write_rendered_wav(&mut job, path, EXPORT_SAMPLE_RATE, progress)
}

/// Why track `index` gets no stem: the mixdown's gate is honoured exactly, in the same
/// order as [`Project::is_track_audible`]. The track's bus is checked first — a muted bus
/// silences its whole group (even a soloed track on it), and while any bus is soloed every
/// track on a non-soloed bus (including master-routed tracks) is gated too. Then the
/// track's own gate applies: a muted track is skipped and, while any track is soloed,
/// every non-solo track is skipped. `None` means the track is audible in the mix and
/// produces a stem. Callers use the reason to annotate reports ("skipped …").
pub fn stem_skip_reason(project: &Project, index: usize) -> Option<&'static str> {
    let track = project.tracks.get(index)?;
    if index >= MAX_TRACKS {
        Some("over the track limit")
    } else if !project.is_bus_audible(track.bus) {
        // A gated bus: distinguish the mute case for the report. (An
        // out-of-range route — only reachable on unvalidated data — reads
        // as inaudible here too, the same way `is_bus_audible` treats it.)
        if track.bus != 0
            && project
                .buses
                .get(track.bus as usize - 1)
                .is_some_and(|b| b.mute)
        {
            Some("on a muted bus")
        } else {
            Some("bus not soloed")
        }
    } else if track.mute {
        Some("muted")
    } else if project.has_solo() && !track.solo {
        Some("not soloed")
    } else {
        None
    }
}

/// Track name made safe for a file name (same rules as song file names), with a `track`
/// fallback for names that sanitize to nothing.
fn stem_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "track".to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// Stem file name for track `index`: `<name>-NN.wav` where NN is the two-digit track
/// number. The index suffix keeps duplicate or sanitized-equal names unique in one export.
fn stem_file_name(project: &Project, index: usize) -> String {
    let name = project
        .tracks
        .get(index)
        .map_or("", |track| track.name.as_str());
    format!("{}-{:02}.wav", stem_name(name), index + 1)
}

/// Renders every audible track of `project` to its own 16-bit stereo WAV in `out_dir`
/// (created when missing) and returns the written paths in track order. Each stem is a
/// full render through the playback DSP with only that one track audible — the same signal
/// it contributes to the mixdown, including its strip (drive/filter/fader/pan), its share
/// of the reverb/delay sends and the master chain. Mute/solo is honoured like the mix —
/// the track's own gate and its bus's gate (see [`Project::is_track_audible`]) — and
/// tracks that are not audible are skipped; [`stem_skip_reason`] says why.
///
/// `fonts` is the already-loaded SoundFont catalog, shared by every stem so `.sf2` files
/// are parsed once by the caller. `progress` receives 0..=1 across the whole batch, ending
/// with 1.0.
pub fn export_stems(
    project: &Project,
    fonts: &FontLibrary,
    out_dir: &Path,
    sample_rate: u32,
    mut progress: impl FnMut(f32),
) -> Result<Vec<PathBuf>, String> {
    project
        .validate()
        .map_err(|e| format!("Cannot render this song: {e}"))?;
    let sample_rate = sample_rate.clamp(8_000, 192_000);
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("Could not create {}: {e}", out_dir.display()))?;
    let song = SongData::from_project(project);
    let seconds = project.duration_seconds();
    let max_seconds = seconds + MAX_TAIL_SECONDS;
    // One shared catalog for every stem; referenced only when the song uses fonts.
    let fonts = project
        .tracks
        .iter()
        .any(|t| t.font.is_some())
        .then(|| Arc::new(fonts.clone()));
    let targets: Vec<(usize, PathBuf)> = (0..song.tracks.len())
        .filter(|&i| stem_skip_reason(project, i).is_none())
        .map(|i| (i, out_dir.join(stem_file_name(project, i))))
        .collect();
    if targets.is_empty() {
        return Err(
            "Nothing to export: every track is muted or silenced by a mute/solo on the track or its bus."
                .to_owned(),
        );
    }
    let count = targets.len() as f32;
    let mut written = Vec::with_capacity(targets.len());
    for (k, (index, path)) in targets.iter().enumerate() {
        // Solo this one track: audibility is applied at event-trigger time and
        // at the strip gain, so the other tracks contribute nothing — no
        // voices, no send signal — to the render.
        let mut stem = song.clone();
        for (j, track) in stem.tracks.iter_mut().enumerate() {
            track.audible = j == *index;
        }
        let mut job =
            OfflineRender::from_song(stem, seconds, fonts.clone(), sample_rate, max_seconds);
        let base = k as f32 / count;
        write_rendered_wav(&mut job, path, sample_rate, |p| {
            progress((base + p / count).clamp(0.0, 1.0));
        })?;
        written.push(path.clone());
    }
    progress(1.0);
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{create_track, Effects, Instrument, Note, Project};

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    fn single(instrument: Instrument, pitch: u8, effects: Effects) -> Project {
        let mut p = Project::empty();
        p.bpm = 120.0;
        p.bars = 1;
        p.tracks = vec![create_track(instrument)];
        p.tracks[0].effects = effects;
        p.tracks[0].patterns[0].notes = vec![Note::new(pitch, 0.0, 2.0, 0.9)];
        p
    }

    const DRY: Effects = Effects {
        cutoff_hz: 20_000.0,
        reverb: 0.0,
        delay: 0.0,
        drive: 0.0,
    };

    #[test]
    fn demo_renders_loud_clean_audio_in_every_bar() {
        let demo = Project::demo();
        let a = render_project(&demo, 22_050, 60.0).unwrap();
        let song = (demo.duration_seconds() * 22_050.0) as usize;
        assert!(a.left.len() > song && a.left.len() < song + 13 * 22_050);
        let all: Vec<f32> = a.left.iter().chain(&a.right).copied().collect();
        assert!(all.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        let peak = all.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let level = rms(&a.left[..song]);
        // The default mix should land near -6 dBFS peak / -18 dB RMS: loud and healthy,
        // with the limiter still only a safety net rather than part of the sound.
        assert!(peak > 0.4 && peak < 0.9, "demo peak {peak}");
        assert!(level > 0.06 && level < 0.4, "demo rms {level}");
        let bar = song / 8;
        for b in 0..8 {
            assert!(rms(&a.left[b * bar..(b + 1) * bar]) > 0.03, "bar {b} quiet");
        }
        // The tail rings out after the last bar and ends in silence.
        assert!(rms(&a.left[song..song + 2205]) > 1e-3, "tail present");
        assert!(a.left[a.left.len() - 100..].iter().all(|s| s.abs() < 1e-3));
    }

    #[test]
    fn mute_solo_and_master_are_honoured() {
        let mut p = Project::demo();
        p.bars = 2;
        for t in &mut p.tracks {
            t.mute = true;
        }
        let a = render_project(&p, 22_050, 30.0).unwrap();
        assert!(a.left.iter().chain(&a.right).all(|s| *s == 0.0));

        let mut p = Project::demo();
        p.bars = 2;
        p.master_volume = 0.0;
        let a = render_project(&p, 22_050, 30.0).unwrap();
        assert!(a.left.iter().chain(&a.right).all(|s| *s == 0.0));

        // Solo equals rendering that track alone.
        let mut solo = Project::demo();
        solo.bars = 2;
        solo.tracks[1].solo = true;
        let mut alone = solo.clone();
        alone.tracks = vec![alone.tracks[1].clone()];
        let a = render_project(&solo, 22_050, 30.0).unwrap();
        let b = render_project(&alone, 22_050, 30.0).unwrap();
        assert!(rms(&a.left) > 0.01);
        assert_eq!(a, b);
    }

    #[test]
    fn pan_and_effects_shape_the_sound() {
        let mut p = single(Instrument::Piano, 60, DRY);
        p.tracks[0].pan = -1.0;
        let a = render_project(&p, 22_050, 10.0).unwrap();
        assert!(
            rms(&a.left) > 0.01 && rms(&a.right) < rms(&a.left) * 0.01,
            "hard left"
        );
        p.tracks[0].pan = 1.0;
        let a = render_project(&p, 22_050, 10.0).unwrap();
        assert!(
            rms(&a.right) > 0.01 && rms(&a.left) < rms(&a.right) * 0.01,
            "hard right"
        );

        let sr = 22_050usize;
        let dry = render_project(&single(Instrument::Pluck, 72, DRY), 22_050, 10.0).unwrap();
        let wet = render_project(
            &single(Instrument::Pluck, 72, Effects { reverb: 1.0, ..DRY }),
            22_050,
            10.0,
        )
        .unwrap();
        assert!(wet.left.len() > dry.left.len(), "reverb extends the tail");
        let tail = |x: &RenderedAudio| rms(&x.left[sr * 2..(sr * 3).min(x.left.len())]);
        assert!(tail(&wet) > tail(&dry) * 3.0 + 1e-4, "reverb tail energy");

        let echo = render_project(
            &single(Instrument::Drums, 38, Effects { delay: 1.0, ..DRY }),
            22_050,
            10.0,
        )
        .unwrap();
        // Dotted eighth at 120 BPM = 375 ms; the dry snare is long gone by then.
        let at = (0.375 * sr as f64) as usize;
        assert!(rms(&echo.left[at..at + 2000]) > 0.01, "delay echo");
        // The ping-pong hop arrives on the right channel another 375 ms later.
        let second = (0.75 * sr as f64) as usize;
        assert!(
            rms(&echo.right[second..second + 2000]) > 0.005,
            "ping-pong echo"
        );

        let bright = render_project(&single(Instrument::Pad, 48, DRY), 22_050, 10.0).unwrap();
        let dark = render_project(
            &single(
                Instrument::Pad,
                48,
                Effects {
                    cutoff_hz: 200.0,
                    ..DRY
                },
            ),
            22_050,
            10.0,
        )
        .unwrap();
        let hf = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
        assert!(
            hf(&dark.left[..sr]) < hf(&bright.left[..sr]) * 0.3,
            "filter darkens"
        );

        let clean = render_project(&single(Instrument::Piano, 48, DRY), 22_050, 10.0).unwrap();
        let driven = render_project(
            &single(Instrument::Piano, 48, Effects { drive: 1.0, ..DRY }),
            22_050,
            10.0,
        )
        .unwrap();
        let crest = |x: &[f32]| x.iter().fold(0.0f32, |m, s| m.max(s.abs())) / rms(x);
        assert!(
            crest(&driven.left[..sr]) < crest(&clean.left[..sr]) * 0.85,
            "drive compresses peaks"
        );
    }

    #[test]
    fn note_timing_in_render_is_exact() {
        let mut p = single(Instrument::Drums, 36, DRY);
        p.tracks[0].patterns[0].notes = vec![Note::new(36, 8.0, 1.0, 1.0)];
        let a = render_project(&p, 48_000, 10.0).unwrap();
        // 120 BPM: 6000 frames per step; latency is compensated.
        assert_eq!(a.left.iter().position(|s| s.abs() > 1e-6), Some(48_000));
    }

    #[test]
    fn preset_tracks_render_without_a_catalog() {
        let pack = crate::packs::template_pack("unit-pack", "Unit").unwrap();
        let mut p = single(Instrument::Lead, 64, DRY);
        p.tracks[0].preset = Some(pack.instruments[0].clone());
        let json = serde_json::to_string(&p).unwrap();
        let loaded: Project = serde_json::from_str(&json).unwrap();
        loaded.validate().unwrap();
        let custom = render_project(&loaded, 22_050, 10.0).unwrap();
        let builtin = render_project(&single(Instrument::Lead, 64, DRY), 22_050, 10.0).unwrap();
        // The template preset is percussive (sustain 0), so judge it over its sounding
        // window; a preset voice must sit at comparable level to a builtin.
        let sounding = &custom.left[..(11_025usize).min(custom.left.len())];
        let peak = sounding.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak > 0.04 && rms(sounding) > 0.006,
            "preset audible (peak {peak}, window rms {})",
            rms(sounding)
        );
        assert_ne!(
            custom.left,
            builtin.left[..custom.left.len().min(builtin.left.len())].to_vec()
        );
    }

    #[test]
    fn wav_export_writes_complete_file() {
        let dir = std::env::temp_dir().join(format!("cadence-audio-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("one bar.wav");
        let p = single(
            Instrument::Piano,
            60,
            Effects {
                reverb: 0.3,
                delay: 0.2,
                ..DRY
            },
        );
        let mut seen = Vec::new();
        export_wav(&p, &path, |x| seen.push(x)).unwrap();
        assert!(seen.windows(2).all(|w| w[1] >= w[0]) && *seen.last().unwrap() == 1.0);
        let mut reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (2, EXPORT_SAMPLE_RATE, 16)
        );
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        let frames = samples.len() / 2;
        assert!(
            frames as f64 >= 2.0 * 44_100.0,
            "song plus tail, got {frames}"
        );
        assert!(samples.iter().any(|s| s.unsigned_abs() > 1000));
        assert!(!partial_path(&path).exists());
        let bad = dir.join("missing").join("x.wav");
        assert!(export_wav(&p, &bad, |_| {}).is_err());
        let mut invalid = p.clone();
        invalid.bpm = f64::NAN;
        assert!(export_wav(&invalid, &path, |_| {}).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// One bar: a piano track ("Keys/Lead") plus a bass track, one note each.
    fn two_track_song() -> Project {
        let mut p = Project::empty();
        p.bars = 1;
        let mut keys = create_track(Instrument::Piano);
        keys.name = "Keys/Lead".to_owned();
        keys.patterns[0].notes = vec![Note::new(60, 0.0, 4.0, 0.9)];
        let mut bass = create_track(Instrument::Bass);
        bass.name = "Bass".to_owned();
        bass.patterns[0].notes = vec![Note::new(36, 0.0, 4.0, 0.9)];
        p.tracks = vec![keys, bass];
        p
    }

    #[test]
    fn stem_export_writes_one_wav_per_audible_track() {
        let dir =
            std::env::temp_dir().join(format!("cadence-stem-per-track-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = two_track_song();
        let fonts = FontLibrary::empty();
        let out = dir.join("stems");

        let mut seen = Vec::new();
        let written = export_stems(&p, &fonts, &out, EXPORT_SAMPLE_RATE, |x| seen.push(x))
            .expect("stem export");
        assert_eq!(written.len(), 2);
        // Sanitized track name plus the zero-padded track number.
        assert!(written[0].ends_with("Keys_Lead-01.wav"), "{:?}", written[0]);
        assert!(written[1].ends_with("Bass-02.wav"), "{:?}", written[1]);
        assert!(seen.windows(2).all(|w| w[1] >= w[0]) && *seen.last().unwrap() == 1.0);
        let a = std::fs::read(&written[0]).unwrap();
        let b = std::fs::read(&written[1]).unwrap();
        assert_ne!(a, b, "stems of different tracks must differ");
        for path in &written {
            let mut reader = hound::WavReader::open(path).unwrap();
            assert_eq!(
                (reader.spec().channels, reader.spec().sample_rate),
                (2, EXPORT_SAMPLE_RATE)
            );
            assert!(reader
                .samples::<i16>()
                .any(|s| s.unwrap().unsigned_abs() > 100));
        }

        // A stem is exactly the mix render with that track soloed (same DSP,
        // same deterministic dither): full wet stem including its send share.
        let mut soloed = p.clone();
        soloed.tracks[1].solo = true;
        let solo_path = dir.join("solo-mix.wav");
        export_wav(&soloed, &solo_path, |_| {}).unwrap();
        assert_eq!(std::fs::read(&solo_path).unwrap(), b);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stem_export_honours_mute_and_solo() {
        let dir =
            std::env::temp_dir().join(format!("cadence-stem-mute-solo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fonts = FontLibrary::empty();
        let mut p = two_track_song();

        // Muted track is skipped; the remaining track still renders alone.
        p.tracks[1].mute = true;
        assert_eq!(stem_skip_reason(&p, 1), Some("muted"));
        let written = export_stems(&p, &fonts, &dir.join("muted"), EXPORT_SAMPLE_RATE, |_| {})
            .expect("stem export");
        assert_eq!(written.len(), 1);
        assert!(written[0].ends_with("Keys_Lead-01.wav"));

        // While a track is soloed only soloed, unmuted tracks get stems.
        let mut p = two_track_song();
        p.tracks[0].solo = true;
        assert_eq!(stem_skip_reason(&p, 1), Some("not soloed"));
        let written = export_stems(&p, &fonts, &dir.join("solo"), EXPORT_SAMPLE_RATE, |_| {})
            .expect("stem export");
        assert_eq!(written.len(), 1);
        assert!(written[0].ends_with("Keys_Lead-01.wav"));

        // Everything muted is an error, not an empty success.
        let mut p = two_track_song();
        for track in &mut p.tracks {
            track.mute = true;
        }
        assert!(export_stems(&p, &fonts, &dir.join("none"), EXPORT_SAMPLE_RATE, |_| {}).is_err());

        // An invalid song is rejected before any file appears.
        let mut invalid = two_track_song();
        invalid.bpm = f64::NAN;
        assert!(export_stems(
            &invalid,
            &fonts,
            &dir.join("bad"),
            EXPORT_SAMPLE_RATE,
            |_| {}
        )
        .is_err());
        assert!(!dir.join("bad").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stem_export_honours_the_bus_gate() {
        let dir = std::env::temp_dir().join(format!("cadence-stem-bus-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fonts = FontLibrary::empty();
        let mut p = two_track_song();

        // A track on a muted bus is skipped, solo or not.
        p.tracks[1].bus = 1;
        p.buses[0].mute = true;
        assert_eq!(stem_skip_reason(&p, 1), Some("on a muted bus"));
        let written = export_stems(
            &p,
            &fonts,
            &dir.join("muted-bus"),
            EXPORT_SAMPLE_RATE,
            |_| {},
        )
        .expect("stem export");
        assert_eq!(written.len(), 1);
        assert!(written[0].ends_with("Keys_Lead-01.wav"));
        p.tracks[1].solo = true;
        assert_eq!(stem_skip_reason(&p, 1), Some("on a muted bus"));

        // While a bus is soloed, tracks on other buses — and master-routed
        // tracks — are gated with their own reason.
        let mut p = two_track_song();
        p.tracks[0].bus = 1;
        p.tracks[1].bus = 2;
        p.buses[0].solo = true;
        assert_eq!(stem_skip_reason(&p, 0), None);
        assert_eq!(stem_skip_reason(&p, 1), Some("bus not soloed"));
        let written = export_stems(
            &p,
            &fonts,
            &dir.join("bus-solo"),
            EXPORT_SAMPLE_RATE,
            |_| {},
        )
        .expect("stem export");
        assert_eq!(written.len(), 1);
        assert!(written[0].ends_with("Keys_Lead-01.wav"));

        // A bus-gated song with nothing else audible is an error, like an
        // all-muted one.
        let mut p = two_track_song();
        p.buses[0].mute = true;
        p.tracks[0].bus = 1;
        p.tracks[1].bus = 1;
        assert!(export_stems(&p, &fonts, &dir.join("none"), EXPORT_SAMPLE_RATE, |_| {}).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stem_file_names_are_safe_and_unique() {
        let mut p = two_track_song();
        p.tracks[0].name = "  a/b\\c:d  ".to_owned();
        p.tracks[1].name = "a/b\\c:d".to_owned();
        assert_eq!(stem_file_name(&p, 0), "a_b_c_d-01.wav");
        // Equal sanitized names are still unique via the track number.
        assert_eq!(stem_file_name(&p, 1), "a_b_c_d-02.wav");
        p.tracks[0].name = "   ".to_owned();
        assert_eq!(stem_file_name(&p, 0), "track-01.wav");
    }
}
