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
use super::theme;
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

/// What Clean Up Unused Audio… must keep, from
/// [`CadenceApp::referenced_audio`].
#[derive(Debug, Default)]
pub(super) struct AudioInUse {
    /// Store names something known still uses.
    pub(super) names: std::collections::BTreeSet<String>,
    /// Recent songs that could not be read (path, reason): their audio is
    /// unknown, so cleanup refuses while any are listed.
    pub(super) unreadable: Vec<(PathBuf, String)>,
}

/// A file action that may need an unsaved-changes decision first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PendingAction {
    /// New empty song.
    New,
    /// New song from the built-in demo.
    Demo,
    /// Open a `.cadence` file.
    Open,
    /// Open this `.cadence` file (Open Recent, a dropped song).
    OpenPath(PathBuf),
    /// Import a Standard MIDI File as a new song.
    ImportMidi,
    /// Import this MIDI file as a new song (a dropped `.mid`).
    ImportMidiPath(PathBuf),
    /// Quit the app.
    Quit,
}

/// eframe storage key of the "Normalize WAV Export" toggle ("1"/"0").
pub(super) const NORMALIZE_STORAGE_KEY: &str = "export.normalize";

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
            PendingAction::OpenPath(path) => self.open_path(path),
            PendingAction::ImportMidi => self.import_midi_dialog(),
            PendingAction::ImportMidiPath(path) => self.import_midi_file(path),
            PendingAction::Quit => self.allow_close = true,
        }
    }

    /// Replaces the song with a new one named `name` (trimmed; blank falls
    /// back to the default name) in `key`, built from `template` (Empty is
    /// the blank one-drum-track song). Used by the New Song dialog.
    pub(super) fn create_new_song(
        &mut self,
        name: &str,
        key: SongKey,
        template: ops::SongTemplate,
    ) {
        let mut project = ops::template_project(template, key);
        let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
        let name = name.trim_end();
        if !name.is_empty() {
            project.name = name.to_string();
        }
        project.key = SongKey::new(key.root, key.mode);
        self.replace_project(project, None, true);
        let text = match template {
            ops::SongTemplate::Empty => format!("Started a new song in {}.", ops::key_label(key)),
            t => format!(
                "Started a new {} song in {} — press Space to hear it.",
                t.label(),
                ops::key_label(key)
            ),
        };
        self.set_status(StatusKind::Success, text);
    }

    // ----- open / save -----------------------------------------------------

    /// Loads `path` as the current song. Used for the launch file and the
    /// open picker; on error the current project is kept and the problem is
    /// shown in the status bar.
    pub(super) fn open_path(&mut self, path: PathBuf) {
        match persistence::load_project(&path) {
            Ok(project) => {
                let shown = file_label(&path);
                // Audio this computer lacks may have travelled with the song
                // in its "<stem> Audio" folder: stage it before the
                // missing-file scan in `replace_project`.
                let found = if self.io_enabled {
                    persistence::import_song_audio(&project, &path).staged.len()
                } else {
                    0
                };
                self.remember_recent(&path);
                self.replace_project(project, Some(path), true);
                if found > 0 {
                    self.set_status(
                        StatusKind::Success,
                        format!(
                            "Opened {shown} and copied {found} audio file{} that came with it \
                             into this computer's audio store.",
                            plural(found)
                        ),
                    );
                } else {
                    self.set_status(StatusKind::Success, format!("Opened {shown}."));
                }
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
                "{} audio clip file{} missing from this computer's audio store — \
                 those clips are silent; re-import the original WAVs, or open a copy \
                 made with File ▸ Save Copy with Audio",
                n,
                super::sidebar::plural(n),
            ));
        }
        if !parts.is_empty() {
            self.set_status(StatusKind::Warning, format!("{}.", parts.join(". ")));
        }
    }

    // ----- recent files ----------------------------------------------------

    /// Puts `path` at the top of File ▸ Open Recent (saved in the desktop
    /// app; headless sessions only keep it in memory).
    pub(super) fn remember_recent(&mut self, path: &Path) {
        persistence::push_recent_file(&mut self.recent_files, path);
        self.store_recent_files();
    }

    /// File ▸ Open Recent ▸ Clear Recent.
    pub(super) fn clear_recent_files(&mut self) {
        self.recent_files.clear();
        self.store_recent_files();
        self.set_status(StatusKind::Info, "Cleared the recent songs list.");
    }

    /// The recent songs that still exist, for the menu.
    pub(super) fn existing_recent_files(&self) -> Vec<PathBuf> {
        self.recent_files
            .iter()
            .filter(|p| p.is_file())
            .cloned()
            .collect()
    }

    fn store_recent_files(&self) {
        if self.io_enabled {
            let _ = persistence::save_recent_files(&self.recent_files);
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
                self.remember_recent(&path);
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

    /// File ▸ Save Copy with Audio…: pick a `.cadence` path, then write the
    /// song plus a sibling `<name> Audio/` folder with every WAV it uses, so
    /// the copy plays on another computer. The open document (its path and
    /// saved state) is unchanged.
    pub(super) fn save_copy_with_audio_dialog(&mut self) {
        if !self.dialogs_available() {
            return;
        }
        let name = file_stem(&self.project.name);
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save a copy with its audio files")
            .add_filter("Cadence song", &[persistence::PROJECT_EXTENSION])
            .set_file_name(format!("{name}.{}", persistence::PROJECT_EXTENSION))
            .save_file()
        else {
            return;
        };
        self.save_copy_with_audio_to(path);
    }

    /// Writes the portable copy (helper path usable without dialogs).
    pub(super) fn save_copy_with_audio_to(&mut self, path: PathBuf) -> bool {
        let path = persistence::with_project_extension(&path);
        match persistence::save_project_with_audio(&self.project, &path) {
            Ok(bundle) => {
                let song = file_label(&path);
                let folder = file_label(&bundle.folder);
                let mut text = if bundle.files == 0 && bundle.missing.is_empty() {
                    format!("Saved a copy as {song}. The song has no audio clips to bundle.")
                } else {
                    format!(
                        "Saved a copy as {song} with {} audio file{} in “{folder}” — \
                         keep the folder next to the song when you move it.",
                        bundle.files,
                        plural(bundle.files)
                    )
                };
                let kind = if bundle.missing.is_empty() {
                    StatusKind::Success
                } else {
                    let n = bundle.missing.len();
                    text.push_str(&format!(
                        " {n} clip file{} {} missing here and stay{} silent.",
                        plural(n),
                        if n == 1 { "is" } else { "are" },
                        if n == 1 { "s" } else { "" }
                    ));
                    StatusKind::Warning
                };
                self.set_status(kind, text);
                true
            }
            Err(err) => {
                self.set_status(StatusKind::Error, err);
                false
            }
        }
    }

    // ----- audio store cleanup ----------------------------------------------

    /// File ▸ Clean Up Unused Audio…: lists the store's WAVs that nothing
    /// known uses and asks before deleting (never automatic). Desktop only —
    /// headless sessions must not look at the real data dir.
    pub(super) fn open_audio_cleanup(&mut self) {
        if !self.io_enabled {
            self.set_status(
                StatusKind::Warning,
                "Cleaning the audio store is only available in the desktop app.",
            );
            return;
        }
        self.plan_audio_cleanup_in(&persistence::audio_dir());
    }

    /// Scans `store` and opens the confirm dialog — or, when a recent song
    /// can't be read, the dialog that refuses and lists those songs (their
    /// audio can't be proven unused, so nothing may be offered for deletion).
    pub(super) fn plan_audio_cleanup_in(&mut self, store: &Path) {
        let in_use = self.referenced_audio();
        if !in_use.unreadable.is_empty() {
            self.dialog = Dialog::CleanAudioBlocked(in_use.unreadable);
            return;
        }
        self.offer_audio_cleanup(persistence::unused_audio_files_in(store, &in_use.names));
    }

    /// Shows the confirm dialog for `plan`, or says there is nothing to do.
    pub(super) fn offer_audio_cleanup(&mut self, plan: persistence::UnusedAudio) {
        if plan.files.is_empty() {
            self.set_status(
                StatusKind::Info,
                "Nothing to clean up: every file in the audio store is used by the open song, \
                 the autosave or a recent song.",
            );
        } else {
            self.dialog = Dialog::CleanAudio(plan);
        }
    }

    /// Store names still in use: the open song and every state undo/redo can
    /// restore, the crash-recovery autosave and each song in Open Recent
    /// (plus any file in a recent song's `<stem> Audio` folder). A recent
    /// song that can't be read is listed in `unreadable` — cleanup must not
    /// run while one is. (Songs elsewhere on disk can't be known — the
    /// dialog says so.)
    pub(super) fn referenced_audio(&self) -> AudioInUse {
        let mut in_use = AudioInUse {
            names: std::iter::once(&self.project)
                .chain(self.history.snapshots())
                .flat_map(persistence::audio_refs)
                .collect(),
            unreadable: Vec::new(),
        };
        if let Some(work) = self
            .recovery_dir
            .as_deref()
            .and_then(persistence::peek_recovery_in)
        {
            in_use.names.extend(persistence::audio_refs(&work));
        }
        if let Dialog::Recovery(work) = &self.dialog {
            in_use.names.extend(persistence::audio_refs(&work.project));
        }
        // The open song's file counts too (its saved version may still use
        // a clip the live state dropped). It never blocks: the live state
        // above is what an unreadable or deleted file would hold.
        let open = self
            .doc
            .path
            .as_ref()
            .filter(|p| !self.recent_files.contains(p));
        for (path, blocks) in self
            .recent_files
            .iter()
            .map(|p| (p, true))
            .chain(open.map(|p| (p, false)))
        {
            match persistence::load_project(path) {
                Ok(song) => in_use.names.extend(persistence::audio_refs(&song)),
                Err(err) if blocks => in_use.unreadable.push((path.clone(), err)),
                Err(_) => {}
            }
            in_use.names.extend(persistence::audio_folder_names(
                &persistence::song_audio_dir(path),
            ));
        }
        in_use
    }

    /// Removes `paths` from File ▸ Open Recent (the cleanup dialog's
    /// "Forget These Songs" for songs that are gone for good).
    pub(super) fn forget_recent_files(&mut self, paths: &[PathBuf]) {
        self.recent_files.retain(|p| !paths.contains(p));
        self.store_recent_files();
    }

    /// The confirmed cleanup: deletes the planned files that are *still*
    /// unused (re-checked now, in case a recent song changed meanwhile).
    /// Deletes nothing while a recent song can't be read.
    pub(super) fn run_audio_cleanup(&mut self, plan: &persistence::UnusedAudio) {
        let in_use = self.referenced_audio();
        if let Some((path, _)) = in_use.unreadable.first() {
            self.set_status(
                StatusKind::Error,
                format!(
                    "Nothing was deleted: the recent song {} can't be read, so its audio \
                     can't be checked.",
                    path.display()
                ),
            );
            return;
        }
        let refs = persistence::audio_names_folded(&in_use.names);
        let files: Vec<PathBuf> = plan
            .files
            .iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| !refs.contains(&n.to_string_lossy().to_ascii_lowercase()))
            })
            .cloned()
            .collect();
        let (removed, freed, errors) = persistence::remove_audio_files(&files);
        if errors.is_empty() {
            self.set_status(
                StatusKind::Success,
                format!(
                    "Removed {removed} unused audio file{} and freed {}.",
                    plural(removed),
                    format_bytes(freed)
                ),
            );
        } else {
            self.set_status(
                StatusKind::Warning,
                format!(
                    "Removed {removed} unused audio file{} ({}), but {} could not be deleted: {}",
                    plural(removed),
                    format_bytes(freed),
                    errors.len(),
                    errors[0]
                ),
            );
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

    /// WAV export options from the File menu's "Normalize WAV Export" toggle.
    pub(super) fn wav_export_options(&self) -> crate::audio::WavExportOptions {
        crate::audio::WavExportOptions {
            normalize_lufs: self
                .export_normalize
                .then_some(crate::audio::DEFAULT_TARGET_LUFS),
        }
    }

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
        let options = self.wav_export_options();
        std::thread::spawn(move || {
            let result = crate::audio::export_wav_with(&project, &render_path, options, move |p| {
                report.store(p.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            })
            .map(
                |done| match (options.normalize_lufs, done.integrated_lufs) {
                    (Some(_), Some(lufs)) => format!(
                        "Exported WAV at {lufs:.1} LUFS ({:+.1} dB): {}",
                        done.gain_db,
                        render_path.display()
                    ),
                    _ => format!("Exported WAV: {}", render_path.display()),
                },
            );
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
        self.place_audio_clip(path, track_id, start);
    }

    /// Body of the menu audio-clip imports (drops go through
    /// [`CadenceApp::open_dropped_paths`]'s worker instead).
    fn place_audio_clip(&mut self, path: PathBuf, track_id: Option<u64>, start: Option<f64>) {
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
            self.set_status(StatusKind::Warning, store_full_note(&name, grown, store));
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

    // ----- drag and drop ----------------------------------------------------

    /// Picks up files dropped on the window this frame (called from
    /// `frame()`): see [`CadenceApp::open_dropped_paths`].
    pub(super) fn handle_file_drops(&mut self, ctx: &egui::Context) {
        if self.drop_import.is_some() {
            self.poll_drop_import(false);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.open_dropped_paths(dropped);
        }
    }

    /// While files hover over the window: a full-window highlight saying
    /// what dropping them will do. Paint-only (a foreground layer), so it
    /// never steals input from the work surface.
    pub(super) fn drop_overlay(&self, ctx: &egui::Context) {
        let hovered: Vec<Option<PathBuf>> =
            ctx.input(|i| i.raw.hovered_files.iter().map(|f| f.path.clone()).collect());
        if hovered.is_empty() {
            return;
        }
        let pal = self.palette();
        let kinds: Vec<DropKind> = hovered.iter().flatten().map(|p| drop_kind(p)).collect();
        let title = drop_hover_title(&kinds, hovered.len());
        let rect = ctx.content_rect().shrink(theme::GAP_S);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("cadence-drop-overlay"),
        ));
        painter.rect(
            rect,
            theme::RADIUS_L,
            pal.bg.gamma_multiply(0.82),
            egui::Stroke::new(2.0_f32, pal.accent),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center() - egui::vec2(0.0, theme::GAP_M),
            egui::Align2::CENTER_CENTER,
            title,
            egui::FontId::new(
                theme::TEXT_HEADING,
                ctx.style()
                    .text_styles
                    .get(&theme::strong())
                    .map_or(egui::FontFamily::Proportional, |f| f.family.clone()),
            ),
            pal.text,
        );
        painter.text(
            rect.center() + egui::vec2(0.0, theme::GAP_L),
            egui::Align2::CENTER_CENTER,
            ".cadence songs · .mid files · .wav audio · .json packs · .sf2 SoundFonts",
            theme::body_font(theme::TEXT_BODY),
            pal.text_dim,
        );
    }

    /// Handles a drop by file type: a `.cadence` song opens and a `.mid`
    /// imports as a new song (both through the unsaved-changes guard), each
    /// `.wav` lands as an audio clip on a new audio track, `.json` installs an
    /// instrument pack and `.sf2` a SoundFont. Only one song can replace the
    /// current one, and audio dropped together with a song is skipped (it
    /// would land in the song being replaced) — the status line says so.
    ///
    /// The heavy part — decoding, hashing and copying WAVs, copying
    /// SoundFonts — runs on a worker thread ([`DropImport`]) so a large drop
    /// never freezes the window; the status bar counts the files and the
    /// results are applied on the GUI thread when the worker is done (all
    /// of a drop's clips as one undo step).
    pub(super) fn open_dropped_paths(&mut self, paths: Vec<PathBuf>) {
        let targets = DropTargets {
            audio: persistence::audio_dir(),
            fonts: crate::font::user_fonts_dir(),
        };
        self.open_dropped_paths_into(paths, targets);
    }

    /// [`CadenceApp::open_dropped_paths`] staging into explicit folders.
    pub(super) fn open_dropped_paths_into(&mut self, paths: Vec<PathBuf>, targets: DropTargets) {
        if self.dialog.is_open() {
            self.set_status(
                StatusKind::Info,
                "Close the open dialog first, then drop the files again.",
            );
            return;
        }
        if let Some(job) = &self.drop_import {
            let text = format!(
                "Still importing the files dropped before ({} of {} done) — drop these again \
                 when it finishes.",
                job.finished(),
                job.total
            );
            self.set_status(StatusKind::Info, text);
            return;
        }
        let mut replace: Option<(DropKind, PathBuf)> = None;
        let mut audio = Vec::new();
        let mut packs = Vec::new();
        let mut fonts = Vec::new();
        let mut report = DropReport::default();
        for path in paths {
            match drop_kind(&path) {
                kind @ (DropKind::Song | DropKind::Midi) => {
                    if replace.is_none() {
                        replace = Some((kind, path));
                    } else {
                        report.skipped.push(file_label(&path));
                    }
                }
                DropKind::Audio => audio.push(path),
                DropKind::Pack => packs.push(path),
                DropKind::SoundFont => fonts.push(path),
                DropKind::Unsupported => report.unsupported.push(file_label(&path)),
            }
        }
        if replace.is_some() {
            report
                .skipped
                .extend(audio.drain(..).map(|p| file_label(&p)));
        }
        // The song first: when the document is clean it opens right away and
        // the imports below land in it; otherwise its guard dialog comes up
        // and the installs (which never touch the song) still go ahead.
        report.replacing = replace.is_some();
        if let Some((kind, path)) = replace {
            self.collect_problems(&mut report.problems, |app| {
                if kind == DropKind::Song {
                    app.request(PendingAction::OpenPath(path));
                } else {
                    app.request(PendingAction::ImportMidiPath(path));
                }
            });
        }
        // Packs are small JSON files: installed right here.
        for path in packs {
            report.done += 1;
            self.collect_problems(&mut report.problems, |app| app.install_pack_file(path));
        }
        if audio.is_empty() && fonts.is_empty() {
            self.finish_drop_report(report);
            return;
        }
        let total = audio.len() + fonts.len();
        let progress = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let report_progress = progress.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let audio_store = targets.audio.clone();
        std::thread::spawn(move || {
            let mut out = Vec::with_capacity(total);
            for path in fonts {
                let result = install_soundfont_into(&targets.fonts, &path);
                out.push(Prepared::Font(result));
                report_progress.fetch_add(1, Ordering::Relaxed);
            }
            for path in audio {
                let result = persistence::stage_audio_file_into(&targets.audio, &path);
                out.push(Prepared::Audio(path, result));
                report_progress.fetch_add(1, Ordering::Relaxed);
            }
            let _ = tx.send(out);
        });
        let at = if self.transport.position.is_finite() {
            self.transport.position.max(0.0)
        } else {
            0.0
        };
        self.drop_import = Some(DropImport {
            rx,
            progress,
            total,
            shown: 0,
            at,
            song_replaced: false,
            store: audio_store,
            report,
        });
        // Results are only picked up on a later frame, so this line always
        // shows first.
        self.set_status(StatusKind::Info, importing_note(total, 0));
    }

    /// Runs `op` and keeps its status line in `problems` when it is a
    /// warning or error (a drop reports them together at the end).
    fn collect_problems(
        &mut self,
        problems: &mut Vec<(StatusKind, String)>,
        op: impl FnOnce(&mut Self),
    ) {
        self.status = None;
        op(self);
        if let Some(s) = &self.status {
            if matches!(s.kind, StatusKind::Error | StatusKind::Warning) {
                problems.push((s.kind, s.text.clone()));
            }
        }
    }

    /// The song was replaced (new/open/import/recover): audio a running drop
    /// import stages is no longer meant for this song (see `replace_project`).
    pub(super) fn drop_import_song_replaced(&mut self) {
        if let Some(job) = &mut self.drop_import {
            job.song_replaced = true;
        }
    }

    /// Checks the drop-import worker (every frame while one runs): updates
    /// the status-bar count and, once it is done, applies the results.
    /// `block` waits for the worker (tests).
    pub(super) fn poll_drop_import(&mut self, block: bool) {
        let Some(job) = &mut self.drop_import else {
            return;
        };
        let received = if block {
            job.rx.recv().map_err(|_| ())
        } else {
            match job.rx.try_recv() {
                Ok(out) => Ok(out),
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    let done = job.finished();
                    if job.shown != done {
                        job.shown = done;
                        let text = importing_note(job.total, done);
                        self.set_status(StatusKind::Info, text);
                    }
                    return;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => Err(()),
            }
        };
        let job = self.drop_import.take().expect("checked above");
        let mut report = job.report;
        let out = match received {
            Ok(out) => out,
            Err(()) => {
                report.done += job.total;
                report.problems.push((
                    StatusKind::Error,
                    "The import worker stopped unexpectedly.".to_owned(),
                ));
                Vec::new()
            }
        };
        let mut clips = Vec::new();
        let mut placed_audio = false;
        for prepared in out {
            report.done += 1;
            match prepared {
                Prepared::Font(result) => {
                    self.collect_problems(&mut report.problems, |app| {
                        app.apply_soundfont_install(result)
                    });
                }
                Prepared::Audio(_, Err(err)) => {
                    report
                        .problems
                        .push((StatusKind::Error, format!("Could not import audio: {err}")));
                }
                Prepared::Audio(path, Ok(staged)) => clips.push((path, staged)),
            }
        }
        if !clips.is_empty() {
            if job.song_replaced {
                let n = clips.len();
                let text = format!(
                    "{n} dropped audio file{} {} not added: another song was opened while \
                     {} imported",
                    plural(n),
                    if n == 1 { "was" } else { "were" },
                    if n == 1 {
                        "it was being"
                    } else {
                        "they were being"
                    }
                );
                // One entry per file keeps "Added X of Y" right.
                report
                    .problems
                    .extend(std::iter::repeat_n((StatusKind::Warning, text), n));
            } else {
                let mut refused = 0;
                self.collect_problems(&mut report.problems, |app| {
                    refused = app.place_dropped_audio(clips, job.at);
                });
                if let Some(last) = report.problems.last().cloned() {
                    let extra = refused.saturating_sub(1);
                    report.problems.extend(std::iter::repeat_n(last, extra));
                }
                placed_audio = true;
            }
        }
        self.finish_drop_report(report);
        let store = persistence::audio_store_bytes_in(&job.store);
        if placed_audio && store > persistence::AUDIO_STORE_WARN_BYTES {
            let done = self
                .status
                .as_ref()
                .map(|s| s.text.clone())
                .unwrap_or_default();
            self.set_status(
                StatusKind::Warning,
                format!("{done} {}", store_size_note(store))
                    .trim()
                    .to_owned(),
            );
        }
    }

    /// Blocks until a running drop import is done and applied (tests).
    #[cfg(test)]
    pub(super) fn wait_for_drop_import(&mut self) {
        while self.drop_import.is_some() {
            self.poll_drop_import(true);
        }
    }

    /// The status line summing up a drop of several files (a single file
    /// keeps the line its own import set).
    fn finish_drop_report(&mut self, report: DropReport) {
        let DropReport {
            done,
            problems,
            skipped,
            unsupported,
            replacing,
        } = report;
        let several =
            done + usize::from(replacing) > 1 || !skipped.is_empty() || !unsupported.is_empty();
        if !several {
            if let Some((kind, first)) = problems.first() {
                if self.status.as_ref().is_none_or(|s| s.text != *first) {
                    self.set_status(*kind, first.clone());
                }
            }
            return;
        }
        let mut parts = Vec::new();
        if done > 0 {
            let ok = done.saturating_sub(problems.len());
            parts.push(format!("Added {ok} of {done} dropped file{}", plural(done)));
        }
        if let Some((_, first)) = problems.first() {
            parts.push(first.trim_end_matches('.').to_owned());
        }
        if !skipped.is_empty() {
            parts.push(format!(
                "Skipped {} (drop one song at a time, and audio separately from songs)",
                skipped.join(", ")
            ));
        }
        if !unsupported.is_empty() {
            parts.push(format!(
                "Can't open {} — drop .cadence songs, .mid files, .wav audio, \
                 .json instrument packs or .sf2 SoundFonts",
                unsupported.join(", ")
            ));
        }
        let kind = if problems.is_empty() && skipped.is_empty() && unsupported.is_empty() {
            StatusKind::Success
        } else {
            StatusKind::Warning
        };
        if !parts.is_empty() {
            self.set_status(kind, format!("{}.", parts.join(". ")));
        }
    }

    /// Puts each staged drop on a fresh audio track at `at` — all of them
    /// one undoable edit — and selects the last clip. Returns how many did
    /// not fit (track limit).
    fn place_dropped_audio(
        &mut self,
        clips: Vec<(PathBuf, persistence::StagedAudio)>,
        at: f64,
    ) -> usize {
        let per_step = self.project.seconds_per_step();
        let n = clips.len();
        let mut placed: Vec<(u64, String, f64)> = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        let label = if n == 1 {
            "Import audio file".to_owned()
        } else {
            format!("Import {n} audio files")
        };
        self.edit(&label, None, |p| {
            for (path, staged) in &clips {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| staged.file.clone());
                let length = (staged.seconds / per_step).max(crate::model::MIN_AUDIO_CLIP_STEPS);
                let clip = ops::add_audio_track(p)
                    .and_then(|id| ops::add_audio_clip(p, id, &staged.file, &name, at, length));
                match clip {
                    Some(id) => placed.push((id, name, staged.seconds)),
                    None => refused.push(name),
                }
            }
        });
        if let Some((clip_id, _, _)) = placed.last() {
            let clip_id = *clip_id;
            let tid = self.clip_track_id(clip_id);
            self.set_single_clip_selection(clip_id);
            if let Some(id) = tid {
                self.arranger.reveal_track = Some(id);
            }
        }
        if let Some(name) = refused.first() {
            self.set_status(
                StatusKind::Warning,
                format!("Could not import “{name}”: the song is at the track limit."),
            );
        } else if let [(_, name, seconds)] = placed.as_slice() {
            self.set_status(
                StatusKind::Success,
                format!(
                    "Imported “{name}” on a new audio track ({seconds:.1}s). The file lives in \
                     the app's audio store."
                ),
            );
        }
        refused.len()
    }

    /// Reloads the font catalog after a SoundFont install (from a drop's
    /// worker) and reports it. A file that turns out not to parse is
    /// removed again so it never lingers as a load error.
    fn apply_soundfont_install(&mut self, result: Result<FontInstall, String>) {
        match result {
            Ok(FontInstall::Already(name)) => self.set_status(
                StatusKind::Info,
                format!("The SoundFont “{name}” is already installed."),
            ),
            Ok(FontInstall::Installed(dest)) => {
                let name = dest
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.reload_fonts();
                let failed = self.io_enabled && !self.fonts.fonts.iter().any(|f| f.name == name);
                if failed {
                    let _ = std::fs::remove_file(&dest);
                    self.reload_fonts();
                    self.set_status(
                        StatusKind::Error,
                        format!("Could not install “{name}”: it is not a readable SoundFont."),
                    );
                } else {
                    self.set_status(
                        StatusKind::Success,
                        format!(
                            "Installed the SoundFont “{name}”. Pick it from a track's sound menu."
                        ),
                    );
                }
            }
            Err(err) => self.set_status(
                StatusKind::Error,
                format!("Could not install SoundFont: {err}"),
            ),
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

/// "Imported “x”. The audio store now holds 2.1 GB — …" once the store
/// passes [`persistence::AUDIO_STORE_WARN_BYTES`].
fn store_full_note(name: &str, grown: &str, store: u64) -> String {
    format!("Imported “{name}”{grown}. {}", store_size_note(store))
}

/// "The audio store now holds 2.1 GB — …".
fn store_size_note(store: u64) -> String {
    format!(
        "The audio store now holds {:.1} GB — old imports are never deleted automatically; \
         use File ▸ Clean Up Unused Audio… if disk runs low.",
        store as f64 / (1024.0 * 1024.0 * 1024.0)
    )
}

/// "Importing 12 dropped files… 3 of 12 done" for the status bar.
fn importing_note(total: usize, done: usize) -> String {
    match (total, done) {
        (1, _) => "Importing 1 dropped file…".to_owned(),
        (_, 0) => format!("Importing {total} dropped files…"),
        _ => format!("Importing {total} dropped files… {done} of {total} done"),
    }
}

/// Where a drop stages its files: the audio store and the SoundFont folder
/// (tests point these at temp folders).
#[derive(Clone, Debug)]
pub(super) struct DropTargets {
    pub(super) audio: PathBuf,
    pub(super) fonts: PathBuf,
}

/// One file the drop-import worker prepared.
enum Prepared {
    Font(Result<FontInstall, String>),
    Audio(PathBuf, Result<persistence::StagedAudio, String>),
}

/// What a multi-file drop reports when it is done.
#[derive(Default)]
struct DropReport {
    /// Files imported or installed (or attempted).
    done: usize,
    /// Status lines (and kinds) of the ones that failed.
    problems: Vec<(StatusKind, String)>,
    /// Names not imported (a second song, audio dropped with a song).
    skipped: Vec<String>,
    /// Names of unsupported files.
    unsupported: Vec<String>,
    /// A song or MIDI file in the drop replaces the current song.
    replacing: bool,
}

/// A drop's WAVs and SoundFonts being staged on a worker thread; polled by
/// [`CadenceApp::poll_drop_import`] each frame.
pub(super) struct DropImport {
    rx: crossbeam_channel::Receiver<Vec<Prepared>>,
    /// Files the worker has finished.
    progress: Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    /// The finished count the status bar shows.
    shown: usize,
    /// Playhead at the drop: where the clips start.
    at: f64,
    /// Set when the song was replaced meanwhile: the clips are not added.
    song_replaced: bool,
    /// The audio store the WAVs are staged into.
    store: PathBuf,
    report: DropReport,
}

impl DropImport {
    fn finished(&self) -> usize {
        self.progress.load(Ordering::Relaxed).min(self.total)
    }
}

/// What dropping a file on the window does, by extension (case-insensitive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropKind {
    /// `.cadence`: open the song.
    Song,
    /// `.mid` / `.midi`: import as a new song.
    Midi,
    /// `.wav`: import as an audio clip on a new audio track.
    Audio,
    /// `.json`: install as an instrument pack.
    Pack,
    /// `.sf2`: install into the SoundFont folder.
    SoundFont,
    /// Anything else (including folders without a known extension).
    Unsupported,
}

/// Classifies a dropped path by its extension.
pub(super) fn drop_kind(path: &Path) -> DropKind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        e if e == persistence::PROJECT_EXTENSION => DropKind::Song,
        "mid" | "midi" => DropKind::Midi,
        "wav" | "wave" => DropKind::Audio,
        "json" => DropKind::Pack,
        "sf2" => DropKind::SoundFont,
        _ => DropKind::Unsupported,
    }
}

