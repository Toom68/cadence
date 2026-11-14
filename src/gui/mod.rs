//! Native eframe/egui interface for Cadence.
//!
//! The UI owns the [`Project`] and an undo [`History`]. Every user edit goes
//! through [`CadenceApp::edit`], which records an undo snapshot and marks the
//! audio engine as needing the new project. The engine is only updated on
//! frames where the project actually changed, never every frame.

mod arranger;
mod dialogs;
mod editor;
mod files;
mod history;
mod mixer;
mod ops;
mod sidebar;
#[cfg(test)]
mod tests;
mod theme;
mod transport;

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use egui::{
    Color32, Context, Frame, Key, KeyboardShortcut, Margin, Modifiers, RichText, Stroke,
    ViewportCommand,
};

use crate::audio::AudioEngine;
use crate::model::{Instrument, Project, Track};

pub use history::History;

use dialogs::Dialog;
use files::{Document, ExportJob, PendingAction};
use theme::{Icon, BG, BORDER_SOFT, CORAL, LIME, TEXT_DIM, TEXT_FAINT};
use transport::TransportState;

const AUTOSAVE_DELAY: Duration = Duration::from_millis(1500);

const fn sc(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}
const CMD_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
pub(crate) const SC_NEW: KeyboardShortcut = sc(Modifiers::COMMAND, Key::N);
pub(crate) const SC_OPEN: KeyboardShortcut = sc(Modifiers::COMMAND, Key::O);
pub(crate) const SC_SAVE: KeyboardShortcut = sc(Modifiers::COMMAND, Key::S);
pub(crate) const SC_SAVE_AS: KeyboardShortcut = sc(CMD_SHIFT, Key::S);
pub(crate) const SC_EXPORT_WAV: KeyboardShortcut = sc(Modifiers::COMMAND, Key::E);
pub(crate) const SC_QUIT: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Q);
pub(crate) const SC_UNDO: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Z);
pub(crate) const SC_REDO: KeyboardShortcut = sc(CMD_SHIFT, Key::Z);
pub(crate) const SC_REDO_ALT: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Y);
pub(crate) const SC_DUPLICATE: KeyboardShortcut = sc(Modifiers::COMMAND, Key::D);
pub(crate) const SC_SELECT_ALL: KeyboardShortcut = sc(Modifiers::COMMAND, Key::A);
pub(crate) const SC_EDITOR: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Num1);
pub(crate) const SC_MIXER: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Num2);
pub(crate) const SC_HELP: KeyboardShortcut = sc(Modifiers::NONE, Key::F1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Editor,
    Mixer,
}

/// Which area keyboard edits (Delete, arrows, Cmd+D) apply to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Arranger,
    Editor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
struct StatusLine {
    kind: StatusKind,
    text: String,
}

#[derive(Default, Debug, Clone)]
struct Selection {
    track: Option<u64>,
    clip: Option<u64>,
    clips: BTreeSet<u64>,
    notes: BTreeSet<u64>,
    /// Pattern shown in the editor for each track.
    patterns: HashMap<u64, u64>,
}

/// The Cadence desktop application.
pub struct CadenceApp {
    project: Project,
    history: History,
    doc: Document,
    engine: Option<AudioEngine>,
    audio_error: Option<String>,
    transport: TransportState,
    sel: Selection,
    arranger: arranger::ArrangerView,
    editor: editor::EditorView,
    tab: Tab,
    focus: Focus,
    dialog: Dialog,
    export: Option<ExportJob>,
    status: Option<StatusLine>,
    engine_dirty: bool,
    last_edit: Option<Instant>,
    autosaved_state: u64,
    /// False in headless tests: no recovery writes and no native dialogs.
    io_enabled: bool,
    allow_close: bool,
    recovery_offered: bool,
    side_tab: sidebar::SideTab,
    library_filter: String,
    /// Bank filter for the instrument library (view state only, never saved).
    library_bank: sidebar::LibraryBank,
    /// Last-frame library chrome geometry for headless tests.
    library_rects: sidebar::LibraryRects,
    catalog: crate::packs::PackCatalog,
    /// Fingerprint of the packs dir at the last catalog refresh.
    pack_stamp: u64,
    window_title: String,
    style_applied: bool,
}

