//! Real-time playback through CPAL.
//!
//! A dedicated thread opens the default output device and owns the `cpal::Stream` (which is
//! `!Send` on some platforms), so [`AudioEngine`] itself is `Send + Sync`. Control methods
//! never block: they build song snapshots on the caller's thread and push them through a
//! bounded lock-free channel. The audio callback drains that channel, renders with the
//! shared [`Renderer`], publishes status through atomics, and hands replaced snapshots back
//! through a second channel so memory is freed off the audio thread. If the device
//! disappears the thread keeps retrying the current default device every two seconds.

use super::renderer::{Command, Renderer, MAX_BLOCK};
use super::song::{SongData, TrackParams};
use crate::model::{Project, Track};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamError};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::JoinHandle;
use std::time::Duration;

const COMMAND_CAPACITY: usize = 256;
const GARBAGE_CAPACITY: usize = 64;
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const METER_RELEASE_SECONDS: f32 = 0.25;

/// Snapshot of the transport for the UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransportStatus {
    pub playing: bool,
    /// Playhead in steps (swung timeline; equals the grid on even steps).
    pub step: f64,
    /// Output peak meters, linear 0..1, with a short release.
    pub peak_l: f32,
    pub peak_r: f32,
    pub voices: usize,
    /// Latest device/stream problem, if any.
    pub error: Option<String>,
}

#[derive(Default)]
struct Shared {
    playing: AtomicBool,
    step: AtomicU64,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    voices: AtomicUsize,
    error: Mutex<Option<String>>,
}

impl Shared {
    fn set_error(&self, message: Option<String>) {
        if let Ok(mut e) = self.error.lock() {
            *e = message;
        }
    }
}

enum ThreadMsg {
    Shutdown,
    StreamFailed { message: String, fatal: bool },
}

/// State owned by the audio callback.
struct RealTime {
    renderer: Renderer,
    commands: Receiver<Command>,
    garbage: Sender<Box<SongData>>,
    shared: Arc<Shared>,
    left: Vec<f32>,
    right: Vec<f32>,
    meter_l: f32,
    meter_r: f32,
}

impl RealTime {
    fn fill<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T], channels: usize) {
        while let Ok(cmd) = self.commands.try_recv() {
            if let Some(old) = self.renderer.apply(cmd) {
                // If the return queue is full the snapshot is dropped here; that is rare and
                // bounded (one free), never a blocking operation.
                if let Err(TrySendError::Full(old) | TrySendError::Disconnected(old)) =
                    self.garbage.try_send(old)
                {
                    drop(old);
                }
            }
        }
        let channels = channels.max(1);
        let frames = data.len() / channels;
        let mut done = 0;
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.renderer
                .render(&mut self.left[..n], &mut self.right[..n]);
            let (bl, br) = self.renderer.peaks();
            pl = pl.max(bl);
            pr = pr.max(br);
            for (i, frame) in data[done * channels..(done + n) * channels]
                .chunks_exact_mut(channels)
                .enumerate()
            {
                let (l, r) = (self.left[i], self.right[i]);
                if channels == 1 {
                    frame[0] = T::from_sample((l + r) * 0.5);
                } else {
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                    for s in &mut frame[2..] {
                        *s = T::EQUILIBRIUM;
                    }
                }
            }
            done += n;
        }
        for s in &mut data[frames * channels..] {
            *s = T::EQUILIBRIUM;
        }
        let decay =
            (-(frames as f32) / (METER_RELEASE_SECONDS * self.renderer.sample_rate())).exp();
        self.meter_l = pl.max(self.meter_l * decay);
        self.meter_r = pr.max(self.meter_r * decay);
        let s = &self.shared;
        s.playing
            .store(self.renderer.is_playing(), Ordering::Relaxed);
        s.step
            .store(self.renderer.position().to_bits(), Ordering::Relaxed);
        s.peak_l.store(self.meter_l.to_bits(), Ordering::Relaxed);
        s.peak_r.store(self.meter_r.to_bits(), Ordering::Relaxed);
        s.voices
            .store(self.renderer.active_voices(), Ordering::Relaxed);
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<RealTime>>,
    control: Sender<ThreadMsg>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = config.channels as usize;
    device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| match state.try_lock() {
            Ok(mut rt) => rt.fill(data, channels),
            Err(TryLockError::Poisoned(p)) => p.into_inner().fill(data, channels),
            // Only contended while the device is being reopened.
            Err(TryLockError::WouldBlock) => data.iter_mut().for_each(|s| *s = T::EQUILIBRIUM),
        },
        move |err: StreamError| {
            let fatal = matches!(err, StreamError::DeviceNotAvailable);
            let _ = control.try_send(ThreadMsg::StreamFailed {
                message: err.to_string(),
                fatal,
            });
        },
        None,
    )
}