/// Outcome of [`install_soundfont_into`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FontInstall {
    /// Copied to this path.
    Installed(PathBuf),
    /// An identical file with this font name was already installed.
    Already(String),
}

/// Copies the `.sf2` at `src` into the SoundFont folder `dir` (created on
/// demand) under its own file name — the stem is the font name songs
/// reference. Checks the size cap and the RIFF `sfbk` header first; a
/// different file under an installed name is refused rather than
/// overwritten (songs using that name would change sound). The copy goes
/// through a temp file and a rename, so a failed copy leaves nothing behind.
pub(super) fn install_soundfont_into(dir: &Path, src: &Path) -> Result<FontInstall, String> {
    let shown = file_label(src);
    let meta = std::fs::metadata(src).map_err(|err| format!("Could not open {shown}: {err}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a file."));
    }
    if meta.len() > crate::font::MAX_FONT_BYTES {
        return Err(format!(
            "{shown} is too large ({} MB, limit {} MB).",
            meta.len() / (1024 * 1024),
            crate::font::MAX_FONT_BYTES / (1024 * 1024)
        ));
    }
    let mut head = [0u8; 12];
    File::open(src)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|_| format!("{shown} is not a SoundFont (.sf2) file."))?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"sfbk" {
        return Err(format!("{shown} is not a SoundFont (.sf2) file."));
    }
    let Some(file_name) = src.file_name() else {
        return Err(format!("{shown} has no file name."));
    };
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.trim().is_empty() {
        return Err(format!("{shown} has no usable name."));
    }
    let dest = dir.join(file_name);
    if dest.exists() {
        if same_file_contents(src, &dest) {
            return Ok(FontInstall::Already(name));
        }
        return Err(format!(
            "a different SoundFont named “{name}” is already installed. \
             Rename the file to install it alongside."
        ));
    }
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    let tmp = dir.join(format!(".{name}.{}.sf2.tmp", std::process::id()));
    let copied = std::fs::copy(src, &tmp).and_then(|_| std::fs::rename(&tmp, &dest));
    if let Err(err) = copied {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Could not copy {shown}: {err}"));
    }
    Ok(FontInstall::Installed(dest))
}

