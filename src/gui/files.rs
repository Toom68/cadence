//! Document and file workflows: open/save/save-as, the unsaved-changes guard,
//! MIDI import/export, WAV sample import, the WAV export worker and
//! instrument-pack install.
//!
//! Everything here is reachable from the menu/shortcuts in `mod.rs` and the
//! library footer in `sidebar.rs`. Native dialogs (rfd) and recovery I/O only
//! run when `io_enabled` is set; the explicit path helpers stay usable in
//! headless tests.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use super::dialogs::{Dialog, NewSongDraft};
use super::ops;
use super::{CadenceApp, StatusKind};
use crate::model::{Project, SongKey, MAX_NAME_CHARS};
use crate::persistence;

/// The open song file plus the history state it was last saved at.
///
/// `saved_state` mirrors `History::state()` after a successful save or load;
/// `dirty()` in `mod.rs` compares it to the live state, so undoing back to the
/// saved point correctly reads as clean.
#[derive(Debug)]
pub(super) struct Document {
    pub(super) path: Option<PathBuf>,
    pub(super) saved_state: Option<u64>,
}

/// A file action that may need an unsaved-changes decision first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PendingAction {
    /// New empty song.
    New,
    /// New song from the built-in demo.
    Demo,
    /// Open a `.cadence` file.
    Open,
    /// Import a Standard MIDI File as a new song.
    ImportMidi,
    /// Quit the app.
    Quit,
}

/// What a running export produces; drives the status-bar and error wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExportKind {
    Wav,
    Stems,
}

impl ExportKind {
    /// "Exporting WAV" / "Exporting stems" for the status bar and File menu.
    fn verb(self) -> &'static str {
        match self {
            ExportKind::Wav => "Exporting WAV",
            ExportKind::Stems => "Exporting stems",
        }
    }

    /// "WAV export" / "Stem export" — fits "{noun} failed: …".
    fn noun(self) -> &'static str {
        match self {
            ExportKind::Wav => "WAV export",
            ExportKind::Stems => "Stem export",
        }
    }
}

/// A WAV or stem export running on a worker thread.
///
/// The [`Project`] was cloned when the export started, so edits made while it
/// renders do not change the output. Progress arrives through a shared atomic
/// and the finished status line through a channel; [`CadenceApp::poll_export`]
/// picks it up once per frame.
pub(super) struct ExportJob {
    rx: crossbeam_channel::Receiver<Result<String, String>>,
    progress: Arc<AtomicU32>,
    kind: ExportKind,
}

impl ExportJob {
    /// Render progress reported by the worker, 0.0..=1.0.
    pub(super) fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }

    /// "Exporting WAV" / "Exporting stems" for the status bar and File menu.
    pub(super) fn verb(&self) -> &'static str {
        self.kind.verb()
    }
}

impl CadenceApp {
    // ----- pending actions -------------------------------------------------

    /// Entry point for actions that replace the song (or quit): ask before
    /// discarding unsaved changes, otherwise run immediately.
    pub(super) fn request(&mut self, action: PendingAction) {
        if self.dirty() {
            self.dialog = Dialog::Unsaved(action);
        } else {
            self.run_pending_action(action);
        }
    }

    /// Runs a pending action after a save/discard decision (or right away when
    /// the document is clean). Called by the unsaved-changes dialog.
    pub(super) fn run_pending_action(&mut self, action: PendingAction) {
        match action {
            // The unsaved-changes decision (if any) has already been made;
            // now ask for the new song's name and key. Cancelling that dialog
            // keeps the current song.
            PendingAction::New => self.dialog = Dialog::NewSong(NewSongDraft::default()),
            PendingAction::Demo => {
                self.replace_project(Project::demo(), None, true);
                self.set_status(
                    StatusKind::Info,
                    "Loaded the demo song. Use Save As… to keep your own version.",
                );
            }
            PendingAction::Open => self.open_dialog(),
            PendingAction::ImportMidi => self.import_midi_dialog(),
            PendingAction::Quit => self.allow_close = true,
        }
    }

