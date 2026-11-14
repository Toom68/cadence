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
//!   ping-pong delay sends; master gain, DC blocker and a look-ahead limiter at -1 dBFS.

mod dsp;
mod engine;
mod export;
mod renderer;
mod song;
mod voice;

pub use engine::{AudioEngine, TransportStatus};
pub use export::{export_wav, render_project, RenderedAudio, EXPORT_SAMPLE_RATE, MAX_TAIL_SECONDS};
pub use renderer::MAX_VOICES;
