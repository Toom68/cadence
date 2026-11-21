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
    Color32, Context, CursorIcon, Event, Frame, Key, KeyboardShortcut, Margin, Modifiers, RichText,
    Stroke, StrokeKind, SurrenderFocusOn, ViewportCommand,
};

use crate::audio::AudioEngine;
use crate::model::{AudioClip, Clip, Instrument, Project, Track};

pub use history::History;

use dialogs::Dialog;
use files::{Document, ExportJob, PendingAction};
use theme::{Icon, Palette, ThemeChoice};
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
pub(crate) const SC_COPY: KeyboardShortcut = sc(Modifiers::COMMAND, Key::C);
pub(crate) const SC_PASTE: KeyboardShortcut = sc(Modifiers::COMMAND, Key::V);
pub(crate) const SC_CUT: KeyboardShortcut = sc(Modifiers::COMMAND, Key::X);
pub(crate) const SC_SELECT_ALL: KeyboardShortcut = sc(Modifiers::COMMAND, Key::A);
pub(crate) const SC_EDITOR: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Num1);
pub(crate) const SC_MIXER: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Num2);
pub(crate) const SC_KEYS: KeyboardShortcut = sc(Modifiers::COMMAND, Key::Num3);
pub(crate) const SC_HELP: KeyboardShortcut = sc(Modifiers::NONE, Key::F1);
/// MIDI record toggle — the performance keys (Z X C V B N M, S D G H J) all
/// sit in the rows below R, so plain R is free.
pub(crate) const SC_RECORD: KeyboardShortcut = sc(Modifiers::NONE, Key::R);

/// How long after a clipboard paste the matching V-key release is ignored.
/// egui-winit turns ⌘V into [`Event::Paste`] on key-down and drops the key
/// press; the release still arrives as a key event and would otherwise look
/// like a second paste request when the OS clipboard was empty.
const PASTE_RELEASE_GUARD: Duration = Duration::from_millis(1000);

