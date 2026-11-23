//! Cadence: a native Rust music sequencer.
//!
//! Modules:
//! - [`model`]: song data model (projects, tracks, patterns, notes).
//! - [`audio`]: real-time engine and offline WAV rendering.
//! - [`midi`]: Standard MIDI File import/export.
//! - [`midi_in`]: hardware MIDI controller input (port scanning, parsing,
//!   sustain-pedal note tracking).
//! - [`persistence`]: safe project save/load and crash recovery.
//! - [`packs`]: data-driven JSON instrument packs (validation, install, catalog).
//! - [`font`]: SoundFont (`.sf2`) instruments — user font directory, GM program
//!   names and the font catalog shared with the audio engine.
//! - [`gui`]: the eframe/egui desktop interface.
//! - [`update`]: signed in-app updates (release manifest, verification,
//!   installer hand-off).

pub mod audio;
pub mod font;
pub mod gui;
pub mod midi;
pub mod midi_in;
pub mod model;
pub mod packs;
pub mod persistence;
pub mod update;

pub use gui::CadenceApp;

/// Human-readable application name.
pub const APP_NAME: &str = "Cadence";

/// Application version from the crate manifest.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

static LAUNCH_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Records the project file passed on the command line (set once by `main`).
pub fn set_launch_path(path: std::path::PathBuf) {
    let _ = LAUNCH_PATH.set(path);
}

/// Project file the app was launched with (e.g. `cadence song.cadence`), if any.
/// The GUI opens it at startup instead of the demo/recovery flow.
pub fn launch_path() -> Option<&'static std::path::Path> {
    LAUNCH_PATH.get().map(|path| path.as_path())
}