struct DeviceInfo {
    name: String,
    sample_rate: u32,
}

fn open_stream(
    state: &Arc<Mutex<RealTime>>,
    control: &Sender<ThreadMsg>,
) -> Result<(cpal::Stream, DeviceInfo), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No audio output device is available.".to_string())?;
    let name = device.name().unwrap_or_else(|_| "Audio output".to_string());
    let supported = device
        .default_output_config()
        .map_err(|e| format!("Could not read the settings of \"{name}\": {e}"))?;
    let format = supported.sample_format();
    let config = supported.config();
    let sample_rate = config.sample_rate.0;
    {
        let mut rt = state.lock().unwrap_or_else(|p| p.into_inner());
        rt.renderer.set_sample_rate(sample_rate as f32);
    }
    let st = Arc::clone(state);
    let ctl = control.clone();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, st, ctl),
        SampleFormat::F64 => build::<f64>(&device, &config, st, ctl),
        SampleFormat::I16 => build::<i16>(&device, &config, st, ctl),
        SampleFormat::I32 => build::<i32>(&device, &config, st, ctl),
        SampleFormat::I8 => build::<i8>(&device, &config, st, ctl),
        SampleFormat::U16 => build::<u16>(&device, &config, st, ctl),
        SampleFormat::U32 => build::<u32>(&device, &config, st, ctl),
        SampleFormat::U8 => build::<u8>(&device, &config, st, ctl),
        other => {
            return Err(format!(
                "\"{name}\" uses an unsupported sample format ({other})."
            ))
        }
    }
    .map_err(|e| format!("Could not open \"{name}\": {e}"))?;
    stream
        .play()
        .map_err(|e| format!("Could not start \"{name}\": {e}"))?;
    Ok((stream, DeviceInfo { name, sample_rate }))
}

