//! Real-time playback through CPAL.
//!
//! A dedicated thread opens the default output device and owns the `cpal::Stream` (which is
//! `!Send` on some platforms), so [`AudioEngine`] itself is `Send + Sync`. Control methods
//! never block: they build song snapshots on the caller's thread and push them through a
//! bounded lock-free channel. The audio callback drains that channel, renders with the
//! shared [`Renderer`], publishes status through atomics, and hands replaced snapshots back
//! through a second channel so memory is freed off the audio thread. If the device
//! disappears the thread keeps retrying the current default device every two seconds.

use super::font::{FontBank, FontSynth};
use super::meter::{BlockPeaks, MixerPeaks, SharedMeter};
use super::renderer::{Command, Renderer, MAX_BLOCK};
use super::song::{SongData, TrackParams};
use super::voice::Sound;
use crate::font::FontLibrary;
use crate::model::{Project, Track};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamError};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const COMMAND_CAPACITY: usize = 256;
const GARBAGE_CAPACITY: usize = COMMAND_CAPACITY;
const MAX_COMMANDS_PER_BLOCK: usize = 8;
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
    mixer: SharedMeter,
    error: Mutex<Option<String>>,
    device_wakeup: OnceLock<Box<dyn Fn() + Send + Sync>>,
    connected: AtomicBool,
    shutdown: AtomicBool,
    sample_rate: AtomicU32,
}

impl Shared {
    fn set_error(&self, message: Option<String>) {
        if let Ok(mut e) = self.error.lock() {
            *e = message;
        }
        if let Some(wakeup) = self.device_wakeup.get() {
            wakeup();
        }
    }
}

enum ThreadMsg {
    Shutdown,
    StreamFailed { generation: u64, error: StreamError },
}

#[derive(Default)]
struct AudioCommand {
    render: Option<Command>,
    fonts: Option<FontBank>,
    preview: Option<Box<FontSynth>>,
}

impl From<Command> for AudioCommand {
    fn from(render: Command) -> Self {
        Self {
            render: Some(render),
            ..Self::default()
        }
    }
}

// Owns every heap-backed value replaced by a command. This is dropped only
// by the caller or stream owner, never by the output callback.
#[derive(Default)]
struct Retired {
    song: Option<Box<SongData>>,
    fonts: Option<FontBank>,
    preview: Option<Box<FontSynth>>,
}

impl Retired {
    fn is_empty(&self) -> bool {
        self.song.is_none() && self.fonts.is_none() && self.preview.is_none()
    }
}

/// Caller-side SoundFont preparation state. The output callback never sees
/// this lock; the stream owner takes it only while switching devices.
struct Preparation {
    rate: f32,
    library: Arc<FontLibrary>,
    bindings: Vec<(u64, u64)>,
    preview: Option<u64>,
}

impl Preparation {
    fn new(rate: f32) -> Self {
        Self {
            rate,
            library: Arc::new(FontLibrary::empty()),
            bindings: Vec::new(),
            preview: None,
        }
    }
}

/// State owned by the audio callback.
struct RealTime {
    renderer: Renderer,
    commands: Receiver<AudioCommand>,
    garbage: Sender<Retired>,
    pending_retirement: Option<Retired>,
    shared: Arc<Shared>,
    left: Vec<f32>,
    right: Vec<f32>,
    meter_l: f32,
    meter_r: f32,
}

impl RealTime {
    fn new(
        rate: f32,
        commands: Receiver<AudioCommand>,
        garbage: Sender<Retired>,
        shared: Arc<Shared>,
    ) -> Self {
        let mut renderer = Renderer::new(rate);
        renderer.swap_prepared_fonts(FontBank::prepare(
            renderer.sample_rate(),
            Arc::new(FontLibrary::empty()),
            [],
        ));
        Self {
            renderer,
            commands,
            garbage,
            pending_retirement: None,
            shared,
            left: vec![0.0; MAX_BLOCK],
            right: vec![0.0; MAX_BLOCK],
            meter_l: 0.0,
            meter_r: 0.0,
        }
    }

    fn apply_command(&mut self, cmd: AudioCommand) -> Retired {
        let fonts = cmd
            .fonts
            .map(|bank| self.renderer.swap_prepared_fonts(bank));
        let preview = cmd
            .preview
            .and_then(|synth| self.renderer.swap_prepared_preview(synth));
        let song = cmd.render.and_then(|cmd| self.renderer.apply(cmd));
        Retired {
            song,
            fonts,
            preview,
        }
    }