/// Removes every input event matching `pred`; returns whether any matched.
fn take_events(ctx: &Context, pred: impl Fn(&Event) -> bool) -> bool {
    ctx.input_mut(|i| {
        let hit = i.events.iter().any(&pred);
        if hit {
            i.events.retain(|e| !pred(e));
        }
        hit
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Editor,
    Mixer,
    Keys,
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

/// The app-level clipboard behind ⌘C/⌘V/⌘X. One clipboard for the whole app:
/// the most recent copy wins, and paste always targets the playhead.
/// Snapshots are taken at copy time, so later edits don't rewrite them.
#[derive(Debug, Clone, Default)]
enum Clipboard {
    /// Nothing copied yet.
    #[default]
    None,
    /// Pattern notes with positions relative to the selection's left edge —
    /// pastes into the selected track's current pattern at the playhead.
    /// Works identically for the drum grid (drum hits are notes).
    Notes(Vec<ops::NoteSnap>),
    /// Arranger clips as `(track_id, clip)` snapshots on their original
    /// tracks, shifted so the group's left edge lands on the playhead.
    /// The two vecs are pattern clips and imported-WAV clips — a selection
    /// may mix both kinds (they share the clip id space).
    Clips(Vec<(u64, Clip)>, Vec<(u64, AudioClip)>),
    /// A whole track; pastes as a fresh deep copy appended at the end.
    Track(Box<Track>),
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
    clipboard: Clipboard,
    /// Set when a paste fired on key-down; the paired V-release is ignored
    /// for [`PASTE_RELEASE_GUARD`] so it can't paste a second time.
    paste_release_guard: Option<Instant>,
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
    /// Installed SoundFont catalog for the sound picker; the parsed fonts are
    /// also pushed to the audio engine so `Track::font` references play.
    fonts: crate::font::FontLibrary,
    /// Fingerprint of the fonts dir at the last catalog refresh.
    font_stamp: u64,
    window_title: String,
    style_applied: bool,
    /// Selected UI theme (appearance only — never part of the song).
    theme: ThemeChoice,
}

impl CadenceApp {
    /// Creates the app: demo song on first launch, the launch file if one was
    /// given, and a recovery offer when a valid autosave exists.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let theme = ThemeChoice::from_storage(cc.storage);
        theme::apply(&cc.egui_ctx, theme);
        let (engine, audio_error) = match AudioEngine::new() {
            Ok(engine) => (Some(engine), None),
            Err(err) => (None, Some(err)),
        };
        let mut app = Self::build(Project::demo(), engine, true);
        app.theme = theme;
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
        // The stamp is taken before parsing so a font dropped in mid-scan is
        // still picked up by the next dir poll instead of being fingerprinted
        // but never loaded.
        let font_stamp = if io_enabled { font_fingerprint() } else { 0 };
        let fonts = if io_enabled {
            crate::font::load_library()
        } else {
            crate::font::FontLibrary::empty()
        };
        if let Some(engine) = &engine {
            engine.set_project(&project);
            engine.set_fonts(std::sync::Arc::new(fonts.clone()));
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
            clipboard: Clipboard::default(),
            paste_release_guard: None,
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
            fonts,
            font_stamp,
            window_title: String::new(),
            style_applied: false,
            theme: ThemeChoice::default(),
        };
        app.fix_selection();
        app
    }

    /// Headless constructor for tests: no audio device, no file I/O.
    #[cfg(test)]
    pub(crate) fn headless(project: Project) -> Self {
        Self::build(project, None, false)
    }

    /// The color palette of the active theme, resolved fresh each call.
    pub(super) fn palette(&self) -> Palette {
        self.theme.palette()
    }

    /// Switches the UI theme and re-applies the egui styles immediately.
    /// Appearance only: the project and its dirty flag are untouched.
    pub(super) fn set_theme(&mut self, ctx: &Context, choice: ThemeChoice) {
        if self.theme == choice {
            return;
        }
        self.theme = choice;
        theme::apply(ctx, choice);
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
        // Held recorded notes reference the old song's patterns — dropping
        // them here keeps a later key release from editing the new song.
        self.editor.record_pending.clear();
        self.editor.kb_record_pitch = None;
        // The loop region is session state for this song, not part of it.
        self.transport.loop_region = None;
        self.project = project;
        self.history.reset();
        self.doc.path = path;
        self.doc.saved_state = clean.then(|| self.history.state());
        self.autosaved_state = self.history.state();
        self.sel = Selection::default();
        // Clip snapshots name tracks of the old document; notes and tracks
        // would paste fine, but a per-song clipboard is the least surprising.
        self.clipboard = Clipboard::None;
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
            .flat_map(|t| {
                t.clips
                    .iter()
                    .map(|c| c.id)
                    .chain(t.audio_clips.iter().map(|c| c.id))
            })
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
            .find(|t| {
                t.clips.iter().any(|c| c.id == clip_id)
                    || t.audio_clips.iter().any(|c| c.id == clip_id)
            })
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

    fn selected_clip_snapshots(&self) -> Vec<(u64, Clip)> {
        ops::collect_clips(&self.project, &self.selected_clip_ids())
    }

    /// The imported-WAV half of [`Self::selected_clip_snapshots`] — the two
    /// lists partition the shared clip-id selection.
    fn selected_audio_clip_snapshots(&self) -> Vec<(u64, AudioClip)> {
        ops::collect_audio_clips(&self.project, &self.selected_clip_ids())
    }

    fn select_all_arranger_clips(&mut self) {
        let ids: BTreeSet<u64> = self
            .project
            .tracks
            .iter()
            .flat_map(|t| {
                t.clips
                    .iter()
                    .map(|c| c.id)
                    .chain(t.audio_clips.iter().map(|c| c.id))
            })
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
            let aclips = self.selected_audio_clip_snapshots();
            let n = clips.len() + aclips.len();
            if n > 0
                && self.edit(
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
                        for (track, clip) in &aclips {
                            ops::delete_audio_clip(p, *track, clip.id);
                        }
                    },
                )
            {
                self.clear_arranger_clip_selection();
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
            let aclips = self.selected_audio_clip_snapshots();
            if !clips.is_empty() || !aclips.is_empty() {
                let primary = self.sel.clip;
                let mut new = None;
                self.edit(
                    if clips.len() + aclips.len() == 1 {
                        "Duplicate clip"
                    } else {
                        "Duplicate clips"
                    },
                    None,
                    |p| new = ops::duplicate_clip_groups_after(p, &clips, &aclips),
                );
                match new {
                    Some(ids) => {
                        let set: BTreeSet<u64> = ids.iter().copied().collect();
                        // `copy_mixed_clip_group` returns pattern ids first,
                        // then audio — index the matching snapshot list.
                        let primary = primary.and_then(|old| {
                            clips
                                .iter()
                                .position(|(_, c)| c.id == old)
                                .or_else(|| {
                                    aclips
                                        .iter()
                                        .position(|(_, c)| c.id == old)
                                        .map(|i| i + clips.len())
                                })
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
            let aclips = self.selected_audio_clip_snapshots();
            if !clips.is_empty() || !aclips.is_empty() {
                let grid = self.arranger.snap.steps();
                let dstep =
                    ops::mixed_group_delta(&self.project, &clips, &aclips, steps as f64 * grid);
                self.edit("Move clips", Some("nudge-clips"), |p| {
                    ops::move_clip_group(p, &clips, dstep);
                    ops::move_audio_clip_group(p, &aclips, dstep);
                });
            }
        }
    }

    // ----- clipboard (copy / paste / cut) ---------------------------------

    /// Whether ⌘C has anything to capture: selected clips or notes, else the
    /// selected track (always present while the song has tracks).
    fn can_copy(&self) -> bool {
        !self.selected_clip_ids().is_empty()
            || !self.sel.notes.is_empty()
            || self.sel.track.is_some()
    }

    /// Clipboard snapshots of the selected notes in the selected track's
    /// current pattern — `(track_id, pattern_id, snaps)`. `None` when the
    /// note selection is empty or doesn't resolve to real notes.
    fn note_clipboard_target(&self) -> Option<(u64, u64, Vec<ops::NoteSnap>)> {
        let track = self.selected_track()?;
        let pid = self.current_pattern_id(track)?;
        let pat = ops::pattern(track, pid)?;
        let snaps = ops::note_snaps(pat, &self.sel.notes);
        (!snaps.is_empty()).then_some((track.id, pid, snaps))
    }

    /// Snapshots `snaps` onto the clipboard with the usual status message.
    fn clipboard_notes(&mut self, snaps: Vec<ops::NoteSnap>) {
        let n = snaps.len();
        self.clipboard = Clipboard::Notes(snaps);
        self.set_status(
            StatusKind::Info,
            format!("Copied {n} note{}.", sidebar::plural(n)),
        );
    }

    /// ⌘C: snapshot the current selection. Like [`Self::delete_selection`],
    /// the focused domain wins — notes while the editor is focused, else
    /// clips — so a stale clip selection can't hijack a note copy (editor
    /// note selection never clears `sel.clips`). The project is untouched;
    /// the clipboard holds data, not references into it.
    fn copy_selection(&mut self) {
        if self.focus == Focus::Editor {
            if let Some((_, _, snaps)) = self.note_clipboard_target() {
                self.clipboard_notes(snaps);
                return;
            }
        }
        let clips = self.selected_clip_snapshots();
        let aclips = self.selected_audio_clip_snapshots();
        if !clips.is_empty() || !aclips.is_empty() {
            let n = clips.len() + aclips.len();
            self.clipboard = Clipboard::Clips(clips, aclips);
            self.set_status(
                StatusKind::Info,
                format!("Copied {n} clip{}.", sidebar::plural(n)),
            );
            return;
        }
        if self.focus != Focus::Editor {
            if let Some((_, _, snaps)) = self.note_clipboard_target() {
                self.clipboard_notes(snaps);
                return;
            }
        }
        if let Some(track) = self
            .sel
            .track
            .and_then(|id| ops::track(&self.project, id))
            .cloned()
        {
            let name = track.name.clone();
            self.clipboard = Clipboard::Track(Box::new(track));
            self.set_status(StatusKind::Info, format!("Copied track “{name}”."));
            return;
        }
        self.set_status(
            StatusKind::Info,
            "Nothing to copy — select notes, clips or a track first.",
        );
    }

    /// ⌘V: paste whatever the clipboard holds. Notes land at the pointer
    /// when it's over the editor grid, everything else at the playhead.
    fn paste_clipboard(&mut self, ctx: &Context) {
        // Clone so the clipboard survives the paste (repeat ⌘V stacks copies).
        match self.clipboard.clone() {
            Clipboard::None => self.set_status(
                StatusKind::Info,
                "Nothing to paste — copy notes, clips or a track first.",
            ),
            Clipboard::Notes(snaps) => self.paste_notes(&snaps, ctx),
            Clipboard::Clips(originals, audio) => self.paste_clips(&originals, &audio),
            Clipboard::Track(track) => self.paste_track(&track),
        }
    }

    /// Pastes note snapshots into the selected track's current pattern — at
    /// the pointer when it's over the editor grid, else at the playhead
    /// (patterns loop, so the origin wraps with `rem_euclid`, see
    /// [`ops::paste_notes`]). New notes merge with the existing ones and
    /// become the selection.
    fn paste_notes(&mut self, snaps: &[ops::NoteSnap], ctx: &Context) {
        let target = self.sel.track.and_then(|tid| {
            let track = ops::track(&self.project, tid)?;
            Some((tid, self.current_pattern_id(track)?))
        });
        let Some((track_id, pid)) = target else {
            self.set_status(
                StatusKind::Warning,
                "Select a track with a pattern to paste notes into.",
            );
            return;
        };
        let pname = ops::track(&self.project, track_id)
            .and_then(|t| ops::pattern(t, pid))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let (step, pitch_anchor) = self
            .editor_paste_target(ctx)
            .unwrap_or((self.transport.position, None));
        // Pitch anchor: shift the whole selection so the note nearest the
        // cursor's row lands on it — paste anywhere, any octave. The shift is
        // clamped so no note leaves the MIDI range.
        let shifted = pitch_anchor.and_then(|anchor| {
            let near = snaps
                .iter()
                .map(|s| s.pitch)
                .min_by_key(|&p| (i32::from(p) - i32::from(anchor)).abs())?;
            let min_p = snaps.iter().map(|s| i32::from(s.pitch)).min()?;
            let max_p = snaps.iter().map(|s| i32::from(s.pitch)).max()?;
            let shift = (i32::from(anchor) - i32::from(near))
                .clamp(-min_p, i32::from(ops::MAX_PITCH) - max_p);
            (shift != 0).then(|| {
                snaps
                    .iter()
                    .map(|s| ops::NoteSnap {
                        pitch: (i32::from(s.pitch) + shift) as u8,
                        ..*s
                    })
                    .collect::<Vec<_>>()
            })
        });
        let snaps: &[ops::NoteSnap] = shifted.as_deref().unwrap_or(snaps);
        let mut new = Vec::new();
        self.edit("Paste notes", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                new = ops::paste_notes(pat, snaps, step);
            }
        });
        if new.is_empty() {
            self.set_status(
                StatusKind::Warning,
                format!(
                    "Nothing pasted — \"{pname}\" is at the {}-note limit.",
                    ops::MAX_NOTES_PER_PATTERN
                ),
            );
            return;
        }
        let n = new.len();
        self.sel.notes = new.into_iter().collect();
        self.focus = Focus::Editor;
        self.set_status(
            StatusKind::Info,
            format!("Pasted {n} note{}.", sidebar::plural(n)),
        );
    }

    /// Pastes clip snapshots back onto their original tracks, shifted so the
    /// group's left edge lands on the playhead (`copy_mixed_clip_group`
    /// clamps the move inside the song like every other clip edit).
    /// `audio` holds the imported-WAV snapshots; the new clips become the
    /// selection.
    fn paste_clips(&mut self, originals: &[(u64, Clip)], audio: &[(u64, AudioClip)]) {
        if originals.is_empty() && audio.is_empty() {
            return;
        }
        // `copy_mixed_clip_group` checks clip limits atomically but pushes
        // clips track by track — a track deleted since the copy would
        // otherwise leave a partial paste behind. Refuse up front instead.
        if originals
            .iter()
            .map(|(track_id, _)| *track_id)
            .chain(audio.iter().map(|(track_id, _)| *track_id))
            .any(|track_id| ops::track(&self.project, track_id).is_none())
        {
            self.set_status(
                StatusKind::Warning,
                "Could not paste clips — a track they were copied from is gone.",
            );
            return;
        }
        let min_start = originals
            .iter()
            .map(|(_, c)| c.start)
            .chain(audio.iter().map(|(_, c)| c.start))
            .fold(f64::INFINITY, f64::min);
        let dstep = self.transport.position - min_start;
        let mut new = None;
        self.edit("Paste clips", None, |p| {
            new = ops::copy_mixed_clip_group(p, originals, audio, dstep);
        });
        match new {
            Some(ids) => {
                let primary = ids.first().copied();
                let set: BTreeSet<u64> = ids.into_iter().collect();
                self.set_clip_selection(set, primary);
            }
            None => self.set_status(
                StatusKind::Warning,
                format!(
                    "Could not paste clips (tracks hold up to {} clips, or a copied track is gone).",
                    ops::MAX_CLIPS_PER_TRACK
                ),
            ),
        }
    }

    /// Pastes a copied track as a new independent track appended at the end
    /// of the list (fresh ids everywhere — see [`ops::paste_track`]).
    fn paste_track(&mut self, source: &Track) {
        let mut new = None;
        self.edit("Paste track", None, |p| new = ops::paste_track(p, source));
        match new {
            Some(id) => {
                let name = ops::track(&self.project, id)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                self.select_track(id);
                self.arranger.reveal_track = Some(id);
                self.set_status(StatusKind::Success, format!("Pasted track “{name}”."));
            }
            None => self.set_status(
                StatusKind::Warning,
                format!("Track limit reached ({} tracks).", ops::MAX_TRACKS),
            ),
        }
    }

    /// ⌘X: snapshot the selection into the clipboard, then delete it — the
    /// whole cut is a single undo step. Same precedence as [`Self::copy_selection`].
    fn cut_selection(&mut self) {
        if self.focus == Focus::Editor {
            if let Some((track_id, pid, snaps)) = self.note_clipboard_target() {
                self.cut_notes(track_id, pid, snaps);
                return;
            }
        }
        let clips = self.selected_clip_snapshots();
        let aclips = self.selected_audio_clip_snapshots();
        if !clips.is_empty() || !aclips.is_empty() {
            let n = clips.len() + aclips.len();
            if self.edit(if n == 1 { "Cut clip" } else { "Cut clips" }, None, |p| {
                for (track, clip) in &clips {
                    ops::delete_clip(p, *track, clip.id);
                }
                for (track, clip) in &aclips {
                    ops::delete_audio_clip(p, *track, clip.id);
                }
            }) {
                self.clipboard = Clipboard::Clips(clips, aclips);
                self.clear_arranger_clip_selection();
                self.set_status(
                    StatusKind::Info,
                    format!("Cut {n} clip{}.", sidebar::plural(n)),
                );
            }
            return;
        }
        if self.focus != Focus::Editor {
            if let Some((track_id, pid, snaps)) = self.note_clipboard_target() {
                self.cut_notes(track_id, pid, snaps);
                return;
            }
        }
        if let Some(track) = self
            .sel
            .track
            .and_then(|id| ops::track(&self.project, id))
            .cloned()
        {
            let name = track.name.clone();
            let tid = track.id;
            if self.edit("Cut track", None, |p| {
                ops::delete_track(p, tid);
            }) {
                self.clipboard = Clipboard::Track(Box::new(track));
                self.fix_selection();
                self.set_status(StatusKind::Info, format!("Cut track “{name}”."));
            }
            return;
        }
        self.set_status(
            StatusKind::Info,
            "Nothing to cut — select notes, clips or a track first.",
        );
    }

    /// Cut the notes in `snaps`' pattern — snapshot to clipboard + delete in
    /// one undo step.
    fn cut_notes(&mut self, track_id: u64, pid: u64, snaps: Vec<ops::NoteSnap>) {
        let ids: Vec<u64> = self.sel.notes.iter().copied().collect();
        let n = ids.len();
        if self.edit("Cut notes", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pid) {
                ops::delete_notes(pat, &ids);
            }
        }) {
            self.clipboard = Clipboard::Notes(snaps);
            self.sel.notes.clear();
            self.set_status(
                StatusKind::Info,
                format!("Cut {n} note{}.", sidebar::plural(n)),
            );
        }
    }

    fn add_track(&mut self, instrument: Instrument) {
        let mut id = None;
        self.edit(&format!("Add {} track", instrument.name()), None, |p| {
            id = ops::add_track(p, instrument)
        });
        self.finish_track_add(id, format!("Added a {} track", instrument.name()));
    }

    /// Edit ▸ Add Track ▸ Audio Track: an empty lane that imported-WAV
    /// clips can live on (File ▸ Import Audio File… creates one on demand,
    /// too).
    fn add_audio_track(&mut self) {
        let mut id = None;
        self.edit("Add audio track", None, |p| id = ops::add_audio_track(p));
        self.finish_track_add(id, "Added an audio track".to_string());
        // `finish_track_add` aims focus at the note editor, which an audio
        // lane has no use for — the arranger is its editing surface.
        self.focus = Focus::Arranger;
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

    /// Re-reads the SoundFont catalog and pushes it to the audio engine. The
    /// 2s dir poll calls this; parsing `.sf2` files is the expensive part, so
    /// it only runs when the fonts folder fingerprint actually changed.
    fn reload_fonts(&mut self) {
        if !self.io_enabled {
            return;
        }
        let stamp = font_fingerprint();
        if stamp == self.font_stamp {
            return;
        }
        self.font_stamp = stamp;
        self.fonts = crate::font::load_library();
        if let Some(engine) = &self.engine {
            engine.set_fonts(std::sync::Arc::new(self.fonts.clone()));
        }
        if !self.fonts.errors.is_empty() {
            self.set_status(
                StatusKind::Warning,
                format!(
                    "{} SoundFont file{} failed to load; the track sound picker lists the problem.",
                    self.fonts.errors.len(),
                    sidebar::plural(self.fonts.errors.len())
                ),
            );
        }
    }

    // ----- frame -----------------------------------------------------------

    /// Runs one UI frame. Separated from `eframe::App::update` so tests can
    /// drive the full interface with a bare `egui::Context`.
    fn frame(&mut self, ctx: &Context) {
        if !self.style_applied {
            theme::apply(ctx, self.theme);
            // Keyboard shortcuts are gated on `wants_keyboard_input()`, which
            // is true while any widget holds focus (a TextEdit or DragValue).
            // The default `Clicks` mode only surrenders that focus on a full
            // click, so drag gestures — marquee-selecting notes, scrubbing the
            // playhead — would leave a stale text field focused and silently
            // kill every editing shortcut. `Presses` hands focus back as soon
            // as the pointer goes down anywhere outside the focused widget.
            ctx.memory_mut(|m| {
                m.options.input_options.surrender_focus_on = SurrenderFocusOn::Presses;
            });
            self.style_applied = true;
        }
        if let Some(engine) = &self.engine {
            engine.wake_on_device_change(|| {
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            });
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
        self.engine_dirty = self
            .engine
            .as_ref()
            .is_some_and(|engine| !engine.try_set_project(&self.project));
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
        } else if self.transport.peak.iter().any(|p| *p > 0.001)
            || (self.engine_dirty && self.engine.as_ref().is_some_and(AudioEngine::is_connected))
        {
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
        if pressed(&SC_KEYS) {
            self.tab = Tab::Keys;
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
        // Clipboard commands arrive as dedicated egui events, not key presses:
        // egui-winit translates ⌘C/⌘V/⌘X into Event::Copy/Paste/Cut on
        // key-down and drops the Event::Key entirely — and emits *no* event
        // for ⌘V when the OS clipboard has no text. `consume_shortcut` can
        // therefore never see them on desktop, so handle both forms. While a
        // text field is focused we returned above, leaving these for it.
        if take_events(ctx, |e| matches!(e, Event::Copy)) || pressed(&SC_COPY) {
            self.copy_selection();
        }
        if take_events(ctx, |e| matches!(e, Event::Cut)) || pressed(&SC_CUT) {
            self.cut_selection();
        }
        if take_events(ctx, |e| matches!(e, Event::Paste(_))) || pressed(&SC_PASTE) {
            // Arm the guard so the V-release can't paste a second time.
            self.paste_release_guard = Some(Instant::now());
            self.paste_clipboard(ctx);
        } else if take_events(ctx, |e| {
            matches!(
                e,
                Event::Key {
                    key: Key::V,
                    pressed: false,
                    modifiers,
                    ..
                } if modifiers.command
            )
        }) {
            // No Paste event fired (empty/non-text OS clipboard), so this
            // release is our only signal — unless it pairs with a paste we
            // already did on the press.
            match self.paste_release_guard {
                Some(t) if t.elapsed() < PASTE_RELEASE_GUARD => {
                    self.paste_release_guard = None;
                }
                _ => {
                    self.paste_release_guard = None;
                    self.paste_clipboard(ctx);
                }
            }
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
        if key(SC_RECORD.modifiers, SC_RECORD.logical_key) {
            self.toggle_record();
        }
        if key(Modifiers::NONE, Key::Escape) {
            self.clear_selection();
        }
        if key(Modifiers::NONE, Key::Delete) || key(Modifiers::NONE, Key::Backspace) {
            self.delete_selection();
        }
        // B splits the clip under the pointer / at the playhead — but only
        // while the arranger owns keyboard edits; with the editor focused it
        // stays a note-audition key for `keyboard_play` (which runs below).
        if self.focus == Focus::Arranger && key(Modifiers::NONE, Key::B) {
            self.split_clip_at_playhead(ctx);
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
        // Computer-keyboard note audition: letter keys play the selected
        // track's sounds while the pattern editor is showing. Reached only
        // when not typing and no dialog is open (both return earlier); it
        // also no-ops off the Editor tab.
        self.keyboard_play(ctx);
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
        let pal = self.palette();
        let frame = Frame::new()
            .fill(pal.bg)
            .inner_margin(Margin::symmetric(8, 3));
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
                        if ui.add(item("New Song…", Some(&SC_NEW))).clicked() {
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
                        if ui.add(item("Import Sample…", None)).clicked() {
                            self.import_sample_dialog();
                        }
                        if ui.add(item("Import Audio File…", None)).clicked() {
                            self.import_audio_clip_dialog();
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
                                        "Exporting…"
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
                        if ui
                            .add_enabled(
                                !exporting,
                                item(
                                    if exporting {
                                        "Exporting…"
                                    } else {
                                        "Export Stems…"
                                    },
                                    None,
                                ),
                            )
                            .clicked()
                        {
                            self.export_stems_dialog();
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
                        // While the menu is open the pointer is on the menu,
                        // not the grid, so the enabled state reflects the
                        // playhead/selection fallback.
                        if ui
                            .add_enabled(
                                self.split_clip_target(ui.ctx()).is_some(),
                                item("Split Clip at Playhead", None).shortcut_text("B"),
                            )
                            .on_disabled_hover_text(
                                "Hover a clip, or select one the playhead crosses",
                            )
                            .clicked()
                        {
                            self.split_clip_at_playhead(ui.ctx());
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
                        if ui
                            .add_enabled(self.can_copy(), item("Cut", Some(&SC_CUT)))
                            .clicked()
                        {
                            self.cut_selection();
                        }
                        if ui
                            .add_enabled(self.can_copy(), item("Copy", Some(&SC_COPY)))
                            .clicked()
                        {
                            self.copy_selection();
                        }
                        if ui
                            .add_enabled(
                                !matches!(self.clipboard, Clipboard::None),
                                item("Paste", Some(&SC_PASTE)),
                            )
                            .on_disabled_hover_text("Copy notes, clips or a track first")
                            .clicked()
                        {
                            self.paste_clipboard(ctx);
                        }
                        if ui
                            .add_enabled(self.can_arpeggiate(), item("Arpeggiator…", None))
                            .on_disabled_hover_text(
                                "Select a melodic track with a pattern to arpeggiate",
                            )
                            .clicked()
                        {
                            self.open_arpeggiator();
                        }
                        if ui
                            .add_enabled(self.has_note_targets(), item("Humanize Notes", None))
                            .on_disabled_hover_text("Select notes or open a pattern that has notes")
                            .clicked()
                        {
                            if let Some((track_id, pid)) = self.editor_target() {
                                self.humanize_notes(track_id, pid);
                            }
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
                            ui.separator();
                            if ui
                                .add_enabled(
                                    self.project.tracks.len() < ops::MAX_TRACKS,
                                    egui::Button::new("Audio Track"),
                                )
                                .on_hover_text("A lane for imported WAV clips")
                                .clicked()
                            {
                                self.add_audio_track();
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
                        let mut recording = self.transport.recording;
                        if ui.checkbox(&mut recording, "Record  (R)").changed() {
                            self.toggle_record();
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
                        if ui.add(item("Keyboard", Some(&SC_KEYS))).clicked() {
                            self.tab = Tab::Keys;
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
                        ui.separator();
                        ui.menu_button("Theme", |ui| {
                            for choice in ThemeChoice::ALL {
                                let on = self.theme == choice;
                                let text = if on {
                                    RichText::new(choice.label()).color(pal.text).strong()
                                } else {
                                    RichText::new(choice.label())
                                };
                                if ui
                                    .selectable_label(on, text)
                                    .on_hover_text(choice.description())
                                    .clicked()
                                {
                                    self.set_theme(ui.ctx(), choice);
                                    ui.close();
                                }
                            }
                        });
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
                                theme::icon_button(
                                    ui,
                                    pal,
                                    Icon::Redo,
                                    24.0,
                                    false,
                                    pal.accent,
                                    &redo_tip,
                                )
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
                                theme::icon_button(
                                    ui,
                                    pal,
                                    Icon::Undo,
                                    24.0,
                                    false,
                                    pal.accent,
                                    &undo_tip,
                                )
                            })
                            .inner
                            .clicked()
                        {
                            self.undo();
                        }
                        ui.add_space(8.0);
                        let (text, color) = if self.dirty() {
                            ("Unsaved changes", pal.warn)
                        } else {
                            ("All changes saved", pal.text_faint)
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
        let pal = self.palette();
        let frame = Frame::new()
            .fill(pal.bg)
            .inner_margin(Margin::symmetric(12, 4))
            .stroke(Stroke::new(1.0_f32, pal.border_soft));
        egui::TopBottomPanel::bottom("status_bar")
            .exact_height(28.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(status) = &self.status {
                        let color = match status.kind {
                            StatusKind::Info => pal.text_dim,
                            StatusKind::Success => pal.accent,
                            StatusKind::Warning => pal.warn,
                            StatusKind::Error => pal.danger,
                        };
                        theme::color_dot(ui, color, 7.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&status.text).small().color(pal.text_dim),
                            )
                            .truncate(),
                        );
                    } else {
                        ui.label(
                            RichText::new("Ready · Press F1 for shortcuts")
                                .small()
                                .color(pal.text_faint),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match (&self.engine, &self.audio_error) {
                            (Some(_), _) => {
                                let err = self.transport.engine_error.clone();
                                let (c, t) = match err {
                                    Some(e) => (pal.danger, format!("Audio error: {e}")),
                                    None => (
                                        pal.accent,
                                        format!("Audio ready · {} voices", self.transport.voices),
                                    ),
                                };
                                ui.label(RichText::new(t).small().color(pal.text_dim));
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
                                    .color(pal.text_dim),
                                );
                                theme::color_dot(ui, pal.danger, 7.0);
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
                            .color(pal.text_faint),
                        );
                        if let Some(job) = &self.export {
                            ui.add_space(14.0);
                            let p = job.progress();
                            ui.add(
                                egui::ProgressBar::new(p)
                                    .desired_width(140.0)
                                    .desired_height(10.0)
                                    .fill(pal.accent),
                            );
                            ui.label(
                                RichText::new(format!("{} {:.0}%", job.verb(), p * 100.0))
                                    .small()
                                    .color(pal.accent),
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
                engine.set_fonts(std::sync::Arc::new(self.fonts.clone()));
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
        let pal = self.palette();
        let frame = Frame::new().fill(pal.bg).inner_margin(Margin {
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
                    theme::card(pal)
                        .inner_margin(Margin::same(0))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            self.arranger_ui(ui);
                        });
                });
            theme::card(pal)
                .inner_margin(Margin::same(0))
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.bottom_tabs(ui);
                    match self.tab {
                        Tab::Editor => self.editor_ui(ui),
                        Tab::Mixer => self.mixer_ui(ui),
                        Tab::Keys => self.keys_ui(ui),
                    }
                });
        });
    }

    fn bottom_tabs(&mut self, ui: &mut egui::Ui) {
        let pal = self.palette();
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
                        (Tab::Keys, "Keyboard", &SC_KEYS),
                    ] {
                        let on = self.tab == tab;
                        let text =
                            RichText::new(label).color(if on { pal.text } else { pal.text_dim });
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
                            b.fill(pal.control).stroke(Stroke::new(1.0_f32, pal.border))
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
            .hline(r.x_range(), r.top(), Stroke::new(1.0_f32, pal.border));
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let pal = self.palette();
        let card = theme::card(pal).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.25).max(20.0));
                ui.label(theme::kicker_text(pal, "EMPTY SONG"));
                ui.label(RichText::new("Start by adding an instrument").size(22.0).color(pal.text));
                ui.add_space(4.0);
                ui.label(RichText::new("Each track gets its own patterns, clips and mixer channel. You can add up to 32 tracks.").color(pal.text_dim));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    let count = Instrument::ALL.len() as f32;
                    let w = count * 128.0 + (count - 1.0) * 8.0;
                    ui.add_space(((ui.available_width() - w) * 0.5).max(0.0));
                    for inst in Instrument::ALL {
                        let color = theme::rgb(sidebar::instrument_color(inst));
                        let b = egui::Button::new(RichText::new(format!("+  {}", inst.name())).color(theme::ink_on(color)).strong()).fill(color).min_size(egui::vec2(128.0, 40.0));
                        if ui.add(b).on_hover_text(inst.description()).clicked() {
                            self.add_track(inst);
                        }
                    }
                });
            });
        });
        // A dragged library sound can start the song too: the arranger isn't
        // shown while there are no tracks, so the empty canvas takes the
        // drop and adds the first track.
        if card
            .response
            .dnd_hover_payload::<sidebar::QuickSound>()
            .is_some()
        {
            ui.painter().rect_stroke(
                card.response.rect.shrink(1.0),
                10.0,
                Stroke::new(1.5_f32, pal.accent),
                StrokeKind::Inside,
            );
            ui.ctx().set_cursor_icon(CursorIcon::Copy);
        }
        if let Some(sound) = card.response.dnd_release_payload::<sidebar::QuickSound>() {
            self.add_sound_track_at(&sound, 0);
        }
    }
}

/// Fingerprint of the files with extension `ext` in `dir` (names + sizes +
/// mtimes), used by the periodic rescan polls. Cheap: only directory
/// metadata is read.
fn dir_fingerprint(dir: &std::path::Path, ext: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mix = |v: u64, h: &mut u64| {
        *h ^= v;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x.eq_ignore_ascii_case(ext))
            {
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

/// Fingerprint of the user pack folder (see [`dir_fingerprint`]).
fn pack_fingerprint() -> u64 {
    dir_fingerprint(&crate::packs::user_packs_dir(), "json")
}

/// Fingerprint of the user SoundFont folder (see [`dir_fingerprint`]).
fn font_fingerprint() -> u64 {
    dir_fingerprint(&crate::font::user_fonts_dir(), "sf2")
}

impl eframe::App for CadenceApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.frame(ctx);
    }

    /// Persists only the UI theme — song state is saved through `.cadence`
    /// files, never through eframe storage.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(theme::STORAGE_KEY, self.theme.id().to_owned());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette().bg.to_normalized_gamma_f32()
    }
}