    /// Replaces the song with a blank one named `name` (trimmed; blank falls
    /// back to the default name) in `key`. Used by the New Song dialog.
    pub(super) fn create_new_song(&mut self, name: &str, key: SongKey) {
        let mut project = Project::empty();
        let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
        let name = name.trim_end();
        if !name.is_empty() {
            project.name = name.to_string();
        }
        project.key = SongKey::new(key.root, key.mode);
        self.replace_project(project, None, true);
        self.set_status(
            StatusKind::Success,
            format!("Started a new song in {}.", ops::key_label(key)),
        );
    }

    // ----- open / save -----------------------------------------------------

    /// Loads `path` as the current song. Used for the launch file and the
    /// open picker; on error the current project is kept and the problem is
    /// shown in the status bar.
    pub(super) fn open_path(&mut self, path: PathBuf) {
        match persistence::load_project(&path) {
            Ok(project) => {
                let shown = file_label(&path);
                self.replace_project(project, Some(path), true);
                self.set_status(StatusKind::Success, format!("Opened {shown}."));
                self.warn_missing_refs();
            }
            Err(err) => self.set_status(StatusKind::Error, err),
        }
    }

    /// After opening a song: warn about content the file references that
    /// this machine doesn't have — SoundFonts (tracks fall back to
    /// built-in/preset sounds) and imported audio (clips stay silent, in
    /// the arranger, on export and in renders). Both are silent playback
    /// tolerances, so the status line is the one place that says why the
    /// song sounds different.
    fn warn_missing_refs(&mut self) {
        let mut parts = Vec::new();
        let mut missing: Vec<String> = self
            .project
            .tracks
            .iter()
            .filter_map(|t| t.font.as_ref().map(|f| f.name.clone()))
            .filter(|name| !self.fonts.fonts.iter().any(|i| &i.name == name))
            .collect();
        if !missing.is_empty() {
            missing.sort();
            missing.dedup();
            let tracks = self
                .project
                .tracks
                .iter()
                .filter(|t| t.font.as_ref().is_some_and(|f| missing.contains(&f.name)))
                .count();
            let names = missing
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!(
                "SoundFont{} {names} not installed here — {} track{} play{} {} fallback sound{}",
                super::sidebar::plural(missing.len()),
                tracks,
                super::sidebar::plural(tracks),
                if tracks == 1 { "s" } else { "" },
                if tracks == 1 { "its" } else { "their" },
                super::sidebar::plural(tracks),
            ));
        }
        if !self.missing_audio.is_empty() {
            let n = self.missing_audio.len();
            parts.push(format!(
                "{} audio clip file{} missing from this PC's audio store — \
                 those clips are silent; re-import the original WAVs",
                n,
                super::sidebar::plural(n),
            ));
        }
        if !parts.is_empty() {
            self.set_status(StatusKind::Warning, format!("{}.", parts.join(". ")));
        }
    }

    /// Open…: pick a `.cadence` file. Cancelling changes nothing.
    fn open_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Cadence song", &[persistence::PROJECT_EXTENSION])
            .pick_file()
        else {
            return;
        };
        self.open_path(path);
    }

    /// Save: write to the current file, or ask where when the song is untitled.
    pub(super) fn save(&mut self) {
        self.save_current();
    }

    /// Attempts to save the song. Returns true only when the project was
    /// written to disk; false on picker cancel or write failure (the status
    /// bar explains which, and the dirty flag/pending action are preserved).
    pub(super) fn save_current(&mut self) -> bool {
        match self.doc.path.clone() {
            Some(path) => self.save_to(path),
            None => self.save_as(),
        }
    }

    /// Save As…: pick a `.cadence` path and write there. Returns false when the
    /// picker is cancelled or the write fails.
    pub(super) fn save_as(&mut self) -> bool {
        if !self.dialogs_available() {
            return false;
        }
        let name = file_stem(&self.project.name);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Cadence song", &[persistence::PROJECT_EXTENSION])
            .set_file_name(format!("{name}.{}", persistence::PROJECT_EXTENSION))
            .save_file()
        else {
            return false;
        };
        self.save_to(path)
    }

    /// Writes the project atomically and marks the document clean. On success
    /// `saved_state` tracks the current history state and the recovery
    /// autosave is cleared; on failure nothing changes.
    fn save_to(&mut self, path: PathBuf) -> bool {
        let path = persistence::with_project_extension(&path);
        match persistence::save_project(&self.project, &path) {
            Ok(()) => {
                self.doc.path = Some(path.clone());
                self.doc.saved_state = Some(self.history.state());
                // An autosave still awaiting its Restore/Discard decision is
                // another session's work: saving this song must not delete it.
                if let (Some(dir), false) = (&self.recovery_dir, self.recovery_offered) {
                    let _ = persistence::clear_recovery_in(dir);
                }
                // The song is safe on disk: a failing autosave streak ends
                // here (the next edit tries again and warns afresh).
                self.autosave_failures = 0;
                self.autosave_retry_at = None;
                self.autosave_error = None;
                // The file name is enough here (the toast repeats it); the
                // full path is in the transport's saved-state tooltip.
                self.set_status(StatusKind::Success, format!("Saved {}.", file_label(&path)));
                true
            }
            Err(err) => {
                self.set_status(StatusKind::Error, err);
                false
            }
        }
    }

    // ----- recovery --------------------------------------------------------

    /// On launch, offer the crash-recovery autosave when a valid one exists —
    /// also when Cadence was started with a song file (the offer then opens
    /// on top of that song). A damaged autosave is never loaded: persistence
    /// moves it aside and the reason is shown as a status warning instead.
    pub(super) fn check_recovery(&mut self) {
        let Some(dir) = self.recovery_dir.clone() else {
            return;
        };
        if self.recovery_offered {
            return;
        }
        match persistence::load_recovery_in(&dir) {
            Ok(Some(work)) => {
                self.recovery_offered = true;
                self.dialog = Dialog::Recovery(Box::new(work));
            }
            Ok(None) => {}
            Err(err) => self.set_status(StatusKind::Warning, err),
        }
    }

    // ----- WAV export ------------------------------------------------------

    /// Export WAV…: pick a `.wav` target, then render on a worker thread so the
    /// interface stays responsive. The render uses a snapshot of the project.
    pub(super) fn export_wav_dialog(&mut self) {
        if self.export.is_some() {
            self.set_status(
                StatusKind::Info,
                "An export is already running — watch the progress in the status bar.",
            );
            return;
        }
        if !self.dialogs_available() {
            return;
        }
        let name = file_stem(&self.project.name);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name(format!("{name}.wav"))
            .save_file()
        else {
            return;
        };
        self.start_wav_export(path);
    }

    /// Spawns the offline render on a worker thread. The `.wav` extension is
    /// enforced here (`Path::with_extension` also turns a picked
    /// `song.cadence` into `song.wav`, so a song file is never overwritten).
    /// Progress flows through a shared atomic and the final result through a
    /// channel polled by [`CadenceApp::poll_export`].
    pub(super) fn start_wav_export(&mut self, path: PathBuf) {
        if self.export.is_some() {
            self.set_status(StatusKind::Info, "An export is already running.");
            return;
        }
        let path = path.with_extension("wav");
        let project = self.project.clone();
        let render_path = path.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let report = progress.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = crate::audio::export_wav(&project, &render_path, move |p| {
                report.store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            })
            .map(|()| format!("Exported WAV: {}", render_path.display()));
            let _ = tx.send(result);
        });
        let shown = path.display().to_string();
        self.export = Some(ExportJob {
            rx,
            progress,
            kind: ExportKind::Wav,
        });
        self.set_status(StatusKind::Info, format!("Exporting WAV to {shown}…"));
    }

    /// Export Stems…: pick a folder, then render every audible track to its
    /// own WAV on a worker thread — same progress plumbing as the WAV export.
    pub(super) fn export_stems_dialog(&mut self) {
        if self.export.is_some() {
            self.set_status(
                StatusKind::Info,
                "An export is already running — watch the progress in the status bar.",
            );
            return;
        }
        if !self.dialogs_available() {
            return;
        }
        let Some(dir) = rfd::FileDialog::new()
            .set_title(format!(
                "Export stems of \"{}\" — choose a folder",
                self.project.name
            ))
            .pick_folder()
        else {
            return;
        };
        self.start_stem_export(dir);
    }

    /// Spawns the per-track renders on a worker thread. Stems land in `dir` as
    /// `<track name>-NN.wav`; tracks that are muted or silenced by solo are
    /// skipped and named in the finished status line.
    pub(super) fn start_stem_export(&mut self, dir: PathBuf) {
        if self.export.is_some() {
            self.set_status(StatusKind::Info, "An export is already running.");
            return;
        }
        let project = self.project.clone();
        let fonts = self.fonts.clone();
        let out_dir = dir.clone();
        let progress = Arc::new(AtomicU32::new(0));
        let report = progress.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            let result = crate::audio::export_stems(
                &project,
                &fonts,
                &out_dir,
                crate::audio::EXPORT_SAMPLE_RATE,
                move |p| {
                    report.store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                },
            )
            .map(|written| {
                let skipped: Vec<String> = (0..project.tracks.len())
                    .filter_map(|i| {
                        crate::audio::stem_skip_reason(&project, i)
                            .map(|reason| format!("\"{}\" {reason}", project.tracks[i].name))
                    })
                    .collect();
                let mut message = format!(
                    "Exported {} stem{} to {}.",
                    written.len(),
                    if written.len() == 1 { "" } else { "s" },
                    out_dir.display()
                );
                if !skipped.is_empty() {
                    message.push_str(&format!(" Skipped {}.", skipped.join(", ")));
                }
                message
            });
            let _ = tx.send(result);
        });
        self.export = Some(ExportJob {
            rx,
            progress,
            kind: ExportKind::Stems,
        });
        self.set_status(
            StatusKind::Info,
            format!("Exporting stems to {}…", dir.display()),
        );
    }

    /// Picks up a finished export (called once per frame). Keeps the GUI
    /// responsive while the worker renders; the status bar shows live progress.
    pub(super) fn poll_export(&mut self) {
        let finished = match &self.export {
            Some(job) => match job.rx.try_recv() {
                Ok(result) => Some(result),
                Err(crossbeam_channel::TryRecvError::Empty) => None,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    Some(Err("The export worker stopped unexpectedly.".to_owned()))
                }
            },
            None => return,
        };
        let Some(result) = finished else { return };
        let kind = self
            .export
            .take()
            .map(|job| job.kind)
            .unwrap_or(ExportKind::Wav);
        match result {
            Ok(message) => self.set_status(StatusKind::Success, message),
            Err(err) => {
                self.set_status(StatusKind::Error, format!("{} failed: {err}", kind.noun()))
            }
        }
    }

    // ----- MIDI exchange ---------------------------------------------------

    /// Export MIDI…: encode the project and write a `.mid` file.
    pub(super) fn export_midi_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let name = file_stem(&self.project.name);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("MIDI file", &["mid", "midi"])
            .set_file_name(format!("{name}.mid"))
            .save_file()
        else {
            return;
        };
        self.export_midi_file(path);
    }

    /// Encodes and writes the MIDI export (`.mid` extension enforced, so a
    /// picked `song.cadence` becomes `song.mid` and is never overwritten).
    /// Helper path usable without dialogs.
    pub(super) fn export_midi_file(&mut self, path: PathBuf) {
        let path = path.with_extension("mid");
        let bytes = match crate::midi::export_midi(&self.project) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.set_status(StatusKind::Error, format!("Could not export MIDI: {err}"));
                return;
            }
        };
        match persistence::write_file_atomic(&path, &bytes) {
            Ok(()) => self.set_status(
                StatusKind::Success,
                format!("Exported MIDI: {}", path.display()),
            ),
            Err(err) => self.set_status(StatusKind::Error, err),
        }
    }

    /// Import MIDI…: pick a `.mid`/`.midi` file and replace the song with it.
    /// Reached through [`PendingAction::ImportMidi`], so unsaved changes are
    /// confirmed first.
    pub(super) fn import_midi_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("MIDI file", &["mid", "midi"])
            .pick_file()
        else {
            return;
        };
        self.import_midi_file(path);
    }

    /// Reads and imports a Standard MIDI File as a new untitled project.
    /// Oversize and malformed files are rejected with a status message; on
    /// success the song is dirty so it is never lost silently.
    pub(super) fn import_midi_file(&mut self, path: PathBuf) {
        let result = read_file_capped(&path, crate::midi::MAX_MIDI_BYTES as u64)
            .and_then(|bytes| crate::midi::import_midi(&bytes));
        match result {
            Ok(project) => {
                let name = project.name.clone();
                self.replace_project(project, None, false);
                self.set_status(
                    StatusKind::Success,
                    format!("Imported “{name}” from MIDI. Use Save As… to keep it."),
                );
            }
            Err(err) => self.set_status(StatusKind::Error, format!("Could not import MIDI: {err}")),
        }
    }

    // ----- samples ---------------------------------------------------------

    /// Import Sample…: pick a `.wav` file and turn it into a custom sample
    /// instrument. The clip is embedded in the track's preset JSON, so the
    /// song stays portable without the source file.
    pub(super) fn import_sample_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .pick_file()
        else {
            return;
        };
        self.import_sample_file(path);
    }

    /// Decodes `path` into an embedded-sample preset and adds it as a NEW
    /// track — importing never overwrites the selected track's instrument
    /// (replacing a sound is what the track's sound picker is for). Helper
    /// path usable without dialogs. Anything unreadable, oversize or invalid
    /// is a status-bar error, not a partial edit; the placement itself is
    /// one undoable `self.edit`.
    pub(super) fn import_sample_file(&mut self, path: PathBuf) {
        let (spec, truncated) = match crate::packs::SampleSpec::from_wav(&path, 60) {
            Ok(result) => result,
            Err(err) => {
                self.set_status(StatusKind::Error, format!("Could not import sample: {err}"));
                return;
            }
        };
        let preset = sample_instrument(&sample_name(&path), spec);
        if let Err(err) = crate::packs::validate_preset(&preset) {
            self.set_status(StatusKind::Error, format!("Could not import sample: {err}"));
            return;
        }
        let label = preset.name.clone();
        let rate = preset.sample.as_ref().map_or(0, |s| s.sample_rate);
        let audition = preset.audition_note.min(127);
        let mut placed = None;
        self.edit("Import sample", None, |p| {
            placed = ops::add_preset_track(p, &preset);
        });
        let Some(id) = placed else {
            self.set_status(
                StatusKind::Warning,
                "Could not import the sample: the song is at the track limit.",
            );
            return;
        };
        self.preview(id, audition, 0.9);
        let clipped = if truncated {
            " — truncated to fit the 2 MB clip cap"
        } else {
            ""
        };
        self.set_status(
            StatusKind::Success,
            format!("Imported “{label}” ({rate} Hz){clipped}. The clip is embedded in the song."),
        );
    }

    // ----- audio clips (imported WAVs on the timeline) ---------------------

    /// Import Audio File…: pick a `.wav` and place it as an arranger clip.
    /// The file is copied into the app's audio store under a content-hash
    /// name; the song records only that name plus the clip's timeline
    /// placement, so projects stay compact — but a song only plays its
    /// audio on machines that have the same store contents (the store is
    /// per-install, not embedded in `.cadence`).
    pub(super) fn import_audio_clip_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .pick_file()
        else {
            return;
        };
        // File-menu import lands at the playhead on the selected audio lane
        // (or a fresh one — see `import_audio_clip_file`).
        self.import_audio_clip_file(path, None, None);
    }

    /// Lane context-menu entry: same pick, but the clip lands at the lane's
    /// right-clicked step on that track.
    pub(super) fn import_audio_clip_dialog_at(&mut self, track_id: u64, step: f64) {
        if !self.dialogs_available() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .pick_file()
        else {
            return;
        };
        self.import_audio_clip_file(path, Some(track_id), Some(step));
    }

    /// Stages `path` into the audio store and places a clip for it — helper
    /// path usable without dialogs (tests call this directly). `track_id`
    /// picks the lane: the given one when it is an audio lane, else the
    /// first audio lane, else a new "Audio" track. `start` defaults to the
    /// transport position. A bogus or oversize file is a status-bar error;
    /// the track/clip placement is one undoable `self.edit`.
    pub(super) fn import_audio_clip_file(
        &mut self,
        path: PathBuf,
        track_id: Option<u64>,
        start: Option<f64>,
    ) {
        let staged = match persistence::stage_audio_file(&path) {
            Ok(staged) => staged,
            Err(err) => {
                self.set_status(StatusKind::Error, format!("Could not import audio: {err}"));
                return;
            }
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| staged.file.clone());
        // The clip spans as many 16th-note steps as the source runs
        // seconds at the current tempo — no time-stretching, so a later
        // tempo change shifts the clip's grid footprint, not its playback
        // speed.
        let length = (staged.seconds / self.project.seconds_per_step())
            .max(crate::model::MIN_AUDIO_CLIP_STEPS);
        let at = start.unwrap_or_else(|| {
            if self.transport.position.is_finite() {
                self.transport.position.max(0.0)
            } else {
                0.0
            }
        });
        // Lane pick: an explicit target wins when it is an audio lane; then
        // the selection, then any audio lane, then a fresh one.
        let lane = track_id
            .or(self.sel.track)
            .filter(|id| ops::track(&self.project, *id).is_some_and(|t| t.audio))
            .or_else(|| self.project.tracks.iter().find(|t| t.audio).map(|t| t.id));
        let mut placed = None;
        let mut made_track = false;
        let file = staged.file.clone();
        self.edit("Import audio file", None, |p| {
            let tid = match lane {
                Some(id) => Some(id),
                None => {
                    made_track = true;
                    ops::add_audio_track(p)
                }
            };
            placed = tid.and_then(|id| ops::add_audio_clip(p, id, &file, &name, at, length));
        });
        let Some(clip_id) = placed else {
            self.set_status(
                StatusKind::Warning,
                if self.project.tracks.len() >= ops::MAX_TRACKS {
                    format!("Could not import “{name}”: the song is at the track limit.")
                } else {
                    format!(
                        "Could not import “{name}”: tracks hold up to {} audio clips.",
                        ops::MAX_AUDIO_CLIPS_PER_TRACK
                    )
                },
            );
            return;
        };
        let tid = self.clip_track_id(clip_id);
        self.set_single_clip_selection(clip_id);
        if let Some(id) = tid {
            self.arranger.reveal_track = Some(id);
        }
        let grown = if self
            .project
            .tracks
            .last()
            .is_some_and(|t| Some(t.id) == tid)
            && made_track
        {
            " on a new audio track"
        } else {
            ""
        };
        let store = persistence::audio_store_bytes();
        if store > persistence::AUDIO_STORE_WARN_BYTES {
            self.set_status(
                StatusKind::Warning,
                format!(
                    "Imported “{name}”{grown}. The audio store now holds {:.1} GB — \
                     old imports are never deleted, so clear it in your app-data folder if disk runs low.",
                    store as f64 / (1024.0 * 1024.0 * 1024.0)
                ),
            );
        } else {
            self.set_status(
                StatusKind::Success,
                format!(
                    "Imported “{name}”{grown} ({:.1}s). The file lives in the app's audio store.",
                    staged.seconds
                ),
            );
        }
    }

    // ----- instrument packs -------------------------------------------------

    /// Import Pack…: validate and install a JSON instrument pack, then reload
    /// the library catalog.
    pub(super) fn import_pack_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Instrument pack", &["json"])
            .pick_file()
        else {
            return;
        };
        self.install_pack_file(path);
    }

    /// Validates `path` as a pack and installs it into the user pack folder.
    /// Helper path usable without dialogs (e.g. tests).
    pub(super) fn install_pack_file(&mut self, path: PathBuf) {
        match crate::packs::install_pack(&path) {
            Ok(installed) => {
                self.reload_packs(true);
                self.set_status(
                    StatusKind::Success,
                    format!(
                        "Installed pack to {}. Its instruments are in the library now.",
                        installed.display()
                    ),
                );
            }
            Err(err) => {
                self.set_status(StatusKind::Error, format!("Could not install pack: {err}"))
            }
        }
    }

    // ----- helpers ---------------------------------------------------------

    /// Native file dialogs only exist in the desktop app; in headless mode we
    /// explain instead of silently doing nothing.
    fn dialogs_available(&mut self) -> bool {
        if self.io_enabled {
            return true;
        }
        self.set_status(
            StatusKind::Warning,
            "File dialogs are only available in the desktop app.",
        );
        false
    }
}