/// Byte-for-byte comparison, streamed (fonts can be hundreds of MB).
fn same_file_contents(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(fa), Ok(fb)) = (File::open(a), File::open(b)) else {
        return false;
    };
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let n = match ra.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if n == 0 {
            return true;
        }
        if rb.read_exact(&mut bb[..n]).is_err() || ba[..n] != bb[..n] {
            return false;
        }
    }
}

/// The drop overlay's headline for the hovered files (`kinds` may be empty
/// when the platform doesn't report paths until the drop).
fn drop_hover_title(kinds: &[DropKind], count: usize) -> String {
    let only = |k: DropKind| !kinds.is_empty() && kinds.iter().all(|x| *x == k);
    match kinds {
        [] => format!(
            "Drop {} to open or import",
            if count == 1 { "the file" } else { "the files" }
        ),
        [DropKind::Song] => "Drop to open this song".to_owned(),
        [DropKind::Midi] => "Drop to import this MIDI file as a new song".to_owned(),
        _ if only(DropKind::Audio) => format!(
            "Drop to import {} on new audio track{}",
            if kinds.len() == 1 {
                "the audio file".to_owned()
            } else {
                format!("{} audio files", kinds.len())
            },
            plural(kinds.len())
        ),
        _ if only(DropKind::Pack) => "Drop to install the instrument pack".to_owned(),
        _ if only(DropKind::SoundFont) => "Drop to install the SoundFont".to_owned(),
        _ if only(DropKind::Unsupported) => "Cadence can't open this file type".to_owned(),
        _ => format!("Drop to open or import {} files", kinds.len()),
    }
}