impl CadenceApp {
    /// Creates the app: demo song on first launch, the launch file if one was
    /// given, and a recovery offer when a valid autosave exists.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let (engine, audio_error) = match AudioEngine::new() {
            Ok(engine) => (Some(engine), None),
            Err(err) => (None, Some(err)),
        };
        let mut app = Self::build(Project::demo(), engine, true);
        app.style_applied = true;
        if let Some(err) = audio_error {
            app.set_status(
                StatusKind::Warning,
                format!(
                    "Audio output unavailable ({err}). Editing, saving and WAV export still work."
                ),
            );
            app.audio_error = Some(err);
        }
        if let Some(path) = crate::launch_path() {
            app.open_path(path.to_path_buf());
        } else {
            app.check_recovery();
        }
        app
    }

    fn build(project: Project, engine: Option<AudioEngine>, io_enabled: bool) -> Self {
        let history = History::new();
        let state = history.state();
        if let Some(engine) = &engine {
            engine.set_project(&project);
        }
        let mut app = Self {
            project,
            history,
            doc: Document {
                path: None,
                saved_state: Some(state),
            },
            engine,
            audio_error: None,
            transport: TransportState::default(),
            sel: Selection::default(),
            arranger: arranger::ArrangerView::default(),
            editor: editor::EditorView::default(),
            tab: Tab::Editor,
            focus: Focus::Arranger,
            dialog: Dialog::None,
            export: None,
            status: None,
            engine_dirty: false,
            last_edit: None,
            autosaved_state: state,
            io_enabled,
            allow_close: false,
            recovery_offered: false,
            side_tab: sidebar::SideTab::default(),
            library_filter: String::new(),
            library_bank: sidebar::LibraryBank::default(),
            library_rects: sidebar::LibraryRects::default(),
            catalog: if io_enabled {
                crate::packs::load_catalog()
            } else {
                crate::packs::PackCatalog::default()
            },
            pack_stamp: if io_enabled { pack_fingerprint() } else { 0 },
            window_title: String::new(),
            style_applied: false,
        };
        app.fix_selection();
        app
    }

    /// Headless constructor for tests: no audio device, no file I/O.
    #[cfg(test)]
    pub(crate) fn headless(project: Project) -> Self {
        Self::build(project, None, false)
    }

    // ----- editing ---------------------------------------------------------

    /// Applies an edit. Records undo only when the project actually changed.
    /// Edits sharing a `group` key coalesce into one undo step per gesture.
    fn edit(&mut self, label: &str, group: Option<&str>, f: impl FnOnce(&mut Project)) -> bool {
        let before = self.project.clone();
        f(&mut self.project);
        if self.project == before {
            return false;
        }
        self.history.record(before, label, group);
        self.project_changed();
        true
    }

    fn project_changed(&mut self) {
        self.engine_dirty = true;
        self.last_edit = Some(Instant::now());
    }

    fn undo(&mut self) {
        if let Some(label) = self.history.undo(&mut self.project) {
            self.project_changed();
            self.fix_selection();
            self.set_status(StatusKind::Info, format!("Undid: {label}"));
        }
    }

    fn redo(&mut self) {
        if let Some(label) = self.history.redo(&mut self.project) {
            self.project_changed();
            self.fix_selection();
            self.set_status(StatusKind::Info, format!("Redid: {label}"));
        }
    }

    fn dirty(&self) -> bool {
        self.doc.saved_state != Some(self.history.state())
    }

    /// Replaces the whole project (new/open/import/recover).
    fn replace_project(&mut self, project: Project, path: Option<std::path::PathBuf>, clean: bool) {
        self.stop();
        self.transport.position = 0.0;
        self.transport.start_marker = 0.0;
        self.project = project;
        self.history.reset();
        self.doc.path = path;
        self.doc.saved_state = clean.then(|| self.history.state());
        self.autosaved_state = self.history.state();
        self.sel = Selection::default();
        self.arranger.scroll_x = 0.0;
        self.arranger.scroll_y = 0.0;
        self.arranger.reset_gesture();
        self.editor.reset_view();
        self.engine_dirty = true;
        self.last_edit = (!clean).then(Instant::now);
        self.fix_selection();
    }

    // ----- selection -------------------------------------------------------

    fn fix_selection(&mut self) {
        let project = &self.project;
        self.sel.patterns.retain(|t, p| {
            project
                .tracks
                .iter()
                .any(|tr| tr.id == *t && tr.patterns.iter().any(|x| x.id == *p))
        });
        let existing_clips: BTreeSet<u64> = project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        self.sel.clips.retain(|id| existing_clips.contains(id));
        if self.sel.clip.is_some_and(|id| existing_clips.contains(&id)) {
            if let Some(id) = self.sel.clip {
                self.sel.clips.insert(id);
            }
        } else {
            self.sel.clip = self.sel.clips.iter().next().copied();
        }
        if let Some(track) = self.sel.clip.and_then(|clip| self.clip_track_id(clip)) {
            self.sel.track = Some(track);
        }
        if self
            .sel
            .track
            .and_then(|id| ops::track(project, id))
            .is_none()
        {
            self.sel.track = self
                .sel
                .clip
                .and_then(|clip| self.clip_track_id(clip))
                .or_else(|| project.tracks.first().map(|t| t.id));
            self.sel.notes.clear();
        }
        let Some(track) = self.sel.track.and_then(|id| ops::track(&self.project, id)) else {
            self.sel.clip = None;
            self.sel.clips.clear();
            self.sel.notes.clear();
            return;
        };
        let notes: BTreeSet<u64> = self
            .current_pattern_id(track)
            .and_then(|pid| ops::pattern(track, pid))
            .map(|p| {
                p.notes
                    .iter()
                    .map(|n| n.id)
                    .filter(|id| self.sel.notes.contains(id))
                    .collect()
            })
            .unwrap_or_default();
        self.sel.notes = notes;
    }

    fn selected_track(&self) -> Option<&Track> {
        self.sel.track.and_then(|id| ops::track(&self.project, id))
    }

    fn current_pattern_id(&self, track: &Track) -> Option<u64> {
        self.sel
            .patterns
            .get(&track.id)
            .copied()
            .filter(|pid| track.patterns.iter().any(|p| p.id == *pid))
            .or_else(|| track.patterns.first().map(|p| p.id))
    }

    fn select_track(&mut self, id: u64) {
        if self.sel.track != Some(id) {
            self.sel.track = Some(id);
            self.sel.clip = None;
            self.sel.clips.clear();
            self.sel.notes.clear();
        }
    }

    fn select_pattern(&mut self, track_id: u64, pattern_id: u64) {
        if self.sel.patterns.get(&track_id) != Some(&pattern_id) {
            self.sel.notes.clear();
        }
        self.sel.patterns.insert(track_id, pattern_id);
    }

    fn clip_track_id(&self, clip_id: u64) -> Option<u64> {
        self.project
            .tracks
            .iter()
            .find(|t| t.clips.iter().any(|c| c.id == clip_id))
            .map(|t| t.id)
    }

    fn selected_clip_ids(&self) -> BTreeSet<u64> {
        let mut ids = self.sel.clips.clone();
        if let Some(id) = self.sel.clip {
            ids.insert(id);
        }
        ids
    }

    fn set_clip_selection(&mut self, ids: BTreeSet<u64>, primary: Option<u64>) {
        let primary = primary.filter(|id| ids.contains(id));
        self.sel.clips = ids;
        self.sel.clip = primary.or_else(|| self.sel.clips.iter().next().copied());
        if let Some(track) = self.sel.clip.and_then(|clip| self.clip_track_id(clip)) {
            self.sel.track = Some(track);
        }
        self.sel.notes.clear();
    }

    fn set_single_clip_selection(&mut self, clip: u64) {
        let mut ids = BTreeSet::new();
        ids.insert(clip);
        self.set_clip_selection(ids, Some(clip));
    }

    fn clear_arranger_clip_selection(&mut self) {
        self.sel.clip = None;
        self.sel.clips.clear();
    }

    fn clear_selection(&mut self) {
        self.clear_arranger_clip_selection();
        self.sel.notes.clear();
    }

    fn selected_clip_snapshots(&self) -> Vec<(u64, crate::model::Clip)> {
        ops::collect_clips(&self.project, &self.selected_clip_ids())
    }

    fn select_all_arranger_clips(&mut self) {
        let ids: BTreeSet<u64> = self
            .project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        self.set_clip_selection(ids, self.sel.clip);
        self.focus = Focus::Arranger;
    }

    fn delete_selection(&mut self) {
        let Some(track_id) = self.sel.track else {
            return;
        };
        if self.focus == Focus::Editor && !self.sel.notes.is_empty() {
            let Some(pid) = self
                .selected_track()
                .and_then(|t| self.current_pattern_id(t))
            else {
                return;
            };
            let ids: Vec<u64> = self.sel.notes.iter().copied().collect();
            let n = ids.len();
            if self.edit("Delete notes", None, |p| {
                if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                    ops::delete_notes(pat, &ids);
                }
            }) {
                self.sel.notes.clear();
                self.set_status(
                    StatusKind::Info,
                    format!("Deleted {n} note{}", if n == 1 { "" } else { "s" }),
                );
            }
        } else {
            let clips = self.selected_clip_snapshots();
            if !clips.is_empty() {
                let n = clips.len();
                if self.edit(
                    if n == 1 {
                        "Delete clip"
                    } else {
                        "Delete clips"
                    },
                    None,
                    |p| {
                        for (track, clip) in &clips {
                            ops::delete_clip(p, *track, clip.id);
                        }
                    },
                ) {
                    self.clear_arranger_clip_selection();
                }
            }
        }
    }

    fn duplicate_selection(&mut self) {
        let Some(track_id) = self.sel.track else {
            return;
        };
        if self.focus == Focus::Editor && !self.sel.notes.is_empty() {
            let Some(pid) = self
                .selected_track()
                .and_then(|t| self.current_pattern_id(t))
            else {
                return;
            };
            let ids: Vec<u64> = self.sel.notes.iter().copied().collect();
            let mut new = Vec::new();
            self.edit("Duplicate notes", None, |p| {
                if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                    new = ops::duplicate_notes(pat, &ids);
                }
            });
            if !new.is_empty() {
                self.sel.notes = new.into_iter().collect();
            }
        } else {
            let clips = self.selected_clip_snapshots();
            if !clips.is_empty() {
                let primary = self.sel.clip;
                let mut new = None;
                self.edit(
                    if clips.len() == 1 {
                        "Duplicate clip"
                    } else {
                        "Duplicate clips"
                    },
                    None,
                    |p| new = ops::duplicate_clip_group_after(p, &clips),
                );
                match new {
                    Some(ids) => {
                        let set: BTreeSet<u64> = ids.iter().copied().collect();
                        let primary = primary.and_then(|old| {
                            clips
                                .iter()
                                .position(|(_, c)| c.id == old)
                                .and_then(|i| ids.get(i).copied())
                        });
                        self.set_clip_selection(set, primary);
                    }
                    None => self.set_status(
                        StatusKind::Warning,
                        format!(
                            "Could not duplicate selection (tracks hold up to {} clips).",
                            ops::MAX_CLIPS_PER_TRACK
                        ),
                    ),
                }
            }
        }
    }

    fn select_all_notes(&mut self) {
        if let Some(track) = self.selected_track() {
            if let Some(pat) = self
                .current_pattern_id(track)
                .and_then(|pid| ops::pattern(track, pid))
            {
                self.sel.notes = pat.notes.iter().map(|n| n.id).collect();
                self.focus = Focus::Editor;
                self.tab = Tab::Editor;
            }
        }
    }

    fn select_all(&mut self) {
        if self.focus == Focus::Arranger {
            self.select_all_arranger_clips();
        } else {
            self.select_all_notes();
        }
    }

    /// Arrow-key nudging of the current selection.
    fn nudge(&mut self, steps: i32, semitones: i32) {
        let Some(track_id) = self.sel.track else {
            return;
        };
        if self.focus == Focus::Editor && !self.sel.notes.is_empty() {
            let Some(pid) = self
                .selected_track()
                .and_then(|t| self.current_pattern_id(t))
            else {
                return;
            };
            let ids = self.sel.notes.clone();
            let snap = self.editor.snap;
            self.edit(
                if semitones != 0 {
                    "Transpose notes"
                } else {
                    "Move notes"
                },
                Some("nudge-notes"),
                |p| {
                    if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                        let orig: Vec<_> = pat
                            .notes
                            .iter()
                            .filter(|n| ids.contains(&n.id))
                            .cloned()
                            .collect();
                        ops::move_notes(pat, &orig, steps as f64 * snap, semitones);
                    }
                },
            );
            if semitones != 0 {
                if let Some(&first) = self.sel.notes.iter().next() {
                    let pitch = self
                        .selected_track()
                        .and_then(|t| ops::pattern(t, pid))
                        .and_then(|p| p.notes.iter().find(|n| n.id == first))
                        .map(|n| n.pitch);
                    if let Some(pitch) = pitch {
                        self.preview(track_id, pitch, 0.8);
                    }
                }
            }
        } else if semitones == 0 {
            let clips = self.selected_clip_snapshots();
            if !clips.is_empty() {
                let grid = self.arranger.snap.steps();
                self.edit("Move clips", Some("nudge-clips"), |p| {
                    ops::move_clip_group(p, &clips, steps as f64 * grid);
                });
            }
        }
    }

    fn add_track(&mut self, instrument: Instrument) {
        let mut id = None;
        self.edit(&format!("Add {} track", instrument.name()), None, |p| {
            id = ops::add_track(p, instrument)
        });
        match id {
            Some(id) => {
                self.select_track(id);
                self.tab = Tab::Editor;
                self.focus = Focus::Editor;
                self.arranger.reveal_track = Some(id);
                self.set_status(
                    StatusKind::Success,
                    format!("Added a {} track", instrument.name()),
                );
            }
            None => self.set_status(
                StatusKind::Warning,
                format!("Track limit reached ({} tracks).", ops::MAX_TRACKS),
            ),
        }
    }

    fn preview(&self, track_id: u64, pitch: u8, velocity: f32) {
        if let (Some(engine), Some(track)) = (&self.engine, ops::track(&self.project, track_id)) {
            engine.preview(track, pitch, 0.45, velocity);
        }
    }

    fn set_status(&mut self, kind: StatusKind, text: impl Into<String>) {
        self.status = Some(StatusLine {
            kind,
            text: text.into(),
        });
    }

    // ----- instrument packs ------------------------------------------------

    /// Re-reads the embedded + installed pack catalog. Files.rs calls this
    /// after installing a pack; the 2s poller calls it on directory changes.
    fn refresh_pack_catalog(&mut self) {
        if self.io_enabled {
            self.catalog = crate::packs::load_catalog();
            self.repair_library_bank();
        }
    }

    /// A bank filter naming a pack that no longer loads falls back to All.
    /// View state only — the song and its dirty flag are untouched.
    fn repair_library_bank(&mut self) {
        if let sidebar::LibraryBank::Pack(ns) = &self.library_bank {
            let alive = self
                .catalog
                .instruments
                .iter()
                .any(|p| sidebar::preset_namespace(&p.id) == ns.as_str());
            if !alive {
                self.library_bank = sidebar::LibraryBank::All;
            }
        }
    }

    /// Reloads the pack catalog. When `announce` is false this is a cheap
    /// no-op unless the packs directory actually changed (the 2s poll in
    /// files.rs relies on that so it can run unconditionally).
    fn reload_packs(&mut self, announce: bool) {
        let stamp = pack_fingerprint();
        if !announce && stamp == self.pack_stamp {
            return;
        }
        self.pack_stamp = stamp;
        let n_errors_before = self.catalog.errors.len();
        self.refresh_pack_catalog();
        let n = self.catalog.errors.len();
        if announce {
            let count = self.catalog.instruments.len();
            if n == 0 {
                self.set_status(
                    StatusKind::Success,
                    format!(
                        "Library refreshed: {count} pack preset{} available.",
                        sidebar::plural(count)
                    ),
                );
            } else {
                self.set_status(
                    StatusKind::Warning,
                    format!(
                        "Library refreshed, but {n} pack file{} did not load.",
                        sidebar::plural(n)
                    ),
                );
                self.show_pack_errors();
            }
        } else if n != n_errors_before {
            self.set_status(StatusKind::Warning, format!("A pack on disk changed; {n} file{} did not load (see the library for details).", sidebar::plural(n)));
        }
    }

    fn show_pack_errors(&mut self) {
        if !self.catalog.errors.is_empty() {
            self.dialog = Dialog::PackErrors(self.catalog.errors.clone());
        }
    }

    // ----- frame -----------------------------------------------------------

    /// Runs one UI frame. Separated from `eframe::App::update` so tests can
    /// drive the full interface with a bare `egui::Context`.
    fn frame(&mut self, ctx: &Context) {
        if !self.style_applied {
            theme::apply(ctx);
            self.style_applied = true;
        }
        self.poll_engine();
        self.poll_export();
        self.handle_close_request(ctx);
        self.handle_shortcuts(ctx);

        self.menu_bar(ctx);
        self.transport_bar(ctx);
        self.status_bar(ctx);
        self.sidebar(ctx);
        self.central(ctx);
        self.dialogs(ctx);

        if ctx.input(|i| i.pointer.any_released()) {
            self.history.seal();
            self.arranger.end_gesture();
            self.editor.end_gesture();
        }
        self.sync_engine();
        self.autosave(ctx);
        self.update_title(ctx);
        self.schedule_repaint(ctx);
    }

    fn sync_engine(&mut self) {
        if !self.engine_dirty {
            return;
        }
        self.engine_dirty = false;
        if let Some(engine) = &self.engine {
            engine.set_project(&self.project);
        }
    }

    fn autosave(&mut self, ctx: &Context) {
        let state = self.history.state();
        if !self.io_enabled
            || !self.dirty()
            || state == self.autosaved_state
            || self.recovery_offered
        {
            return;
        }
        let elapsed = self.last_edit.map_or(AUTOSAVE_DELAY, |t| t.elapsed());
        if elapsed < AUTOSAVE_DELAY {
            ctx.request_repaint_after(AUTOSAVE_DELAY - elapsed);
            return;
        }
        self.autosaved_state = state;
        if let Err(err) = crate::persistence::save_recovery(&self.project) {
            self.set_status(StatusKind::Warning, format!("Autosave failed: {err}"));
        }
    }

    fn schedule_repaint(&self, ctx: &Context) {
        if self.transport.playing {
            ctx.request_repaint();
        } else if self.export.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        } else if self.transport.peak.iter().any(|p| *p > 0.001) {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn update_title(&mut self, ctx: &Context) {
        let file = self
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let name = file.unwrap_or_else(|| self.project.name.clone());
        let title = format!("{}{name} — Cadence", if self.dirty() { "● " } else { "" });
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn handle_close_request(&mut self, ctx: &Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.allow_close || !self.dirty() {
            if self.io_enabled && !self.recovery_offered {
                let _ = crate::persistence::clear_recovery();
            }
            return;
        }
        ctx.send_viewport_cmd(ViewportCommand::CancelClose);
        self.dialog = Dialog::Unsaved(PendingAction::Quit);
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let typing = ctx.wants_keyboard_input();
        if self.dialog.is_open() {
            if !typing && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.dialog_escape();
            }
            return;
        }
        let pressed = |s: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(s));
        if pressed(&SC_SAVE_AS) {
            self.save_as();
        } else if pressed(&SC_SAVE) {
            self.save();
        }
        if pressed(&SC_OPEN) {
            self.request(PendingAction::Open);
        }
        if pressed(&SC_NEW) {
            self.request(PendingAction::New);
        }
        if pressed(&SC_EXPORT_WAV) {
            self.export_wav_dialog();
        }
        if pressed(&SC_QUIT) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if pressed(&SC_HELP) {
            self.dialog = Dialog::Help;
        }
        if pressed(&SC_EDITOR) {
            self.tab = Tab::Editor;
        }
        if pressed(&SC_MIXER) {
            self.tab = Tab::Mixer;
        }
        if typing {
            return;
        }
        if pressed(&SC_REDO) || pressed(&SC_REDO_ALT) {
            self.redo();
        } else if pressed(&SC_UNDO) {
            self.undo();
        }
        if pressed(&SC_DUPLICATE) {
            self.duplicate_selection();
        }
        if pressed(&SC_SELECT_ALL) {
            self.select_all();
        }
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if key(Modifiers::NONE, Key::Space) {
            self.toggle_play();
        }
        if key(Modifiers::NONE, Key::Home) {
            self.seek(0.0);
        }
        if key(Modifiers::NONE, Key::L) {
            self.toggle_loop();
        }
        if key(Modifiers::NONE, Key::Escape) {
            self.clear_selection();
        }
        if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
            self.delete_selection();
        }
        if key(Modifiers::SHIFT, Key::ArrowUp) {
            self.nudge(0, 12);
        } else if key(Modifiers::NONE, Key::ArrowUp) {
            self.nudge(0, 1);
        }
        if key(Modifiers::SHIFT, Key::ArrowDown) {
            self.nudge(0, -12);
        } else if key(Modifiers::NONE, Key::ArrowDown) {
            self.nudge(0, -1);
        }
        if key(Modifiers::NONE, Key::ArrowLeft) {
            self.nudge(-1, 0);
        }
        if key(Modifiers::NONE, Key::ArrowRight) {
            self.nudge(1, 0);
        }
        if key(Modifiers::NONE, Key::Equals) || key(Modifiers::NONE, Key::Plus) {
            self.zoom(1.25);
        }
        if key(Modifiers::NONE, Key::Minus) {
            self.zoom(0.8);
        }
    }

    /// Zooms whichever timeline has keyboard focus.
    fn zoom(&mut self, factor: f32) {
        if self.focus == Focus::Editor && self.tab == Tab::Editor {
            self.editor.zoom(factor);
        } else {
            self.arranger.zoom_by(factor, None);
        }
    }

    // ----- layout ----------------------------------------------------------

    fn menu_bar(&mut self, ctx: &Context) {
        let frame = Frame::new().fill(BG).inner_margin(Margin::symmetric(8, 3));
        egui::TopBottomPanel::top("menu_bar")
            .frame(frame)
            .show_separator_line(false)
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    let egui_ctx = ui.ctx().clone();
                    let fmt = move |s: &KeyboardShortcut| egui_ctx.format_shortcut(s);
                    let item = |text: &str, s: Option<&KeyboardShortcut>| {
                        let b = egui::Button::new(text.to_owned()).min_size(egui::vec2(220.0, 0.0));
                        match s {
                            Some(s) => b.shortcut_text(fmt(s)),
                            None => b,
                        }
                    };
                    ui.menu_button("File", |ui| {
                        if ui.add(item("New Song", Some(&SC_NEW))).clicked() {
                            self.request(PendingAction::New);
                        }
                        if ui.add(item("New from Demo", None)).clicked() {
                            self.request(PendingAction::Demo);
                        }
                        if ui.add(item("Open…", Some(&SC_OPEN))).clicked() {
                            self.request(PendingAction::Open);
                        }
                        ui.separator();
                        if ui.add(item("Save", Some(&SC_SAVE))).clicked() {
                            self.save();
                        }
                        if ui.add(item("Save As…", Some(&SC_SAVE_AS))).clicked() {
                            self.save_as();
                        }
                        ui.separator();
                        if ui.add(item("Import MIDI…", None)).clicked() {
                            self.request(PendingAction::ImportMidi);
                        }
                        if ui.add(item("Export MIDI…", None)).clicked() {
                            self.export_midi_dialog();
                        }
                        let exporting = self.export.is_some();
                        if ui
                            .add_enabled(
                                !exporting,
                                item(
                                    if exporting {
                                        "Exporting WAV…"
                                    } else {
                                        "Export WAV…"
                                    },
                                    Some(&SC_EXPORT_WAV),
                                ),
                            )
                            .clicked()
                        {
                            self.export_wav_dialog();
                        }
                        ui.separator();
                        if ui
                            .add(item(
                                if cfg!(target_os = "macos") {
                                    "Quit Cadence"
                                } else {
                                    "Exit"
                                },
                                Some(&SC_QUIT),
                            ))
                            .clicked()
                        {
                            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        let undo = self
                            .history
                            .undo_label()
                            .map_or("Undo".to_owned(), |l| format!("Undo {l}"));
                        if ui
                            .add_enabled(self.history.can_undo(), item(&undo, Some(&SC_UNDO)))
                            .clicked()
                        {
                            self.undo();
                        }
                        let redo = self
                            .history
                            .redo_label()
                            .map_or("Redo".to_owned(), |l| format!("Redo {l}"));
                        if ui
                            .add_enabled(self.history.can_redo(), item(&redo, Some(&SC_REDO)))
                            .clicked()
                        {
                            self.redo();
                        }
                        ui.separator();
                        let has_sel =
                            !self.sel.notes.is_empty() || !self.selected_clip_ids().is_empty();
                        if ui
                            .add_enabled(has_sel, item("Duplicate", Some(&SC_DUPLICATE)))
                            .clicked()
                        {
                            self.duplicate_selection();
                        }
                        if ui
                            .add_enabled(has_sel, item("Delete", None).shortcut_text("Del"))
                            .clicked()
                        {
                            self.delete_selection();
                        }
                        if ui
                            .add(item("Select All Notes", Some(&SC_SELECT_ALL)))
                            .clicked()
                        {
                            self.select_all_notes();
                        }
                        if ui
                            .add(item("Deselect", None).shortcut_text("Esc"))
                            .clicked()
                        {
                            self.clear_selection();
                        }
                        ui.separator();
                        ui.menu_button("Add Track", |ui| {
                            for inst in Instrument::ALL {
                                if ui
                                    .add_enabled(
                                        self.project.tracks.len() < ops::MAX_TRACKS,
                                        egui::Button::new(inst.name()),
                                    )
                                    .clicked()
                                {
                                    self.add_track(inst);
                                }
                            }
                        });
                    });
                    ui.menu_button("Transport", |ui| {
                        let play = if self.transport.playing {
                            "Pause"
                        } else {
                            "Play"
                        };
                        if ui.add(item(play, None).shortcut_text("Space")).clicked() {
                            self.toggle_play();
                        }
                        if ui.add(item("Stop", None)).clicked() {
                            self.stop();
                        }
                        if ui
                            .add(item("Return to Start", None).shortcut_text("Home"))
                            .clicked()
                        {
                            self.seek(0.0);
                        }
                        ui.separator();
                        let mut looping = self.transport.looping;
                        if ui.checkbox(&mut looping, "Loop song  (L)").changed() {
                            self.toggle_loop();
                        }
                        let mut metro = self.transport.metronome;
                        if ui.checkbox(&mut metro, "Metronome").changed() {
                            self.toggle_metronome();
                        }
                    });
                    ui.menu_button("View", |ui| {
                        if ui.add(item("Pattern Editor", Some(&SC_EDITOR))).clicked() {
                            self.tab = Tab::Editor;
                        }
                        if ui.add(item("Mixer", Some(&SC_MIXER))).clicked() {
                            self.tab = Tab::Mixer;
                        }
                        ui.separator();
                        if ui.add(item("Zoom In", None).shortcut_text("=")).clicked() {
                            self.zoom(1.25);
                        }
                        if ui.add(item("Zoom Out", None).shortcut_text("-")).clicked() {
                            self.zoom(0.8);
                        }
                        if ui.add(item("Fit Song in Arranger", None)).clicked() {
                            self.arranger.fit_request = true;
                        }
                        ui.checkbox(&mut self.arranger.follow, "Follow playhead");
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.add(item("Shortcuts & Tips", Some(&SC_HELP))).clicked() {
                            self.dialog = Dialog::Help;
                        }
                        if ui.add(item("About Cadence", None)).clicked() {
                            self.dialog = Dialog::About;
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let redo_tip = self.history.redo_label().map_or("Redo".to_owned(), |l| {
                            format!("Redo {l} ({})", fmt(&SC_REDO))
                        });
                        if ui
                            .add_enabled_ui(self.history.can_redo(), |ui| {
                                theme::icon_button(ui, Icon::Redo, 24.0, false, LIME, &redo_tip)
                            })
                            .inner
                            .clicked()
                        {
                            self.redo();
                        }
                        let undo_tip = self.history.undo_label().map_or("Undo".to_owned(), |l| {
                            format!("Undo {l} ({})", fmt(&SC_UNDO))
                        });
                        if ui
                            .add_enabled_ui(self.history.can_undo(), |ui| {
                                theme::icon_button(ui, Icon::Undo, 24.0, false, LIME, &undo_tip)
                            })
                            .inner
                            .clicked()
                        {
                            self.undo();
                        }
                        ui.add_space(8.0);
                        let (text, color) = if self.dirty() {
                            ("Unsaved changes", theme::AMBER)
                        } else {
                            ("All changes saved", TEXT_FAINT)
                        };
                        let shown = if self.doc.path.is_none() && !self.dirty() {
                            "Not saved yet"
                        } else {
                            text
                        };
                        ui.label(RichText::new(shown).small().color(color));
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &Context) {
        let frame = Frame::new()
            .fill(BG)
            .inner_margin(Margin::symmetric(12, 4))
            .stroke(Stroke::new(1.0_f32, BORDER_SOFT));
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(28.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(status) = &self.status {
                        let color = match status.kind {
                            StatusKind::Info => TEXT_DIM,
                            StatusKind::Success => LIME,
                            StatusKind::Warning => theme::AMBER,
                            StatusKind::Error => CORAL,
                        };
                        theme::color_dot(ui, color, 7.0);
                        ui.add(
                            egui::Label::new(RichText::new(&status.text).small().color(TEXT_DIM))
                                .truncate(),
                        );
                    } else {
                        ui.label(
                            RichText::new("Ready · Press F1 for shortcuts")
                                .small()
                                .color(TEXT_FAINT),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match (&self.engine, &self.audio_error) {
                            (Some(_), _) => {
                                let err = self.transport.engine_error.clone();
                                let (c, t) = match err {
                                    Some(e) => (CORAL, format!("Audio error: {e}")),
                                    None => (
                                        LIME,
                                        format!("Audio ready · {} voices", self.transport.voices),
                                    ),
                                };
                                ui.label(RichText::new(t).small().color(TEXT_DIM));
                                theme::color_dot(ui, c, 7.0);
                            }
                            (None, err) => {
                                if ui
                                    .small_button("Retry audio")
                                    .on_hover_text(err.clone().unwrap_or_default())
                                    .clicked()
                                {
                                    self.retry_audio();
                                }
                                ui.label(
                                    RichText::new(
                                        "Offline (no audio output) · editing & export work",
                                    )
                                    .small()
                                    .color(TEXT_DIM),
                                );
                                theme::color_dot(ui, CORAL, 7.0);
                            }
                        }
                        ui.add_space(14.0);
                        let total = self.project.total_steps();
                        let secs = ops::steps_to_seconds(total, self.project.bpm);
                        let t = self.project.tracks.len();
                        ui.label(
                            RichText::new(format!(
                                "{t} track{} · {} bars · {}",
                                if t == 1 { "" } else { "s" },
                                self.project.bars,
                                ops::format_time(secs)
                            ))
                            .small()
                            .color(TEXT_FAINT),
                        );
                        if let Some(job) = &self.export {
                            ui.add_space(14.0);
                            let p = job.progress();
                            ui.add(
                                egui::ProgressBar::new(p)
                                    .desired_width(140.0)
                                    .desired_height(10.0)
                                    .fill(LIME),
                            );
                            ui.label(
                                RichText::new(format!("Exporting WAV {:.0}%", p * 100.0))
                                    .small()
                                    .color(LIME),
                            );
                        }
                    });
                });
            });
    }

    fn retry_audio(&mut self) {
        match AudioEngine::new() {
            Ok(engine) => {
                engine.set_project(&self.project);
                engine.set_metronome(self.transport.metronome);
                self.engine = Some(engine);
                self.audio_error = None;
                self.set_status(StatusKind::Success, "Audio output connected.");
            }
            Err(err) => {
                self.set_status(StatusKind::Warning, format!("Still no audio output: {err}"));
                self.audio_error = Some(err);
            }
        }
    }

    fn central(&mut self, ctx: &Context) {
        let frame = Frame::new().fill(BG).inner_margin(Margin {
            left: 8,
            right: 10,
            top: 8,
            bottom: 8,
        });
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            if self.project.tracks.is_empty() {
                self.empty_state(ui);
                return;
            }
            let total = ui.available_height();
            let max = (total - 250.0).max(200.0);
            egui::TopBottomPanel::top("arranger_panel")
                .resizable(true)
                .show_separator_line(false)
                .default_height((total * 0.43).clamp(200.0, max))
                .height_range(170.0..=max)
                .frame(Frame::new().inner_margin(Margin {
                    bottom: 8,
                    ..Margin::ZERO
                }))
                .show_inside(ui, |ui| {
                    theme::card().inner_margin(Margin::same(0)).show(ui, |ui| {
                        ui.set_min_size(ui.available_size());
                        self.arranger_ui(ui);
                    });
                });
            theme::card().inner_margin(Margin::same(0)).show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.bottom_tabs(ui);
                match self.tab {
                    Tab::Editor => self.editor_ui(ui),
                    Tab::Mixer => self.mixer_ui(ui),
                }
            });
        });
    }

    fn bottom_tabs(&mut self, ui: &mut egui::Ui) {
        Frame::new()
            .inner_margin(Margin {
                left: 12,
                right: 12,
                top: 8,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (tab, label, s) in [
                        (Tab::Editor, "Pattern Editor", &SC_EDITOR),
                        (Tab::Mixer, "Mixer", &SC_MIXER),
                    ] {
                        let on = self.tab == tab;
                        let text =
                            RichText::new(label).color(if on { theme::TEXT } else { TEXT_DIM });
                        let b = egui::Button::new(text)
                            .selected(on)
                            .min_size(egui::vec2(120.0, 28.0))
                            .corner_radius(egui::CornerRadius {
                                nw: 7,
                                ne: 7,
                                sw: 0,
                                se: 0,
                            });
                        let b = if on {
                            b.fill(theme::CONTROL)
                                .stroke(Stroke::new(1.0_f32, theme::BORDER))
                        } else {
                            b.fill(Color32::TRANSPARENT).stroke(Stroke::NONE)
                        };
                        if ui
                            .add(b)
                            .on_hover_text(format!("{label} ({})", ui.ctx().format_shortcut(s)))
                            .clicked()
                        {
                            self.tab = tab;
                        }
                    }
                });
            });
        let r = ui.cursor();
        ui.painter()
            .hline(r.x_range(), r.top(), Stroke::new(1.0_f32, theme::BORDER));
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.25).max(20.0));
                ui.label(theme::kicker_text("EMPTY SONG"));
                ui.label(RichText::new("Start by adding an instrument").size(22.0).color(theme::TEXT));
                ui.add_space(4.0);
                ui.label(RichText::new("Each track gets its own patterns, clips and mixer channel. You can add up to 32 tracks.").color(TEXT_DIM));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    let w = 6.0 * 128.0 + 5.0 * 8.0;
                    ui.add_space(((ui.available_width() - w) * 0.5).max(0.0));
                    for inst in Instrument::ALL {
                        let color = theme::rgb(sidebar::instrument_color(inst));
                        let b = egui::Button::new(RichText::new(format!("+  {}", inst.name())).color(theme::INK).strong()).fill(color).min_size(egui::vec2(128.0, 40.0));
                        if ui.add(b).on_hover_text(inst.description()).clicked() {
                            self.add_track(inst);
                        }
                    }
                });
            });
        });
    }
}

/// Fingerprint of the packs directory (names + sizes + mtimes) used by the
/// 2-second auto-refresh poll. Cheap: only directory metadata is read.
fn pack_fingerprint() -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mix = |v: u64, h: &mut u64| {
        *h ^= v;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    if let Ok(rd) = std::fs::read_dir(crate::packs::user_packs_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            for b in p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned().into_bytes())
                .unwrap_or_default()
            {
                mix(b as u64, &mut h);
            }
            if let Ok(m) = e.metadata() {
                mix(m.len(), &mut h);
                if let Ok(t) = m.modified() {
                    mix(
                        t.duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        &mut h,
                    );
                }
            }
        }
    }
    h
}

impl eframe::App for CadenceApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }
}