fn audio_thread(
    state: Arc<Mutex<RealTime>>,
    shared: Arc<Shared>,
    control_rx: Receiver<ThreadMsg>,
    control_tx: Sender<ThreadMsg>,
    ready: Sender<Result<DeviceInfo, String>>,
) {
    let mut stream = match open_stream(&state, &control_tx) {
        Ok((stream, info)) => {
            let _ = ready.send(Ok(info));
            Some(stream)
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    loop {
        match control_rx.recv_timeout(RETRY_INTERVAL) {
            Ok(ThreadMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(ThreadMsg::StreamFailed { message, fatal }) => {
                shared.set_error(Some(format!("Audio device problem: {message}")));
                if fatal {
                    stream = None;
                    shared.playing.store(false, Ordering::Relaxed);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if stream.is_none() {
            if let Ok((s, info)) = open_stream(&state, &control_tx) {
                stream = Some(s);
                shared.set_error(Some(format!(
                    "Audio reconnected to \"{}\" ({} Hz).",
                    info.name, info.sample_rate
                )));
            }
        }
    }
    drop(stream);
}

/// Owns the native audio output and the real-time renderer.
pub struct AudioEngine {
    commands: Sender<Command>,
    garbage: Receiver<Box<SongData>>,
    shared: Arc<Shared>,
    control: Sender<ThreadMsg>,
    thread: Option<JoinHandle<()>>,
    sample_rate: u32,
    device_name: String,
}

impl AudioEngine {
    /// Opens the default output device. Returns a readable error (and leaves nothing running)
    /// when no device is usable, so the app can still edit and export offline.
    pub fn new() -> Result<Self, String> {
        let (commands, command_rx) = bounded(COMMAND_CAPACITY);
        let (garbage_tx, garbage) = bounded(GARBAGE_CAPACITY);
        let (control, control_rx) = bounded(16);
        let (ready_tx, ready_rx) = bounded(1);
        let shared = Arc::new(Shared::default());
        let state = Arc::new(Mutex::new(RealTime {
            renderer: Renderer::new(48_000.0),
            commands: command_rx,
            garbage: garbage_tx,
            shared: Arc::clone(&shared),
            left: vec![0.0; MAX_BLOCK],
            right: vec![0.0; MAX_BLOCK],
            meter_l: 0.0,
            meter_r: 0.0,
        }));
        let thread_shared = Arc::clone(&shared);
        let thread_control = control.clone();
        let thread = std::thread::Builder::new()
            .name("cadence-audio".into())
            .spawn(move || audio_thread(state, thread_shared, control_rx, thread_control, ready_tx))
            .map_err(|e| format!("Could not start the audio thread: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(info)) => Ok(AudioEngine {
                commands,
                garbage,
                shared,
                control,
                thread: Some(thread),
                sample_rate: info.sample_rate,
                device_name: info.name,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = control.try_send(ThreadMsg::Shutdown);
                Err("The audio device did not respond.".to_string())
            }
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    fn send(&self, cmd: Command) -> Result<(), String> {
        while self.garbage.try_recv().is_ok() {}
        self.commands.try_send(cmd).map_err(|e| match e {
            TrySendError::Full(_) => "The audio engine is busy; try again.".to_string(),
            TrySendError::Disconnected(_) => "The audio engine has stopped.".to_string(),
        })
    }

    /// Starts playback of `project` from `start_step` (steps), optionally looping the song.
    /// `loop_region` is the repeat span in steps while `looping` is on (`None`
    /// wraps at the song end).
    pub fn play(
        &self,
        project: &Project,
        start_step: f64,
        looping: bool,
        loop_region: Option<(f64, f64)>,
    ) -> Result<(), String> {
        let song = Box::new(SongData::from_project(project));
        self.send(Command::Play {
            song,
            start_step,
            looping,
            loop_region,
        })?;
        // Reflect the request immediately; the callback confirms within one buffer.
        self.shared.playing.store(true, Ordering::Relaxed);
        self.shared
            .step
            .store(start_step.max(0.0).to_bits(), Ordering::Relaxed);
        Ok(())
    }

    /// Halts at the current position; notes release and effect tails ring out.
    pub fn pause(&self) {
        let _ = self.send(Command::Pause);
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    /// Stops, rewinds to the start and silences all voices and effect tails (panic).
    pub fn stop(&self) {
        let _ = self.send(Command::Stop);
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.step.store(0f64.to_bits(), Ordering::Relaxed);
    }

    /// Updates the song while keeping the transport running (edits, mixer moves, mute/solo).
    pub fn set_project(&self, project: &Project) {
        let _ = self.send(Command::Load(Box::new(SongData::from_project(project))));
    }

    /// Updates the loop region without restarting the transport — used while
    /// the region is dragged on the ruler during playback (`None` wraps at
    /// the song end again).
    pub fn set_loop_region(&self, region: Option<(f64, f64)>) {
        let _ = self.send(Command::SetLoopRegion(region));
    }

    /// Auditions one note with `track`'s sound and mixer settings, even when stopped or muted.
    pub fn preview(&self, track: &Track, pitch: u8, duration_seconds: f64, velocity: f32) {
        let params = TrackParams::from_track(track, true);
        let _ = self.send(Command::Preview {
            params,
            pitch,
            velocity,
            seconds: duration_seconds,
        });
    }

    pub fn set_metronome(&self, enabled: bool) {
        let _ = self.send(Command::Metronome(enabled));
    }

    /// Installs the SoundFont catalog for tracks with `Track::font` set.
    /// Fonts are parsed on the caller's thread ([`crate::font::load_library`])
    /// and pushed to the audio callback — typically once at startup and again
    /// when the font folder changes. An empty catalog makes font tracks fall
    /// back to their preset/built-in sound.
    pub fn set_fonts(&self, library: Arc<crate::font::FontLibrary>) {
        let _ = self.send(Command::SetFonts(library));
    }

    pub fn status(&self) -> TransportStatus {
        let s = &self.shared;
        TransportStatus {
            playing: s.playing.load(Ordering::Relaxed),
            step: f64::from_bits(s.step.load(Ordering::Relaxed)),
            peak_l: f32::from_bits(s.peak_l.load(Ordering::Relaxed)),
            peak_r: f32::from_bits(s.peak_r.load(Ordering::Relaxed)),
            voices: s.voices.load(Ordering::Relaxed),
            error: s.error.lock().ok().and_then(|e| e.clone()),
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        let _ = self.control.try_send(ThreadMsg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn engine_is_shareable() {
        assert_send_sync::<AudioEngine>();
    }

    #[test]
    fn callback_state_renders_commands_without_a_device() {
        let (tx, rx) = bounded(8);
        let (gtx, grx) = bounded(8);
        let shared = Arc::new(Shared::default());
        let mut rt = RealTime {
            renderer: Renderer::new(44_100.0),
            commands: rx,
            garbage: gtx,
            shared: Arc::clone(&shared),
            left: vec![0.0; MAX_BLOCK],
            right: vec![0.0; MAX_BLOCK],
            meter_l: 0.0,
            meter_r: 0.0,
        };
        let demo = Project::demo();
        tx.send(Command::Play {
            song: Box::new(SongData::from_project(&demo)),
            start_step: 0.0,
            looping: true,
            loop_region: None,
        })
        .unwrap();
        // Interleaved i16 stereo, odd buffer size.
        let mut out = vec![0i16; 2 * 1023];
        for _ in 0..60 {
            rt.fill(&mut out, 2);
        }
        assert!(shared.playing.load(Ordering::Relaxed));
        assert!(f64::from_bits(shared.step.load(Ordering::Relaxed)) > 1.0);
        assert!(f32::from_bits(shared.peak_l.load(Ordering::Relaxed)) > 0.01);
        assert!(out.iter().any(|s| s.unsigned_abs() > 300));
        assert!(
            grx.try_recv().is_ok(),
            "replaced snapshot handed back for disposal"
        );
        // Mono and multichannel devices.
        let mut mono = vec![0.0f32; 512];
        rt.fill(&mut mono, 1);
        let mut six = vec![1.0f32; 6 * 128];
        rt.fill(&mut six, 6);
        assert!(six.chunks(6).all(|f| f[2..].iter().all(|s| *s == 0.0)));
        tx.send(Command::Stop).unwrap();
        let mut out = vec![0.0f32; 2 * 4096];
        rt.fill(&mut out, 2);
        rt.fill(&mut out, 2);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(shared.voices.load(Ordering::Relaxed), 0);
    }

    /// Opens the real device when one exists. Never plays notes, so it is silent.
    #[test]
    #[ignore = "needs an audio output device"]
    fn real_device_lifecycle() {
        match AudioEngine::new() {
            Ok(engine) => {
                assert!(engine.sample_rate() >= 8_000);
                engine.set_metronome(false);
                engine.set_project(&Project::empty());
                engine.stop();
                std::thread::sleep(Duration::from_millis(200));
                let s = engine.status();
                assert!(!s.playing);
            }
            Err(e) => assert!(!e.is_empty()),
        }
    }
}