/// "512 KB", "12.3 MB", "1.2 GB".
pub(super) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.0} KB", (b / KB).ceil())
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Context, DroppedFile, HoveredFile, RawInput};

    fn frame(app: &mut CadenceApp, ctx: &Context, input: RawInput) {
        let _ = ctx.run(input, |ctx| app.frame(ctx));
    }

    #[test]
    fn drops_are_classified_by_extension() {
        for (name, kind) in [
            ("a.cadence", DropKind::Song),
            ("A.CADENCE", DropKind::Song),
            ("x.mid", DropKind::Midi),
            ("x.MIDI", DropKind::Midi),
            ("take.wav", DropKind::Audio),
            ("pack.json", DropKind::Pack),
            ("GM.sf2", DropKind::SoundFont),
            ("song.mp3", DropKind::Unsupported),
            ("folder", DropKind::Unsupported),
        ] {
            assert_eq!(drop_kind(Path::new(name)), kind, "{name}");
        }
        assert_eq!(
            drop_hover_title(&[DropKind::Audio, DropKind::Audio], 2),
            "Drop to import 2 audio files on new audio tracks"
        );
        assert!(drop_hover_title(&[], 1).contains("the file"));
    }

    #[test]
    fn dropped_song_opens_through_the_unsaved_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Dropped.cadence");
        let mut song = Project::empty();
        song.name = "Dropped Song".into();
        persistence::save_project(&song, &path).unwrap();

        // Clean document: opens right away, through a real frame's input.
        let mut app = CadenceApp::headless(Project::demo());
        let ctx = Context::default();
        let mut input = RawInput::default();
        input.dropped_files.push(DroppedFile {
            path: Some(path.clone()),
            ..Default::default()
        });
        frame(&mut app, &ctx, input);
        assert_eq!(app.project.name, "Dropped Song");
        assert_eq!(app.doc.path.as_deref(), Some(path.as_path()));

        // Dirty document: the guard comes up holding the dropped path.
        app.edit("Rename", None, |p| p.name.push('!'));
        app.open_dropped_paths(vec![path.clone()]);
        assert!(
            matches!(&app.dialog, Dialog::Unsaved(PendingAction::OpenPath(p)) if *p == path),
            "{:?}",
            app.dialog
        );
        // While a dialog is up further drops are refused, not queued.
        app.open_dropped_paths(vec![dir.path().join("other.cadence")]);
        assert!(matches!(&app.dialog, Dialog::Unsaved(PendingAction::OpenPath(p)) if *p == path));
        app.run_pending_action(PendingAction::OpenPath(path));
        assert_eq!(app.project.name, "Dropped Song");
    }

    #[test]
    fn dropped_midi_imports_and_odd_files_explain_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let mid = dir.path().join("groove.mid");
        std::fs::write(&mid, crate::midi::export_midi(&Project::demo()).unwrap()).unwrap();
        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, "hi").unwrap();

        let mut app = CadenceApp::headless(Project::empty());
        app.open_dropped_paths(vec![txt.clone()]);
        let status = app.status.clone().unwrap();
        assert_eq!(status.kind, StatusKind::Warning);
        assert!(
            status.text.contains("Can't open notes.txt"),
            "{}",
            status.text
        );

        // A MIDI file plus an audio file: the MIDI imports, the audio is
        // skipped (it would land in the song being replaced) and said so.
        let before = app.project.tracks.len();
        app.open_dropped_paths(vec![mid, dir.path().join("take.wav")]);
        assert!(
            app.project.tracks.len() > before,
            "MIDI import replaced the song"
        );
        assert!(app.project.tracks.iter().all(|t| !t.audio));
        let status = app.status.clone().unwrap();
        assert!(status.text.contains("Skipped take.wav"), "{}", status.text);
    }

    #[test]
    fn hovering_files_paints_the_overlay_without_input_side_effects() {
        let mut app = CadenceApp::headless(Project::demo());
        let ctx = Context::default();
        let project = app.project.clone();
        for _ in 0..2 {
            let mut input = RawInput::default();
            input.hovered_files.push(HoveredFile {
                path: Some("take.wav".into()),
                ..Default::default()
            });
            frame(&mut app, &ctx, input);
        }
        assert_eq!(app.project, project);
        assert!(!app.dialog.is_open());
    }

    #[test]
    fn opening_and_saving_feed_open_recent_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cadence");
        let b = dir.path().join("b.cadence");
        persistence::save_project(&Project::empty(), &a).unwrap();

        let mut app = CadenceApp::headless(Project::demo());
        app.open_path(a.clone());
        assert!(app.save_to(b.clone()));
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(app.recent_files, vec![canon(&b), canon(&a)]);
        // Re-opening moves to the top without duplicating.
        app.open_path(a.clone());
        assert_eq!(app.recent_files, vec![canon(&a), canon(&b)]);
        // Missing songs stay remembered but are not offered.
        std::fs::remove_file(&b).unwrap();
        assert_eq!(app.existing_recent_files(), vec![canon(&a)]);
        // A failed open is not remembered.
        app.open_path(dir.path().join("missing.cadence"));
        assert_eq!(app.recent_files.len(), 2);
        app.clear_recent_files();
        assert!(app.recent_files.is_empty());
        // Headless: nothing was written next to the songs.
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn save_copy_with_audio_leaves_the_document_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = CadenceApp::headless(Project::demo());
        app.edit("Rename", None, |p| p.name = "Portable".into());
        let state = app.history.state();
        assert!(app.save_copy_with_audio_to(dir.path().join("Copy")));
        let copy = dir.path().join("Copy.cadence");
        assert_eq!(persistence::load_project(&copy).unwrap().name, "Portable");
        // The demo has no audio clips: no folder, and still untitled + dirty.
        assert!(!dir.path().join("Copy Audio").exists());
        assert_eq!(app.doc.path, None);
        assert!(app.dirty());
        assert_eq!(app.history.state(), state);
        let status = app.status.clone().unwrap();
        assert_eq!(status.kind, StatusKind::Success, "{}", status.text);
    }

    #[test]
    fn audio_cleanup_keeps_everything_still_in_use() {
        let store = tempfile::tempdir().unwrap();
        let songs = tempfile::tempdir().unwrap();
        let wav = |name: &str| {
            let p = store.path().join(name);
            std::fs::write(&p, vec![1u8; 2048]).unwrap();
            name.to_owned()
        };
        let (live, undo, recent, autosave, stale) = (
            wav("1111111111111111.wav"),
            wav("2222222222222222.wav"),
            wav("3333333333333333.wav"),
            wav("4444444444444444.wav"),
            wav("5555555555555555.wav"),
        );
        let with_clip = |p: &mut Project, file: &str| {
            let mut track = crate::model::create_audio_track("Audio");
            track
                .audio_clips
                .push(crate::model::AudioClip::new(file, "Take", 0.0, 16.0));
            p.tracks.push(track);
        };

        let mut app = CadenceApp::headless(Project::empty());
        // A clip that only undo can bring back still counts.
        app.edit("Add", None, |p| with_clip(p, &undo));
        app.edit("Remove", None, |p| p.tracks.clear());
        app.edit("Add live", None, |p| with_clip(p, &live));
        // A recent song and the autosave.
        let mut song = Project::empty();
        with_clip(&mut song, &recent);
        let song_path = songs.path().join("recent.cadence");
        persistence::save_project(&song, &song_path).unwrap();
        app.recent_files = vec![song_path];
        let rec = songs.path().join("recovery");
        let mut saved = Project::empty();
        with_clip(&mut saved, &autosave);
        persistence::save_recovery_in(&rec, &saved, None).unwrap();
        app.recovery_dir = Some(rec);

        let refs = app.referenced_audio().names;
        let plan = persistence::unused_audio_files_in(store.path(), &refs);
        assert_eq!(plan.files, vec![store.path().join(&stale)]);
        app.offer_audio_cleanup(plan.clone());
        assert!(matches!(app.dialog, Dialog::CleanAudio(_)));

        // Confirming deletes only the stale file.
        app.dialog = Dialog::None;
        app.run_audio_cleanup(&plan);
        assert!(!store.path().join(&stale).exists());
        for name in [&live, &undo, &recent, &autosave] {
            assert!(store.path().join(name).is_file(), "{name}");
        }
        assert!(app.status.clone().unwrap().text.contains("freed 2 KB"));
        // Nothing left: no dialog, just a note.
        app.offer_audio_cleanup(persistence::unused_audio_files_in(store.path(), &refs));
        assert!(!app.dialog.is_open());
        // Headless sessions never scan the real store.
        app.open_audio_cleanup();
        assert!(!app.dialog.is_open());
        assert_eq!(app.status.clone().unwrap().kind, StatusKind::Warning);
    }

    /// A recent song that can't be read (an unplugged drive) might use any
    /// store file, so cleanup refuses until it is back or forgotten; files
    /// in a recent song's `<stem> Audio` folder always count as used.
    #[test]
    fn audio_cleanup_refuses_while_a_recent_song_is_unreadable() {
        let store = tempfile::tempdir().unwrap();
        let songs = tempfile::tempdir().unwrap();
        let wav = |name: &str| {
            std::fs::write(store.path().join(name), vec![1u8; 1024]).unwrap();
            name.to_owned()
        };
        let (stale, bundled) = (wav("5555555555555555.wav"), wav("6666666666666666.wav"));
        let song_path = songs.path().join("Song.cadence");
        persistence::save_project(&Project::empty(), &song_path).unwrap();
        let folder = persistence::song_audio_dir(&song_path);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(&bundled), b"copy").unwrap();
        let gone = songs.path().join("Unplugged").join("Away.cadence");

        let mut app = CadenceApp::headless(Project::empty());
        app.recent_files = vec![gone.clone(), song_path];
        app.plan_audio_cleanup_in(store.path());
        match &app.dialog {
            Dialog::CleanAudioBlocked(songs) => {
                assert_eq!(songs.iter().map(|s| &s.0).collect::<Vec<_>>(), [&gone]);
            }
            other => panic!("expected the blocked dialog, got {other:?}"),
        }
        // A confirm that races the drive going away deletes nothing either.
        let plan = persistence::UnusedAudio {
            files: vec![store.path().join(&stale)],
            bytes: 1024,
        };
        app.dialog = Dialog::None;
        app.run_audio_cleanup(&plan);
        assert!(store.path().join(&stale).is_file());
        assert_eq!(app.status.clone().unwrap().kind, StatusKind::Error);

        // Forgetting the unreadable song lets the scan go ahead; the bundled
        // copy's name is still kept.
        app.forget_recent_files(std::slice::from_ref(&gone));
        assert_eq!(app.recent_files.len(), 1);
        app.plan_audio_cleanup_in(store.path());
        match &app.dialog {
            Dialog::CleanAudio(plan) => {
                assert_eq!(plan.files, vec![store.path().join(&stale)]);
            }
            other => panic!("expected the confirm dialog, got {other:?}"),
        }
    }

    /// The confirm-time re-check ignores ASCII case like the scan does: a
    /// clip referencing `take.wav` keeps a stored `TAKE.WAV`.
    #[test]
    fn audio_cleanup_recheck_ignores_name_case() {
        let store = tempfile::tempdir().unwrap();
        let file = store.path().join("TAKE.WAV");
        std::fs::write(&file, b"x").unwrap();
        let mut app = CadenceApp::headless(Project::empty());
        app.edit("Add", None, |p| {
            let mut track = crate::model::create_audio_track("Audio");
            track
                .audio_clips
                .push(crate::model::AudioClip::new("take.wav", "Take", 0.0, 16.0));
            p.tracks.push(track);
        });
        let plan = persistence::UnusedAudio {
            files: vec![file.clone()],
            bytes: 1,
        };
        app.run_audio_cleanup(&plan);
        assert!(file.is_file());
    }

    fn tone_wav(dir: &Path, name: &str, hz: f32) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..4_000u32 {
            let t = i as f32 / 8_000.0;
            w.write_sample(((t * hz * std::f32::consts::TAU).sin() * 9_000.0) as i16)
                .unwrap();
        }
        w.finalize().unwrap();
        path
    }

    /// Dropped WAVs and SoundFonts are staged on a worker: the drop returns
    /// at once with an "Importing…" status, a second drop meanwhile is
    /// refused, and the clips land in one undo step when it finishes.
    #[test]
    fn dropped_files_import_on_a_worker_as_one_undo_step() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let targets = DropTargets {
            audio: data.path().join("audio"),
            fonts: data.path().join("fonts"),
        };
        let a = tone_wav(src.path(), "a.wav", 220.0);
        let b = tone_wav(src.path(), "b.wav", 330.0);
        let bogus = src.path().join("bogus.wav");
        std::fs::write(&bogus, b"not a wav").unwrap();
        let font = fake_font(src.path(), "Keys.sf2", b"body");

        let mut app = CadenceApp::headless(Project::empty());
        let tracks = app.project.tracks.len();
        app.open_dropped_paths_into(vec![a.clone(), b, bogus, font], targets.clone());
        assert!(app.drop_import.is_some());
        let status = app.status.clone().unwrap();
        assert!(
            status.text.contains("Importing 4 dropped files"),
            "{}",
            status.text
        );
        // Busy: a further drop is refused, not queued.
        app.open_dropped_paths_into(vec![a], targets.clone());
        assert!(app.status.clone().unwrap().text.contains("Still importing"));

        app.wait_for_drop_import();
        assert!(app.drop_import.is_none());
        assert_eq!(app.project.tracks.len(), tracks + 2);
        assert_eq!(std::fs::read_dir(&targets.audio).unwrap().count(), 2);
        assert!(targets.fonts.join("Keys.sf2").is_file());
        let status = app.status.clone().unwrap();
        assert_eq!(status.kind, StatusKind::Warning);
        assert!(
            status.text.contains("Added 3 of 4 dropped files"),
            "{}",
            status.text
        );
        // Both clips came in as one step.
        app.undo();
        assert_eq!(app.project.tracks.len(), tracks);
    }

    /// Audio staged for a song that was replaced meanwhile is not dropped
    /// into the new song.
    #[test]
    fn dropped_audio_skips_a_song_replaced_while_importing() {
        let src = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let targets = DropTargets {
            audio: data.path().join("audio"),
            fonts: data.path().join("fonts"),
        };
        let a = tone_wav(src.path(), "a.wav", 220.0);
        let mut app = CadenceApp::headless(Project::empty());
        app.open_dropped_paths_into(vec![a], targets);
        app.run_pending_action(PendingAction::Demo);
        let demo = app.project.clone();
        app.wait_for_drop_import();
        assert_eq!(app.project, demo);
        let status = app.status.clone().unwrap();
        assert_eq!(status.kind, StatusKind::Warning);
        assert!(
            status.text.contains("another song was opened"),
            "{}",
            status.text
        );
    }

    fn fake_font(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
        let mut bytes = b"RIFF\0\0\0\0sfbk".to_vec();
        bytes.extend_from_slice(body);
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn soundfonts_install_once_and_never_overwrite() {
        let src = tempfile::tempdir().unwrap();
        let fonts = tempfile::tempdir().unwrap();
        let dir = fonts.path().join("fonts");
        let font = fake_font(src.path(), "Piano.sf2", b"one");

        let dest = dir.join("Piano.sf2");
        assert_eq!(
            install_soundfont_into(&dir, &font),
            Ok(FontInstall::Installed(dest.clone()))
        );
        assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&font).unwrap());
        assert_eq!(
            install_soundfont_into(&dir, &font),
            Ok(FontInstall::Already("Piano".into()))
        );

        // Same name, different bytes: refused, installed file untouched.
        let other = tempfile::tempdir().unwrap();
        let clash = fake_font(other.path(), "Piano.sf2", b"two");
        let err = install_soundfont_into(&dir, &clash).unwrap_err();
        assert!(err.contains("already installed"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(&font).unwrap());

        // Not a SoundFont, or missing: errors, and nothing else is left.
        let bogus = src.path().join("Fake.sf2");
        std::fs::write(&bogus, b"RIFF\0\0\0\0WAVEfmt ").unwrap();
        assert!(install_soundfont_into(&dir, &bogus).is_err());
        assert!(install_soundfont_into(&dir, &src.path().join("nope.sf2")).is_err());
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(names.len(), 1);
    }
}