    fn retire(&mut self, old: Retired) -> bool {
        if let Err(TrySendError::Full(old) | TrySendError::Disconnected(old)) =
            self.garbage.try_send(old)
        {
            self.pending_retirement = Some(old);
            return false;
        }
        true
    }

    fn drain_commands(&mut self) {
        if let Some(old) = self.pending_retirement.take() {
            if !self.retire(old) {
                return;
            }
        }
        for _ in 0..MAX_COMMANDS_PER_BLOCK {
            let Ok(cmd) = self.commands.try_recv() else {
                break;
            };
            let old = self.apply_command(cmd);
            if !old.is_empty() && !self.retire(old) {
                break;
            }
        }
    }

    fn fill<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T], channels: usize) {
        self.fill_with_metering::<T, true>(data, channels);
    }

    fn fill_with_metering<T: SizedSample + FromSample<f32>, const METER: bool>(
        &mut self,
        data: &mut [T],
        channels: usize,
    ) {
        self.drain_commands();
        let channels = channels.max(1);
        let frames = data.len() / channels;
        let mut done = 0;
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        let mut peaks = BlockPeaks::default();
        while done < frames {
            let n = (frames - done).min(MAX_BLOCK);
            self.renderer.render_with_metering::<METER>(
                &mut self.left[..n],
                &mut self.right[..n],
                &mut peaks,
            );
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
        if METER {
            s.mixer.publish(self.renderer.tracks(), &peaks);
        }
    }
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: Arc<Mutex<RealTime>>,
    control: Sender<ThreadMsg>,
    generation: u64,
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
            let _ = control.try_send(ThreadMsg::StreamFailed {
                generation,
                error: err,
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
    preparation: &Mutex<Preparation>,
    control: &Sender<ThreadMsg>,
    generation: u64,
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
        let mut prep = preparation.lock().unwrap_or_else(|p| p.into_inner());
        let mut rt = state.lock().unwrap_or_else(|p| p.into_inner());
        if (prep.rate - sample_rate as f32).abs() >= 0.5 {
            // No callback is running. Apply commands prepared at the old
            // rate before changing it; new producers then see the new rate.
            drop(rt.pending_retirement.take());
            for _ in 0..COMMAND_CAPACITY {
                let Ok(cmd) = rt.commands.try_recv() else {
                    break;
                };
                drop(rt.apply_command(cmd));
            }
            rt.renderer.set_sample_rate(sample_rate as f32);
            prep.rate = rt.renderer.sample_rate();
            prep.preview = None;
        }
    }
    let st = Arc::clone(state);
    let ctl = control.clone();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, st, ctl, generation),
        SampleFormat::F64 => build::<f64>(&device, &config, st, ctl, generation),
        SampleFormat::I16 => build::<i16>(&device, &config, st, ctl, generation),
        SampleFormat::I32 => build::<i32>(&device, &config, st, ctl, generation),
        SampleFormat::I8 => build::<i8>(&device, &config, st, ctl, generation),
        SampleFormat::U16 => build::<u16>(&device, &config, st, ctl, generation),
        SampleFormat::U32 => build::<u32>(&device, &config, st, ctl, generation),
        SampleFormat::U8 => build::<u8>(&device, &config, st, ctl, generation),
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

fn fatal_stream_error(error: &StreamError) -> bool {
    // CPAL 0.16's WASAPI output worker exits after reporting any stream
    // error, including BackendSpecific; keeping that stream cannot recover.
    cfg!(windows) || matches!(error, StreamError::DeviceNotAvailable)
}

fn audio_thread(
    state: Arc<Mutex<RealTime>>,
    shared: Arc<Shared>,
    preparation: Arc<Mutex<Preparation>>,
    garbage: Receiver<Retired>,
    control_rx: Receiver<ThreadMsg>,
    control_tx: Sender<ThreadMsg>,
    ready: Sender<Result<DeviceInfo, String>>,
) {
    let mut generation = 0;
    let mut stream = match open_stream(&state, &preparation, &control_tx, generation) {
        Ok((stream, info)) => {
            shared
                .sample_rate
                .store(info.sample_rate, Ordering::Relaxed);
            shared.connected.store(true, Ordering::Release);
            let _ = ready.send(Ok(info));
            Some(stream)
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut next_retry = Instant::now();
    while !shared.shutdown.load(Ordering::Acquire) {
        // The sender also drains on edits. Drain here when no UI commands
        // arrive so retired resources don't linger indefinitely.
        garbage.try_iter().for_each(drop);
        let wait = if stream.is_some() {
            RETRY_INTERVAL
        } else {
            next_retry.saturating_duration_since(Instant::now())
        };
        match control_rx.recv_timeout(wait) {
            Ok(ThreadMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(ThreadMsg::StreamFailed {
                generation: failed,
                error,
            }) if failed == generation && stream.is_some() => {
                if fatal_stream_error(&error) {
                    shared.connected.store(false, Ordering::Release);
                    drop(stream.take());
                    let mut rt = state.lock().unwrap_or_else(|p| p.into_inner());
                    rt.renderer.device_lost();
                    rt.meter_l = 0.0;
                    rt.meter_r = 0.0;
                    shared.playing.store(false, Ordering::Relaxed);
                    shared.peak_l.store(0, Ordering::Relaxed);
                    shared.peak_r.store(0, Ordering::Relaxed);
                    shared.voices.store(0, Ordering::Relaxed);
                    shared.mixer.publish(&[], &BlockPeaks::default());
                    next_retry = Instant::now();
                }
                shared.set_error(Some(format!("Audio device problem: {error}")));
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
        }
        if stream.is_none()
            && !shared.shutdown.load(Ordering::Acquire)
            && Instant::now() >= next_retry
        {
            generation += 1;
            if let Ok((s, info)) = open_stream(&state, &preparation, &control_tx, generation) {
                stream = Some(s);
                shared
                    .sample_rate
                    .store(info.sample_rate, Ordering::Relaxed);
                shared.connected.store(true, Ordering::Release);
                shared.set_error(Some(format!(
                    "Audio reconnected to \"{}\" ({} Hz).",
                    info.name, info.sample_rate
                )));
            }
            next_retry = Instant::now() + RETRY_INTERVAL;
        }
    }
    drop(stream);
}

/// Owns the native audio output and the real-time renderer.
pub struct AudioEngine {
    commands: Sender<AudioCommand>,
    garbage: Receiver<Retired>,
    preparation: Arc<Mutex<Preparation>>,
    shared: Arc<Shared>,
    control: Sender<ThreadMsg>,
    thread: Option<JoinHandle<()>>,
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
        let initial_rate = 48_000.0;
        let state = Arc::new(Mutex::new(RealTime::new(
            initial_rate,
            command_rx,
            garbage_tx,
            Arc::clone(&shared),
        )));
        let preparation = Arc::new(Mutex::new(Preparation::new(initial_rate)));
        let thread_shared = Arc::clone(&shared);
        let thread_control = control.clone();
        let thread_prep = Arc::clone(&preparation);
        let thread_garbage = garbage.clone();
        let thread = std::thread::Builder::new()
            .name("cadence-audio".into())
            .spawn(move || {
                audio_thread(
                    state,
                    thread_shared,
                    thread_prep,
                    thread_garbage,
                    control_rx,
                    thread_control,
                    ready_tx,
                )
            })
            .map_err(|e| format!("Could not start the audio thread: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(info)) => Ok(AudioEngine {
                commands,
                garbage,
                preparation,
                shared,
                control,
                thread: Some(thread),
                device_name: info.name,
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                shared.shutdown.store(true, Ordering::Release);
                let _ = control.try_send(ThreadMsg::Shutdown);
                Err("The audio device did not respond.".to_string())
            }
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Ordering::Relaxed)
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    pub(crate) fn wake_on_device_change<F: Fn() + Send + Sync + 'static>(
        &self,
        build_wakeup: impl FnOnce() -> F,
    ) {
        self.shared
            .device_wakeup
            .get_or_init(|| Box::new(build_wakeup()));
    }

    fn send(&self, cmd: impl Into<AudioCommand>) -> Result<(), String> {
        while self.garbage.try_recv().is_ok() {}
        self.commands.try_send(cmd.into()).map_err(|e| match e {
            TrySendError::Full(_) => "The audio engine is busy; try again.".to_string(),
            TrySendError::Disconnected(_) => "The audio engine has stopped.".to_string(),
        })
    }

    fn send_song(
        &self,
        song: Box<SongData>,
        make_command: impl FnOnce(Box<SongData>) -> Command,
    ) -> Result<(), String> {
        let mut prep = self.preparation.lock().unwrap_or_else(|p| p.into_inner());
        let mut bindings: Vec<_> = FontBank::track_bindings(&song.tracks).collect();
        bindings.sort_unstable();
        let changed = prep.bindings != bindings;
        let fonts = changed.then(|| {
            FontBank::prepare(
                prep.rate,
                Arc::clone(&prep.library),
                bindings.iter().copied(),
            )
        });
        self.send(AudioCommand {
            render: Some(make_command(song)),
            fonts,
            preview: None,
        })?;
        if changed {
            prep.bindings = bindings;
        }
        Ok(())
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
        if !self.is_connected() {
            return Err("The audio output is disconnected; waiting for a device.".into());
        }
        let song = Box::new(SongData::from_project(project));
        self.send_song(song, |song| Command::Play {
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
        self.try_set_project(project);
    }

    pub(crate) fn try_set_project(&self, project: &Project) -> bool {
        !self.commands.is_full()
            && self
                .send_song(Box::new(SongData::from_project(project)), Command::Load)
                .is_ok()
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
        let render = Command::Preview {
            params,
            pitch,
            velocity,
            seconds: duration_seconds,
        };
        if let Sound::Font(key) = params.sound {
            let mut prep = self.preparation.lock().unwrap_or_else(|p| p.into_inner());
            let key = key.font_key;
            if prep.preview != Some(key) {
                let preview = FontBank::prepare_synth(&prep.library, prep.rate, key);
                if self
                    .send(AudioCommand {
                        render: Some(render),
                        fonts: None,
                        preview,
                    })
                    .is_ok()
                {
                    prep.preview = Some(key);
                }
                return;
            }
        }
        let _ = self.send(render);
    }

    pub fn set_metronome(&self, enabled: bool) {
        let _ = self.send(Command::Metronome(enabled));
    }

    /// Installs the SoundFont catalog for tracks with `Track::font` set.
    /// Fonts are parsed on the caller's thread ([`crate::font::load_library`])
    /// and pushed to the audio callback — typically once at startup and again
    /// when the font folder changes. An empty catalog makes font tracks fall
    /// back to their preset/built-in sound.
    pub fn set_fonts(&self, library: Arc<FontLibrary>) {
        let mut prep = self.preparation.lock().unwrap_or_else(|p| p.into_inner());
        let bank = FontBank::prepare(
            prep.rate,
            Arc::clone(&library),
            prep.bindings.iter().copied(),
        );
        if self
            .send(AudioCommand {
                fonts: Some(bank),
                ..AudioCommand::default()
            })
            .is_ok()
        {
            prep.library = library;
            prep.preview = None;
        }
    }

    /// Lock-free read of the last callback's per-track and subgroup peaks.
    /// A concurrent update can return `None`; consumers may reuse their
    /// previous snapshot for that frame. Map tracks by ID, not by position.
    pub fn mixer_peaks(&self) -> Option<MixerPeaks> {
        self.shared.mixer.read()
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
        self.shared.shutdown.store(true, Ordering::Release);
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
        let mut rt = RealTime::new(44_100.0, rx, gtx, Arc::clone(&shared));
        let demo = Project::demo();
        tx.send(
            Command::Play {
                song: Box::new(SongData::from_project(&demo)),
                start_step: 0.0,
                looping: true,
                loop_region: None,
            }
            .into(),
        )
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
        tx.send(Command::Stop.into()).unwrap();
        let mut out = vec![0.0f32; 2 * 4096];
        rt.fill(&mut out, 2);
        rt.fill(&mut out, 2);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(shared.voices.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn full_retirement_queue_defers_commands_without_dropping_state() {
        let (tx, rx) = bounded(8);
        let (gtx, grx) = bounded(1);
        let mut rt = RealTime::new(48_000.0, rx, gtx, Arc::default());
        for _ in 0..3 {
            tx.send(Command::Load(Box::new(SongData::empty())).into())
                .unwrap();
        }
        rt.fill(&mut [0.0f32; 256], 2);
        assert!(rt.pending_retirement.is_some());
        assert_eq!(rt.commands.len(), 1);
        rt.fill(&mut [0.0f32; 256], 2);
        assert!(rt.pending_retirement.is_some());
        assert_eq!(rt.commands.len(), 1);
        assert!(grx.try_recv().unwrap().song.is_some());
        rt.fill(&mut [0.0f32; 256], 2);
        assert!(grx.try_recv().unwrap().song.is_some());
        rt.fill(&mut [0.0f32; 256], 2);
        assert!(grx.try_recv().unwrap().song.is_some());
        assert!(rt.pending_retirement.is_none());
        assert!(rt.commands.is_empty());
    }

    #[test]
    fn mixer_taps_follow_track_ids_and_bus_routing() {
        use crate::model::{create_track, Instrument, Note};

        let (tx, rx) = bounded(8);
        let (gtx, _grx) = bounded(8);
        let shared = Arc::<Shared>::default();
        let mut rt = RealTime::new(48_000.0, rx, gtx, Arc::clone(&shared));
        let mut project = Project::empty();
        project.tracks[0].id = 0;
        project.tracks[0].bus = 2;
        project.tracks[0].patterns[0].notes = vec![Note::new(36, 0.0, 4.0, 1.0)];
        project.tracks.push(create_track(Instrument::Piano));
        let silent = project.tracks[1].id;
        tx.send(
            Command::Play {
                song: Box::new(SongData::from_project(&project)),
                start_step: 0.0,
                looping: false,
                loop_region: None,
            }
            .into(),
        )
        .unwrap();
        rt.fill(&mut [0.0f32; 1024], 2);
        let first = shared.mixer.read().unwrap();
        assert!(first.track(0).unwrap() > 0.01);
        assert_eq!(first.track(silent), Some(0.0));
        assert_eq!(first.buses[0], 0.0);
        assert!(first.buses[1] > 0.01);
        project.tracks.reverse();
        tx.send(Command::Load(Box::new(SongData::from_project(&project))).into())
            .unwrap();
        rt.fill(&mut [0.0f32; 1024], 2);
        let reordered = shared.mixer.read().unwrap();
        assert_eq!(reordered.tracks[0].unwrap().track_id, silent);
        assert!(reordered.track(0).unwrap() > 0.01);
        assert_eq!(reordered.track(silent), Some(0.0));
        assert!(reordered.buses[1] > 0.01);
    }

    #[test]
    fn mixer_taps_do_not_change_audio_bits() {
        let (tx_on, rx_on) = bounded(1);
        let (tx_off, rx_off) = bounded(1);
        let (trash_on, _g1) = bounded(1);
        let (trash_off, _g2) = bounded(1);
        let mut on = RealTime::new(48_000.0, rx_on, trash_on, Arc::default());
        let mut off = RealTime::new(48_000.0, rx_off, trash_off, Arc::default());
        let project = Project::demo();
        for tx in [tx_on, tx_off] {
            tx.send(
                Command::Play {
                    song: Box::new(SongData::from_project(&project)),
                    start_step: 0.0,
                    looping: false,
                    loop_region: None,
                }
                .into(),
            )
            .unwrap();
        }
        let mut a = [0.0f32; 1024];
        let mut b = a;
        for _ in 0..32 {
            on.fill(&mut a, 2);
            off.fill_with_metering::<f32, false>(&mut b, 2);
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens and reopens a real WASAPI device; no audible notes"]
    fn real_device_recovers_backend_failure() {
        let engine = AudioEngine::new().unwrap();
        let project = Project::empty();
        engine
            .control
            .try_send(ThreadMsg::StreamFailed {
                generation: 0,
                error: StreamError::BackendSpecific {
                    err: cpal::BackendSpecificError {
                        description: "injected WASAPI stream failure".into(),
                    },
                },
            })
            .unwrap();
        let deadline = Instant::now() + 5 * RETRY_INTERVAL;
        while !engine
            .status()
            .error
            .is_some_and(|message| message.starts_with("Audio reconnected"))
        {
            assert!(Instant::now() < deadline, "WASAPI stream did not reopen");
            std::thread::sleep(Duration::from_millis(10));
        }
        engine.play(&project, 0.0, false, None).unwrap();
        // Simulate a UI thread held up by snapshot/SoundFont preparation.
        // The real device must keep asking the callback to advance transport.
        let _ui_busy = engine.preparation.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while engine.status().step < 1.0 {
            assert!(Instant::now() < deadline, "reopened callback did not run");
            std::thread::sleep(Duration::from_millis(10));
        }
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
