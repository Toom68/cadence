//! Audio: native real-time playback ([`AudioEngine`]) and offline rendering
//! ([`export_wav`], [`render_project`]).
//!
//! Both paths drive the same `Renderer`, so an export sounds exactly like playback:
//!
//! - six built-in synthesized instruments (eight-voice drum kit, FM electric piano, analog
//!   bass, Karplus-Strong pluck, detuned pad, monophonic gliding lead) plus a generic
//!   oscillator/ADSR/bit-crush voice for data-driven pack presets (`Track::preset`);
//! - sample-accurate scheduling with swing, chased notes on seek, and a bounded voice pool
//!   ([`MAX_VOICES`]) with release-aware stealing;
//! - per-track drive, low-pass filter, fader and pan; shared reverb and tempo-synced
//!   ping-pong delay sends; per-track three-band EQ; master gain, DC blocker, an optional
//!   glue compressor and a look-ahead limiter at -1 dBFS;
//! - optional BS.1770 loudness normalization of WAV exports ([`export_wav_with`]);
//! - sampled General MIDI instruments from user-installed SoundFonts (`Track::font`),
//!   rendered by per-track `rustysynth` instances into the same strips as voices.

mod dsp;
mod engine;
mod export;
mod font;
mod loudness;
mod meter;
mod renderer;
mod song;
mod voice;

pub use engine::{AudioEngine, TransportStatus};
pub use export::{
    export_stems, export_wav, export_wav_with, render_project, stem_skip_reason, RenderedAudio,
    WavExportOptions, WavExportReport, EXPORT_SAMPLE_RATE, MAX_TAIL_SECONDS, MAX_TARGET_LUFS,
    MIN_TARGET_LUFS,
};
pub use loudness::{LoudnessMeter, DEFAULT_TARGET_LUFS};
pub use meter::{MixerPeaks, TrackPeak};
pub use renderer::MAX_VOICES;