/// Project name made safe to use as a file name.
fn file_stem(name: &str) -> String {
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
        "untitled".to_owned()
    } else {
        cleaned.to_owned()
    }
}

/// Display name for an imported clip: the file stem, trimmed and length-capped.
fn sample_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .map(|s| s.trim().chars().take(MAX_NAME_CHARS).collect::<String>())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Sample".to_string())
}

/// A melodic preset whose only source is the imported clip: a sustain-forever
/// envelope (the clip itself is the note's shape), full-quality playback, and
/// a namespaced `sample.<name>` id. `audition_note` follows the clip's root so
/// previews play at unity speed.
fn sample_instrument(name: &str, spec: crate::packs::SampleSpec) -> crate::packs::InstrumentPreset {
    let slug = crate::packs::slugify(name);
    crate::packs::InstrumentPreset {
        id: format!(
            "sample.{}",
            if slug.is_empty() {
                "clip".to_string()
            } else {
                slug
            }
        ),
        name: name.to_string(),
        description: "Imported WAV sample.".to_string(),
        oscillators: Vec::new(),
        envelope: crate::packs::Envelope {
            attack: 0.002,
            decay: 0.0,
            sustain: 1.0,
            release: 0.08,
        },
        pitch_envelope: None,
        filter_envelope: None,
        filter_cutoff: 20_000.0,
        filter_resonance: 0.0,
        lfo: None,
        bit_depth: 24,
        sample_hold: 1,
        gain: 0.9,
        monophonic: false,
        velocity_to_cutoff: crate::packs::DEFAULT_VELOCITY_TO_CUTOFF,
        spread: 0.0,
        hidden: false,
        audition_note: spec.root_pitch.min(127),
        audition_duration: 0.8,
        kit: None,
        sample: Some(spec),
    }
}

/// A song's file name for status messages ("neon-circuits.cadence"), or the
/// whole path when it has none.
pub(super) fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Reads a file refusing anything over `max` bytes (checked before and during
/// the read so a growing file cannot slip past the cap).
fn read_file_capped(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let shown = path.display();
    let meta = std::fs::metadata(path).map_err(|err| format!("Could not open {shown}: {err}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file."));
    }
    if meta.len() > max {
        return Err(format!(
            "{shown} is too large ({:.1} MB, limit {:.1} MB).",
            meta.len() as f64 / (1024.0 * 1024.0),
            max as f64 / (1024.0 * 1024.0)
        ));
    }
    let file = File::open(path).map_err(|err| format!("Could not open {shown}: {err}"))?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("Could not read {shown}: {err}"))?;
    if bytes.len() as u64 > max {
        return Err(format!("{shown} is too large."));
    }
    Ok(bytes)
}
