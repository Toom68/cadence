//! Modal dialogs: the unsaved-changes guard, New Song (name + key), the
//! Arpeggiator, Help, About, the launch-time recovery offer and the
//! instrument-pack error list.
//!
//! `mod.rs` owns *when* a dialog opens (`self.dialog = Dialog::…`) and calls
//! [`CadenceApp::dialogs`] once per frame plus [`CadenceApp::dialog_escape`]
//! on Escape. Each dialog is an [`egui::Modal`]: centered and backdrop-blocking.
//! Most dialogs dismiss on Escape or a backdrop click; the recovery offer is
//! the exception — it only closes on an explicit Restore or Discard so the
//! autosave on disk can never be lost to an accidental dismissal.

use std::sync::{Arc, Mutex};

use egui::{
    vec2, Align, Color32, Context, Frame, Key, KeyboardShortcut, Layout, Margin, Modifiers,
    RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui,
};

use super::files::PendingAction;
use super::ops::{self, ArpDirection, ArpSpec};
use super::theme::{self, Palette};
use super::transport::paint_brand_mark;
use super::{CadenceApp, StatusKind};
use crate::model::{ChordQuality, KeyMode, Project, SongKey, MAX_NAME_CHARS};
use crate::persistence;

/// The currently open modal dialog, if any.
#[derive(Debug)]
pub(super) enum Dialog {
    /// Nothing open.
    None,
    /// Shortcuts & editing tips (F1 / Help menu).
    Help,
    /// About Cadence.
    About,
    /// Scrollable third-party license notices (opened from About).
    Notices,
    /// A file action waiting on a save / don't-save / cancel decision.
    Unsaved(PendingAction),
    /// Launch-time offer to restore the crash-recovery autosave.
    Recovery(Box<persistence::RecoveredWork>),
    /// Non-fatal instrument-pack load problems.
    PackErrors(Vec<String>),
    /// Name + key for a new song (after any unsaved-changes decision).
    NewSong(NewSongDraft),
    /// Chord arpeggio generator for the selected track's current pattern.
    Arp(ArpDraft),
    /// Update check result, download and install choice (see `updates.rs`).
    Update,
    /// App settings: theme, audio output, automatic update checks.
    Preferences,
    /// Confirm deleting audio-store files nothing known uses.
    CleanAudio(persistence::UnusedAudio),
}

/// Editable state of the New Song dialog.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NewSongDraft {
    pub(super) name: String,
    pub(super) key: SongKey,
}

impl Default for NewSongDraft {
    fn default() -> Self {
        NewSongDraft {
            name: Project::empty().name,
            key: SongKey::default(),
        }
    }
}

/// Rates offered by the arpeggiator: (steps between notes, label).
const ARP_RATES: [(u32, &str); 3] = [
    (1, "Every step (16ths)"),
    (2, "Every 2 steps (8ths)"),
    (4, "Every 4 steps (quarters)"),
];
/// Lowest / highest octave the chord root may start in (C4 = middle C).
const ARP_MIN_OCTAVE: i32 = 0;
const ARP_MAX_OCTAVE: i32 = 7;

/// Editable state of the Arpeggiator dialog.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ArpDraft {
    /// Diatonic scale degree (0 = I) of the song key, or `None` for a custom root.
    pub(super) degree: Option<usize>,
    /// Pitch class used when `degree` is `None`.
    pub(super) custom_root: u8,
    pub(super) quality: ChordQuality,
    /// Octave of the chord root (C4 = MIDI 60).
    pub(super) octave: i32,
    pub(super) octaves: u32,
    pub(super) direction: ArpDirection,
    pub(super) rate_steps: u32,
    pub(super) velocity: f32,
    pub(super) accent: bool,
    pub(super) replace: bool,
}

impl ArpDraft {
    /// Defaults: the key's I chord, rooted in the octave holding the
    /// instrument's comfortable pitch, Up over 2 octaves in eighths at 80%
    /// velocity, accented downbeats, replacing the pattern's notes.
    pub(super) fn new(key: SongKey, default_pitch: u8) -> Self {
        ArpDraft {
            degree: Some(0),
            custom_root: key.root,
            quality: key.degree_chord_quality(0),
            octave: (default_pitch as i32 / 12 - 1).clamp(ARP_MIN_OCTAVE, ARP_MAX_OCTAVE),
            octaves: 2,
            direction: ArpDirection::Up,
            rate_steps: 2,
            velocity: 0.8,
            accent: true,
            replace: true,
        }
    }

    /// Pitch class of the chord root in `key`.
    pub(super) fn root_class(&self, key: SongKey) -> u8 {
        self.degree
            .map_or(self.custom_root % 12, |d| key.degree_root(d))
    }

    pub(super) fn spec(&self, key: SongKey) -> ArpSpec {
        let root = (self.octave + 1) * 12 + self.root_class(key) as i32;
        ArpSpec {
            chord_root: root.clamp(0, ops::MAX_PITCH as i32) as u8,
            quality: self.quality,
            octaves: self.octaves.clamp(1, ops::MAX_ARP_OCTAVES),
            direction: self.direction,
            rate_steps: self.rate_steps.max(1),
            velocity: self.velocity,
            accent: self.accent,
            replace: self.replace,
        }
    }
}

/// "I — C", "vii° — Bdim" or "Custom…" for the chord picker.
fn degree_label(key: SongKey, degree: Option<usize>) -> String {
    match degree {
        Some(d) => format!(
            "{} — {}",
            key.degree_numeral(d),
            key.degree_chord_quality(d)
                .symbol_in(key.degree_root(d), key)
        ),
        None => "Custom…".to_owned(),
    }
}

fn mode_label(mode: KeyMode) -> &'static str {
    match mode {
        KeyMode::Major => "Major",
        KeyMode::Minor => "Minor",
    }
}

/// Root-note chips plus Major/Minor chips. Returns true when `key` changed.
fn key_picker(ui: &mut Ui, pal: Palette, key: &mut SongKey) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for pc in 0..12u8 {
            let on = key.root == pc;
            // Each root spelled the way that key is written in this mode
            // (Bb major, C# minor).
            let name = SongKey::new(pc, key.mode).spell(pc);
            let tip = format!("Key of {name}");
            if theme::chip(ui, pal, name, on, pal.accent, vec2(30.0, 24.0), &tip).clicked() && !on {
                key.root = pc;
                changed = true;
            }
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for mode in KeyMode::ALL {
            let on = key.mode == mode;
            let label = mode_label(mode);
            if theme::chip(ui, pal, label, on, pal.accent, vec2(64.0, 24.0), label).clicked() && !on
            {
                key.mode = mode;
                changed = true;
            }
        }
    });
    changed
}

impl Dialog {
    pub(super) fn is_open(&self) -> bool {
        !matches!(self, Dialog::None)
    }
}

/// Dialog widths: one size per kind so they feel like one family.
pub(super) const W_COMPACT: f32 = 440.0;
const W_FORM: f32 = 500.0;
const W_LIST: f32 = 560.0;
const W_HELP: f32 = 720.0;

impl CadenceApp {
    /// Draws the active modal dialog. Called once per frame from `frame()`;
    /// also drives the pending-quit viewport close and the periodic pack-folder
    /// rescan (both cheap no-ops when idle).
    pub(super) fn dialogs(&mut self, ctx: &Context) {
        self.poll_pack_dir(ctx);
        self.poll_font_dir(ctx);
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if matches!(self.dialog, Dialog::None) {
            return;
        }
        let mut dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let keep = match &mut dialog {
            Dialog::None => false,
            Dialog::Help => self.help_dialog(ctx),
            Dialog::About => self.about_dialog(ctx),
            Dialog::Notices => self.notices_dialog(ctx),
            Dialog::Unsaved(action) => self.unsaved_dialog(ctx, action.clone()),
            Dialog::Recovery(work) => self.recovery_dialog(ctx, work),
            Dialog::PackErrors(errors) => self.pack_errors_dialog(ctx, errors),
            Dialog::NewSong(draft) => self.new_song_dialog(ctx, draft),
            Dialog::Arp(draft) => self.arp_dialog(ctx, draft),
            Dialog::Update => self.update_dialog(ctx),
            Dialog::Preferences => self.preferences_dialog(ctx),
            Dialog::CleanAudio(plan) => self.clean_audio_dialog(ctx, plan),
        };
        if keep {
            self.dialog = dialog;
        }
    }

    /// Escape inside a dialog: cancel the pending action or dismiss the dialog.
    /// The recovery offer is exempt — it only closes on an explicit Restore or
    /// Discard so the autosave on disk can never be lost to a stray keypress.
    /// While `recovery_offered` stays set, `handle_close_request` also preserves
    /// the file, so quitting the app defers the decision to next launch.
    pub(super) fn dialog_escape(&mut self) {
        if matches!(self.dialog, Dialog::Recovery(_)) {
            return;
        }
        self.dialog = Dialog::None;
    }

    // ----- unsaved changes -------------------------------------------------

    /// Save / Don't Save / Cancel before New, Demo, Open, MIDI import or Quit.
    /// A failed or cancelled save keeps the dialog (and the dirty flag) so the
    /// pending action is never lost silently. Enter saves, Escape cancels.
    fn unsaved_dialog(&mut self, ctx: &Context, action: PendingAction) -> bool {
        let pal = self.palette();
        #[derive(Clone, Copy)]
        enum Choice {
            Save,
            Discard,
            Cancel,
        }
        let mut choice = None;
        let name = self.project.name.clone();
        let path = self.doc.path.clone();
        let resp = show_dialog(ctx, "cadence-unsaved", pal, W_COMPACT, |ui| {
            header(ui, pal, "UNSAVED CHANGES", "Save your work first?");
            ui.add_space(theme::GAP_S);
            ui.add(egui::Label::new(theme::strong_text(&name).color(pal.text)).truncate());
            match &path {
                Some(p) => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(p.display().to_string())
                                .small()
                                .color(pal.text_faint),
                        )
                        .truncate(),
                    );
                }
                None => theme::hint(ui, pal, "Not saved to a file yet"),
            }
            ui.add_space(theme::GAP_S);
            let verb = if matches!(action, PendingAction::Quit) {
                "quit Cadence"
            } else {
                "continue"
            };
            ui.label(
                RichText::new(format!(
                    "This song has unsaved changes. Save them before you {verb}?"
                ))
                .color(pal.text_dim),
            );
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Save").clicked() {
                    choice = Some(Choice::Save);
                }
                if theme::secondary_button(ui, "Don't Save")
                    .on_hover_text("Discard the unsaved changes and continue")
                    .clicked()
                {
                    choice = Some(Choice::Discard);
                }
                if theme::secondary_button(ui, "Cancel").clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
        if choice.is_none() && enter_pressed(ctx) {
            choice = Some(Choice::Save);
        }
        let choice = choice.or_else(|| resp.should_close().then_some(Choice::Cancel));
        match choice {
            Some(Choice::Save) => {
                // A cancelled picker or failed write keeps the guard (and the
                // pending action) open so nothing is lost silently.
                if self.save_current() {
                    self.run_pending_action(action);
                    false
                } else {
                    true
                }
            }
            Some(Choice::Discard) => {
                self.run_pending_action(action);
                false
            }
            Some(Choice::Cancel) => false,
            // No decision yet: the guard must stay up on ordinary frames.
            None => true,
        }
    }

    // ----- recovery offer ----------------------------------------------------

    /// Restore / Discard for the launch-time autosave offer.
    ///
    /// This dialog can only be resolved by an explicit choice: Escape, a
    /// backdrop click and ordinary frames all leave it (and
    /// `recovery_offered`) untouched, so the autosave on disk is never lost
    /// to an accidental dismissal. Enter restores — the non-destructive
    /// choice. `handle_close_request` preserves the file while
    /// `recovery_offered` is set, so the user can also simply close the
    /// app and decide next launch.
    fn recovery_dialog(&mut self, ctx: &Context, work: &persistence::RecoveredWork) -> bool {
        let pal = self.palette();
        #[derive(Clone, Copy)]
        enum Choice {
            Restore,
            Discard,
        }
        let mut choice = None;
        let project = &work.project;
        let tracks = project.tracks.len();
        let source = work
            .source
            .as_deref()
            .map_or_else(|| "an untitled song".to_owned(), super::files::file_label);
        let when = work
            .saved_at
            .map(|t| format!("Autosaved {}", time_ago(t)))
            .unwrap_or_else(|| "Autosaved".to_owned());
        let same_song = work.source.is_some() && work.source == self.doc.path;
        let replaces = match (&self.doc.path, same_song) {
            (Some(_), true) => "Newer than the saved file you just opened. Restoring loads it in the editor; the file on disk changes only when you save.".to_owned(),
            (Some(open), false) => format!(
                "Restoring replaces the open song ({}) in the editor; its file stays unchanged.",
                super::files::file_label(open)
            ),
            (None, _) => "Restoring replaces the song currently open.".to_owned(),
        };
        let _resp = show_dialog(ctx, "cadence-recovery", pal, W_COMPACT, |ui| {
            header(ui, pal, "RECOVERY", "Restore unsaved work?");
            ui.add_space(theme::GAP_S);
            ui.label(
                RichText::new("Cadence found unsaved work from a previous session:")
                    .color(pal.text_dim),
            );
            ui.add_space(theme::GAP_S);
            ui.add(egui::Label::new(theme::strong_text(&project.name).color(pal.text)).truncate());
            let from = ui.add(
                egui::Label::new(
                    RichText::new(format!("From {source} · {when}")).color(pal.text_dim),
                )
                .truncate(),
            );
            if let Some(path) = &work.source {
                from.on_hover_text(path.display().to_string());
            }
            ui.label(
                RichText::new(format!(
                    "{tracks} track{} · {} bars · {:.0} BPM",
                    if tracks == 1 { "" } else { "s" },
                    project.bars,
                    project.bpm,
                ))
                .color(pal.text_dim),
            );
            ui.add_space(theme::GAP_S);
            theme::hint(ui, pal, &replaces);
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Restore Work").clicked() {
                    choice = Some(Choice::Restore);
                }
                if theme::secondary_button(ui, "Discard")
                    .on_hover_text("Delete the autosave and keep the current song")
                    .clicked()
                {
                    choice = Some(Choice::Discard);
                }
            });
            ui.add_space(theme::GAP_S);
            theme::hint(
                ui,
                pal,
                "This offer stays until you choose — closing Cadence keeps the autosave for next time.",
            );
        });
        if choice.is_none() && enter_pressed(ctx) {
            choice = Some(Choice::Restore);
        }
        match choice {
            Some(Choice::Restore) => {
                let project = project.clone();
                let source = work.source.clone();
                self.recovery_offered = false;
                // Reattach the song file the work came from, so Save writes
                // the recovered work back where it belongs. The song stays
                // dirty (and autosaved) until the user saves.
                let message = match &source {
                    Some(path) => format!(
                        "Restored unsaved work for {} — Save ({}) to keep it.",
                        super::files::file_label(path),
                        ctx.format_shortcut(&super::SC_SAVE)
                    ),
                    None => "Restored the recovered autosave — Save As… to keep it.".to_owned(),
                };
                self.replace_project(project, source, false);
                self.set_status(StatusKind::Success, message);
                false
            }
            Some(Choice::Discard) => {
                if let Some(dir) = self.recovery_dir.clone() {
                    match persistence::clear_recovery_in(&dir) {
                        Ok(()) => {
                            self.recovery_offered = false;
                            self.set_status(StatusKind::Info, "Discarded the recovery autosave.");
                            false
                        }
                        // Failed delete: keep the offer open so the user can
                        // retry or restore instead of silently losing data.
                        Err(err) => {
                            self.set_status(
                                StatusKind::Error,
                                format!("Could not discard the autosave: {err}"),
                            );
                            true
                        }
                    }
                } else {
                    self.recovery_offered = false;
                    false
                }
            }
            None => true,
        }
    }

    // ----- new song ---------------------------------------------------------

    /// Name + key for a new song. Create (or Enter) replaces the song; Cancel,
    /// Escape or a backdrop click keep the current one.
    fn new_song_dialog(&mut self, ctx: &Context, draft: &mut NewSongDraft) -> bool {
        let pal = self.palette();
        let mut create = false;
        let mut cancel = false;
        let resp = show_dialog(ctx, "cadence-new-song", pal, W_COMPACT, |ui| {
            header(ui, pal, "NEW SONG", "Start a new song");
            ui.add_space(theme::GAP_M);
            ui.label(theme::kicker_text(pal, "NAME"));
            ui.add_space(theme::GAP_XS);
            ui.add_sized(
                vec2(ui.available_width(), theme::CONTROL_M),
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text("Song name")
                    .char_limit(MAX_NAME_CHARS)
                    .margin(Margin::symmetric(8, 4))
                    .vertical_align(Align::Center),
            );
            ui.add_space(theme::GAP_M);
            ui.label(theme::kicker_text(pal, "KEY"));
            ui.add_space(theme::GAP_XS);
            key_picker(ui, pal, &mut draft.key);
            ui.add_space(theme::GAP_S);
            theme::hint(
                ui,
                pal,
                &format!(
                    "{} — the Arpeggiator offers this key's chords. You can change it later.",
                    ops::key_label(draft.key)
                ),
            );
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Create").clicked() {
                    create = true;
                }
                if theme::secondary_button(ui, "Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            create = true;
        }
        if create {
            let (name, key) = (draft.name.clone(), draft.key);
            self.create_new_song(&name, key);
            return false;
        }
        !cancel && !resp.should_close()
    }

    // ----- arpeggiator ------------------------------------------------------

    /// `(track id, pattern id)` the arpeggiator writes to: the selected track
    /// when it uses the piano roll (not the drum grid) and has a pattern.
    pub(super) fn arp_target(&self) -> Option<(u64, u64)> {
        let track = self.selected_track()?;
        if track.shows_drum_grid() {
            return None;
        }
        Some((track.id, self.current_pattern_id(track)?))
    }

    pub(super) fn can_arpeggiate(&self) -> bool {
        self.arp_target().is_some()
    }

    /// Edit ▸ Arpeggiator…: opens the dialog with defaults for the song key.
    pub(super) fn open_arpeggiator(&mut self) {
        let Some(track) = self
            .arp_target()
            .and_then(|(t, _)| ops::track(&self.project, t))
        else {
            self.set_status(
                StatusKind::Warning,
                "To use the Arpeggiator, select a melodic track (not drums or audio) that has a pattern.",
            );
            return;
        };
        let draft = ArpDraft::new(self.project.key, track.instrument.default_pitch());
        self.dialog = Dialog::Arp(draft);
    }

    /// Writes an arpeggio into the target pattern as one undo step. Returns
    /// the number of notes written, or `None` when there is no target.
    pub(super) fn arpeggiate(&mut self, spec: &ArpSpec) -> Option<usize> {
        let Some((track_id, pattern_id)) = self.arp_target() else {
            self.set_status(
                StatusKind::Error,
                "To use the Arpeggiator, select a melodic track (not drums or audio) that has a pattern.",
            );
            return None;
        };
        let mut written = 0;
        let mut pattern_name = String::new();
        self.edit("Arpeggiate", None, |p| {
            if let Some(pat) = ops::pattern_mut(p, track_id, pattern_id) {
                written = ops::generate_arpeggio(pat, spec);
                pattern_name = pat.name.clone();
            }
        });
        self.fix_selection();
        if written == 0 {
            self.set_status(
                StatusKind::Warning,
                format!("No notes written — \"{pattern_name}\" is full."),
            );
        } else {
            self.set_status(
                StatusKind::Success,
                format!(
                    "Arpeggio: {} · {} · {} oct · {} note{} in \"{pattern_name}\".",
                    spec.quality.symbol_in(spec.chord_root, self.project.key),
                    spec.direction.label(),
                    spec.octaves,
                    written,
                    if written == 1 { "" } else { "s" },
                ),
            );
        }
        Some(written)
    }

    /// Changes the song key as an undoable edit.
    pub(super) fn set_song_key(&mut self, key: SongKey) {
        let key = SongKey::new(key.root, key.mode);
        if self.edit("Change key", None, |p| p.key = key) {
            self.set_status(
                StatusKind::Info,
                format!("Song key: {}.", ops::key_label(key)),
            );
        }
    }

    /// Chord, pattern and key controls. Generate writes (undoably) and keeps
    /// the dialog open for another try; Done / Escape / backdrop close it.
    fn arp_dialog(&mut self, ctx: &Context, draft: &mut ArpDraft) -> bool {
        let Some((track_id, pattern_id)) = self.arp_target() else {
            return false;
        };
        let target = ops::track(&self.project, track_id).map_or_else(String::new, |t| {
            let pattern = ops::pattern(t, pattern_id).map_or("", |p| p.name.as_str());
            format!("Writes into \"{pattern}\" on {}.", t.name)
        });
        let key = self.project.key;
        let mut new_key = key;
        let mut generate = false;
        let mut done = false;
        let pal = self.palette();
        const COMBO_W: f32 = 200.0;
        let resp = show_dialog(ctx, "cadence-arp", pal, W_FORM, |ui| {
            header(ui, pal, "ARPEGGIATOR", "Arpeggiate a chord");
            ui.add(
                egui::Label::new(RichText::new(&target).small().color(pal.text_faint)).truncate(),
            );
            ui.add_space(theme::GAP_M);

            ui.label(theme::kicker_text(pal, "SONG KEY"));
            ui.add_space(theme::GAP_XS);
            key_picker(ui, pal, &mut new_key);
            ui.add_space(theme::GAP_M);

            ui.label(theme::kicker_text(pal, "CHORD"));
            ui.add_space(theme::GAP_XS);
            egui::Grid::new("arp-chord")
                .num_columns(2)
                .spacing([theme::GAP_L, theme::GAP_S])
                .show(ui, |ui| {
                    form_label(ui, pal, "Chord");
                    theme::combo("arp-degree", degree_label(key, draft.degree), COMBO_W).show_ui(
                        ui,
                        |ui| {
                            for d in 0..7 {
                                let on = draft.degree == Some(d);
                                if ui
                                    .selectable_label(on, degree_label(key, Some(d)))
                                    .clicked()
                                {
                                    draft.degree = Some(d);
                                    draft.quality = key.degree_chord_quality(d);
                                }
                            }
                            if ui
                                .selectable_label(draft.degree.is_none(), "Custom…")
                                .clicked()
                            {
                                draft.custom_root = draft.root_class(key);
                                draft.degree = None;
                            }
                        },
                    );
                    ui.end_row();
                    if draft.degree.is_none() {
                        form_label(ui, pal, "Root");
                        theme::combo("arp-root", key.spell(draft.custom_root), COMBO_W).show_ui(
                            ui,
                            |ui| {
                                for pc in 0..12u8 {
                                    ui.selectable_value(&mut draft.custom_root, pc, key.spell(pc));
                                }
                            },
                        );
                        ui.end_row();
                    }
                    form_label(ui, pal, "Type");
                    theme::combo("arp-quality", draft.quality.label(), COMBO_W).show_ui(ui, |ui| {
                        for q in ChordQuality::ALL {
                            ui.selectable_value(&mut draft.quality, q, q.label());
                        }
                    });
                    ui.end_row();
                    form_label(ui, pal, "Start octave");
                    theme::combo("arp-octave", format!("Octave {}", draft.octave), COMBO_W)
                        .show_ui(ui, |ui| {
                            for o in ARP_MIN_OCTAVE..=ARP_MAX_OCTAVE {
                                let label = if o == 4 {
                                    format!("Octave {o} (middle C)")
                                } else {
                                    format!("Octave {o}")
                                };
                                ui.selectable_value(&mut draft.octave, o, label);
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(theme::GAP_M);

            ui.label(theme::kicker_text(pal, "PATTERN"));
            ui.add_space(theme::GAP_XS);
            egui::Grid::new("arp-pattern")
                .num_columns(2)
                .spacing([theme::GAP_L, theme::GAP_S])
                .show(ui, |ui| {
                    form_label(ui, pal, "Direction");
                    theme::combo("arp-direction", draft.direction.label(), COMBO_W).show_ui(
                        ui,
                        |ui| {
                            for d in ArpDirection::ALL {
                                ui.selectable_value(&mut draft.direction, d, d.label());
                            }
                        },
                    );
                    ui.end_row();
                    form_label(ui, pal, "Octaves");
                    ui.horizontal(|ui| {
                        theme::slider(
                            ui,
                            pal,
                            &mut draft.octaves,
                            1..=ops::MAX_ARP_OCTAVES,
                            COMBO_W - 40.0,
                        );
                        ui.label(RichText::new(draft.octaves.to_string()).color(pal.text_dim));
                    });
                    ui.end_row();
                    form_label(ui, pal, "Rate");
                    let rate = ARP_RATES
                        .iter()
                        .find(|(r, _)| *r == draft.rate_steps)
                        .map_or("", |(_, l)| l);
                    theme::combo("arp-rate", rate, COMBO_W).show_ui(ui, |ui| {
                        for (r, label) in ARP_RATES {
                            ui.selectable_value(&mut draft.rate_steps, r, label);
                        }
                    });
                    ui.end_row();
                    form_label(ui, pal, "Velocity");
                    ui.horizontal(|ui| {
                        theme::slider(ui, pal, &mut draft.velocity, 0.2..=1.0, COMBO_W - 40.0);
                        ui.label(
                            RichText::new(format!("{:.0}%", draft.velocity * 100.0))
                                .color(pal.text_dim),
                        );
                    });
                    ui.end_row();
                });
            ui.add_space(theme::GAP_S);
            ui.checkbox(&mut draft.accent, "Accent the first note of each bar");
            ui.checkbox(&mut draft.replace, "Replace the pattern's notes");
            ui.add_space(theme::GAP_S);
            let spec = draft.spec(key);
            let tones: Vec<String> = ops::arp_tones(&spec)
                .into_iter()
                .map(|p| ops::note_name_in(p, key))
                .collect();
            theme::hint(
                ui,
                pal,
                &format!(
                    "{}: {}",
                    spec.quality.symbol_in(spec.chord_root, key),
                    tones.join(" ")
                ),
            );
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Generate").clicked() {
                    generate = true;
                }
                if theme::secondary_button(ui, "Done").clicked() {
                    done = true;
                }
            });
        });
        if enter_pressed(ctx) {
            generate = true;
        }
        if new_key != key {
            self.set_song_key(new_key);
            if let Some(d) = draft.degree {
                draft.quality = new_key.degree_chord_quality(d);
            }
        }
        if generate {
            let spec = draft.spec(self.project.key);
            self.arpeggiate(&spec);
        }
        !done && !resp.should_close()
    }

    // ----- help ---------------------------------------------------------------

    /// Shortcuts, mouse gestures and editing tips (F1 / Help menu): the one
    /// place every shortcut and gesture is written down, a page per area.
    fn help_dialog(&mut self, ctx: &Context) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let height = (ctx.content_rect().height() - 260.0).clamp(220.0, 560.0);
        let page_id = egui::Id::new("cadence-help-page");
        let mut page: HelpPage = ctx.data(|d| d.get_temp(page_id)).unwrap_or_default();
        let resp = show_dialog(ctx, "cadence-help", pal, W_HELP, |ui| {
            header(ui, pal, "SHORTCUTS & TIPS", "How to work in Cadence");
            ui.add_space(theme::GAP_S);
            let pages = HelpPage::ALL.map(|p| theme::Segment::new(p, p.label()));
            theme::segmented(ui, pal, &mut page, &pages, theme::CONTROL_M, None);
            ui.add_space(theme::GAP_M);
            ScrollArea::vertical()
                .id_salt(("help-scroll", page))
                .max_height(height)
                .auto_shrink([false, true])
                .show(ui, |ui| match page {
                    HelpPage::Shortcuts => {
                        ui.columns(2, |cols| {
                            for section in &SHORTCUT_SECTIONS[..2] {
                                help_section(&mut cols[0], pal, ctx, section);
                            }
                            for section in &SHORTCUT_SECTIONS[2..] {
                                help_section(&mut cols[1], pal, ctx, section);
                            }
                        });
                    }
                    page => {
                        for section in page.sections() {
                            help_section(ui, pal, ctx, section);
                        }
                    }
                });
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Done").clicked() {
                    closed = true;
                }
            });
        });
        ctx.data_mut(|d| d.insert_temp(page_id, page));
        closed |= enter_pressed(ctx);
        !closed && !resp.should_close()
    }

    // ----- about ---------------------------------------------------------------

    /// Honest about-box: what Cadence is, what it isn't (no plugins yet).
    fn about_dialog(&mut self, ctx: &Context) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let resp = show_dialog(ctx, "cadence-about", pal, W_COMPACT, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_M;
                let (mark, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
                paint_brand_mark(ui.painter(), mark, pal);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(
                        theme::strong_text("Cadence")
                            .size(theme::TEXT_HEADING + 4.0)
                            .color(pal.text),
                    );
                    ui.label(RichText::new("A focused songwriting studio").color(pal.text_dim));
                    theme::hint(ui, pal, &format!("Version {}", env!("CARGO_PKG_VERSION")));
                });
            });
            ui.add_space(theme::GAP_M);
            ui.label(
                RichText::new(
                    "Write drum patterns and melodies, arrange them across instrument tracks, \
                     mix with per-track effects and export WAV or MIDI. Everything is written \
                     in Rust and runs fully offline — no account, no cloud. The only network \
                     use is the optional update check (Help → Check for Updates).",
                )
                .color(pal.text_dim),
            );
            ui.add_space(theme::GAP_S);
            ui.label(
                RichText::new(
                    "Songs are .cadence files you can keep and share. Instrument packs add new \
                     synth sounds as plain JSON, with no rebuild. Plugin instruments (VST3, AU, \
                     CLAP) are planned but not supported yet.",
                )
                .small()
                .color(pal.text_faint),
            );
            ui.add_space(theme::GAP_M);
            ui.label(theme::kicker_text(pal, "CREDITS"));
            ui.add_space(theme::GAP_XS);
            for line in [
                "Inter — SIL Open Font License 1.1",
                "DejaVu Sans — Bitstream Vera / DejaVu license",
                "Built with Rust, egui and eframe, CPAL, rustysynth, midly and hound",
            ] {
                ui.label(RichText::new(line).small().color(pal.text_dim));
            }
            ui.add_space(theme::GAP_XS);
            theme::hint(
                ui,
                pal,
                "The font license texts are in the licenses folder of this download, and every crate and font license is in THIRD-PARTY-NOTICES.txt beside it.",
            );
            ui.add_space(theme::GAP_XS);
            if theme::secondary_button(ui, "Third-Party Notices…")
                .on_hover_text(
                    "Every Rust crate and bundled font Cadence ships, with license texts",
                )
                .clicked()
            {
                self.dialog = Dialog::Notices;
            }
            if let Some(note) = UNSIGNED_NOTE {
                ui.add_space(theme::GAP_S);
                theme::hint(ui, pal, note);
            }
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Close").clicked() {
                    closed = true;
                }
            });
        });
        closed |= enter_pressed(ctx);
        !closed && !resp.should_close()
    }

    /// The generated third-party notices (crates + bundled fonts), embedded
    /// at build time so the text is always available even when the package's
    /// THIRD-PARTY-NOTICES.txt file is not at hand.
    fn notices_dialog(&mut self, ctx: &Context) -> bool {
        /// Height of the notices dialog around its scrolling text.
        const NOTICES_CHROME: f32 = 140.0;
        let pal = self.palette();
        let mut closed = false;
        let resp = show_dialog(ctx, "cadence-notices", pal, W_HELP, |ui| {
            ui.label(
                RichText::new("Third-Party Notices")
                    .size(theme::TEXT_HEADING)
                    .color(pal.text),
            );
            theme::hint(
                ui,
                pal,
                "Licenses for every Rust crate and bundled font in this build.",
            );
            ui.add_space(theme::GAP_S);
            // Title, hint and the Close row stay put; only the text scrolls.
            // The text is ~1.6 MB: it is wrapped into lines once per width
            // and only the visible rows are laid out each frame.
            let font = egui::FontId::monospace(
                ui.style()
                    .text_styles
                    .get(&egui::TextStyle::Small)
                    .map_or(theme::TEXT_SMALL, |f| f.size),
            );
            let (row_h, char_w) =
                ui.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
            let width = ui.available_width() - ui.spacing().scroll.bar_width - 4.0;
            let lines = notices_lines((width / char_w.max(1.0)).floor() as usize);
            ScrollArea::vertical()
                .id_salt("notices-body")
                .max_height((dialog_max_height(ctx) - NOTICES_CHROME).max(80.0))
                .auto_shrink([false, false])
                .show_rows(ui, row_h, lines.len(), |ui, rows| {
                    ui.set_min_width(ui.available_width());
                    for line in &lines[rows] {
                        ui.add(
                            egui::Label::new(
                                RichText::new(*line).font(font.clone()).color(pal.text_dim),
                            )
                            .truncate(),
                        );
                    }
                });
            ui.add_space(theme::GAP_S);
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Close").clicked() {
                    closed = true;
                }
            });
        });
        closed |= enter_pressed(ctx);
        !closed && !resp.should_close()
    }

    // ----- preferences ---------------------------------------------------------

    /// File ▸ Preferences… (⌘/Ctrl+,): the app-wide settings in one place.
    /// Every control applies immediately and is saved the same way as its
    /// other entry point (View ▸ Theme, the status-bar output picker, the
    /// update dialog's checkbox), so there is nothing to confirm or cancel.
    fn preferences_dialog(&mut self, ctx: &Context) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let mut theme_pick = self.theme;
        let mut device_pick: Option<Option<String>> = None;
        let mut auto = self.update.prefs.auto_check;
        let current_device = self.audio_device_pref.clone();
        let live_device = self.engine.as_ref().map(|e| e.device_name());
        let audio_error = self.audio_error.clone();
        let list_devices = self.io_enabled;
        const COMBO_W: f32 = 260.0;
        let resp = show_dialog(ctx, "cadence-preferences", pal, W_FORM, |ui| {
            header(ui, pal, "PREFERENCES", "Cadence settings");
            ui.add_space(theme::GAP_M);

            ui.label(theme::kicker_text(pal, "APPEARANCE"));
            ui.add_space(theme::GAP_XS);
            let options: Vec<theme::Segment<'_, theme::ThemeChoice>> = theme::ThemeChoice::ALL
                .iter()
                .map(|c| theme::Segment::new(*c, c.label()).tip(c.description()))
                .collect();
            theme::segmented(ui, pal, &mut theme_pick, &options, theme::CONTROL_M, None);
            ui.add_space(theme::GAP_M);

            ui.label(theme::kicker_text(pal, "AUDIO OUTPUT"));
            ui.add_space(theme::GAP_XS);
            let shown = current_device
                .clone()
                .unwrap_or_else(|| "System default".to_owned());
            theme::combo("prefs-audio-device", shown, COMBO_W).show_ui(ui, |ui| {
                if ui
                    .selectable_label(current_device.is_none(), "System default")
                    .clicked()
                {
                    device_pick = Some(None);
                }
                // Device enumeration only runs while the list is open, and
                // never in headless sessions.
                if list_devices {
                    for name in crate::audio::AudioEngine::output_devices() {
                        let on = current_device.as_deref() == Some(name.as_str());
                        if ui.selectable_label(on, &name).clicked() {
                            device_pick = Some(Some(name));
                        }
                    }
                }
            });
            let note = match (&live_device, &audio_error) {
                (Some(name), _) if !name.is_empty() => format!("Playing through {name}."),
                (Some(_), _) => "Audio output ready.".to_owned(),
                (None, Some(err)) => format!("No audio output: {err}"),
                (None, None) => "No audio output.".to_owned(),
            };
            ui.add(egui::Label::new(RichText::new(note).small().color(pal.text_faint)).truncate());
            ui.add_space(theme::GAP_M);

            ui.label(theme::kicker_text(pal, "UPDATES"));
            ui.add_space(theme::GAP_XS);
            ui.checkbox(&mut auto, "Check for updates automatically");
            theme::hint(
                ui,
                pal,
                "At launch and once a day. Help ▸ Check for Updates… always works.",
            );

            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Done").clicked() {
                    closed = true;
                }
            });
        });
        if theme_pick != self.theme {
            self.set_theme(ctx, theme_pick);
        }
        if let Some(device) = device_pick {
            self.set_audio_device(device);
        }
        if auto != self.update.prefs.auto_check {
            self.update.prefs.auto_check = auto;
            self.store_update_prefs();
        }
        closed |= enter_pressed(ctx);
        !closed && !resp.should_close()
    }

    // ----- audio store cleanup ---------------------------------------------------

    /// Confirms File ▸ Clean Up Unused Audio…. Destructive, so Enter does
    /// not confirm; Escape, a backdrop click or Cancel keep every file.
    fn clean_audio_dialog(&mut self, ctx: &Context, plan: &persistence::UnusedAudio) -> bool {
        let pal = self.palette();
        let mut remove = false;
        let mut cancel = false;
        let n = plan.files.len();
        let size = super::files::format_bytes(plan.bytes);
        let resp = show_dialog(ctx, "cadence-clean-audio", pal, W_COMPACT, |ui| {
            header(
                ui,
                pal,
                "AUDIO STORE",
                &format!(
                    "Remove {n} unused audio file{}?",
                    if n == 1 { "" } else { "s" }
                ),
            );
            ui.add_space(theme::GAP_S);
            ui.label(RichText::new(format!("This frees {size} of disk space.")).color(pal.text));
            ui.add_space(theme::GAP_S);
            ui.label(
                RichText::new(
                    "These imported WAVs are not used by the open song (or its undo history), \
                     the crash-recovery autosave, or any song in File ▸ Open Recent.",
                )
                .color(pal.text_dim),
            );
            ui.add_space(theme::GAP_S);
            theme::hint(
                ui,
                pal,
                "Other songs that still use them will play those clips silent. Open such songs \
                 first, or keep a copy made with File ▸ Save Copy with Audio.",
            );
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Remove Files").clicked() {
                    remove = true;
                }
                if theme::secondary_button(ui, "Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if remove {
            let plan = plan.clone();
            self.run_audio_cleanup(&plan);
            return false;
        }
        !cancel && !resp.should_close()
    }

    // ----- pack errors ---------------------------------------------------------

    /// Lists non-fatal instrument-pack load problems. Reload Packs re-reads
    /// the folder (and comes back here if something still fails).
    fn pack_errors_dialog(&mut self, ctx: &Context, errors: &[String]) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let mut reload = false;
        let dir = crate::packs::user_packs_dir();
        let resp = show_dialog(ctx, "cadence-pack-errors", pal, W_LIST, |ui| {
            header(
                ui,
                pal,
                "INSTRUMENT PACKS",
                "Some packs could not be loaded",
            );
            ui.add_space(theme::GAP_XS);
            ui.label(
                RichText::new(
                    "Cadence keeps working with the packs that did load. Fix or remove these \
                     files, then reload.",
                )
                .small()
                .color(pal.text_dim),
            );
            ui.add_space(theme::GAP_S);
            // Height of the dialog around the list: header, notes, buttons.
            const CHROME: f32 = 200.0;
            ScrollArea::vertical()
                .max_height((dialog_max_height(ctx) - CHROME).clamp(80.0, 300.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for err in errors {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new("•").color(pal.danger));
                            ui.label(RichText::new(err).small().color(pal.text));
                        });
                        ui.add_space(theme::GAP_XS);
                    }
                });
            ui.add_space(theme::GAP_S);
            ui.add(
                egui::Label::new(
                    RichText::new(format!("Packs folder: {}", dir.display()))
                        .small()
                        .color(pal.text_faint),
                )
                .truncate(),
            );
            button_row(ui, |ui| {
                if theme::primary_button(ui, pal, "Close").clicked() {
                    closed = true;
                }
                if theme::secondary_button(ui, "Reload Packs")
                    .on_hover_text("Read the packs folder again")
                    .clicked()
                {
                    reload = true;
                }
            });
        });
        closed |= enter_pressed(ctx);
        if reload {
            // Opens this dialog again when some file still fails to load.
            self.reload_packs(true);
            return false;
        }
        !closed && !resp.should_close()
    }
}

// ----- scaffold ---------------------------------------------------------------

/// Note under the credits for builds that are not distribution-signed.
const UNSIGNED_NOTE: Option<&str> = if cfg!(target_os = "macos") {
    Some("This build is ad-hoc signed only, not notarized: macOS may ask you to confirm the first launch.")
} else if cfg!(target_os = "windows") {
    Some("This build is not code-signed: Windows SmartScreen may show a warning the first time you run it.")
} else {
    None
};

/// Centered modal with the app's styling and a dimmed backdrop.
fn modal(id: &str, pal: Palette) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            Frame::new()
                .fill(pal.panel)
                .stroke(Stroke::new(1.0f32, pal.border))
                .corner_radius(theme::RADIUS_L as u8 + 2)
                .inner_margin(Margin::same(20)),
        )
}

/// Room kept between a dialog's frame and the window edge, plus the
/// frame's own margins, top and bottom combined.
const DIALOG_V_CHROME: f32 = 2.0 * 24.0 + 2.0 * 21.0;

/// Tallest a dialog body may grow in this window: the modal plus its
/// frame always fits on screen.
pub(super) fn dialog_max_height(ctx: &Context) -> f32 {
    (ctx.content_rect().height() - DIALOG_V_CHROME).max(120.0)
}

/// A modal `width` px wide (never wider than the window) with `add` inside.
/// A body taller than the window scrolls, so its button row stays reachable.
pub(super) fn show_dialog<T>(
    ctx: &Context,
    id: &str,
    pal: Palette,
    width: f32,
    add: impl FnOnce(&mut Ui) -> T,
) -> egui::ModalResponse<T> {
    let width = width.min(ctx.content_rect().width() - 64.0).max(240.0);
    let max_height = dialog_max_height(ctx);
    modal(id, pal).show(ctx, |ui| {
        ui.set_width(width);
        // The modal area offers last frame's size; let the body grow to the
        // window-derived cap instead.
        ui.set_max_height(max_height);
        ScrollArea::vertical()
            .id_salt((id, "dialog-body"))
            .max_height(max_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_width(width);
                add(ui)
            })
            .inner
    })
}

/// Kicker + title header shared by all dialogs.
pub(super) fn header(ui: &mut Ui, pal: Palette, kicker: &str, title: &str) {
    theme::section_header(ui, pal, kicker, title);
}

/// "just now", "5 minutes ago", "3 hours ago", "2 days ago" — relative, so no
/// time-zone handling is needed.
pub(super) fn time_ago(t: std::time::SystemTime) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| d.as_secs());
    let (n, unit) = match secs {
        0..60 => return "just now".to_owned(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// The dialog's buttons, right-aligned. Add the primary action first: it
/// ends up last (rightmost), with the secondary actions to its left.
pub(super) fn button_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(theme::GAP_L);
    // A fixed-height row: `with_layout` would claim all the height the modal
    // offers and centre the buttons in it, stretching the dialog.
    let _row = ui
        .allocate_ui_with_layout(
            vec2(ui.available_width(), theme::CONTROL_L),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = theme::GAP_S;
                add(ui);
            },
        )
        .response
        .rect;
    #[cfg(test)]
    ui.ctx()
        .data_mut(|d| d.insert_temp(button_row_probe_id(), _row));
}

/// Where tests find the rect of the last drawn dialog button row.
#[cfg(test)]
fn button_row_probe_id() -> egui::Id {
    egui::Id::new("dialog-button-row-probe")
}

/// Enter confirms a dialog unless a widget (a button reached with Tab, a
/// combo box) has keyboard focus and will use the key itself.
fn enter_pressed(ctx: &Context) -> bool {
    ctx.memory(|m| m.focused().is_none())
        && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter))
}

/// Left cell of a form row.
fn form_label(ui: &mut Ui, pal: Palette, text: &str) {
    ui.label(RichText::new(text).color(pal.text_dim));
}

// ----- help content -------------------------------------------------------------

/// Pages of the Help dialog.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum HelpPage {
    #[default]
    Shortcuts,
    Arranger,
    Editor,
    Mixer,
    Keyboard,
}

impl HelpPage {
    pub(super) const ALL: [HelpPage; 5] = [
        HelpPage::Shortcuts,
        HelpPage::Arranger,
        HelpPage::Editor,
        HelpPage::Mixer,
        HelpPage::Keyboard,
    ];

    fn label(self) -> &'static str {
        match self {
            HelpPage::Shortcuts => "Shortcuts",
            HelpPage::Arranger => "Arranger",
            HelpPage::Editor => "Pattern editor",
            HelpPage::Mixer => "Mixer",
            HelpPage::Keyboard => "Keyboard",
        }
    }

    /// The page's sections, top to bottom (Shortcuts lays them out in two
    /// columns).
    pub(super) fn sections(self) -> &'static [Section] {
        match self {
            HelpPage::Shortcuts => SHORTCUT_SECTIONS,
            HelpPage::Arranger => ARRANGER_SECTIONS,
            HelpPage::Editor => EDITOR_SECTIONS,
            HelpPage::Mixer => MIXER_SECTIONS,
            HelpPage::Keyboard => KEYBOARD_SECTIONS,
        }
    }
}

/// How to do it: the right-hand side of a Help row.
#[derive(Clone, Copy, Debug)]
pub(super) enum Keys {
    /// A real shortcut, written in the platform's notation.
    Shortcut(&'static KeyboardShortcut),
    /// Either of two shortcuts.
    Either(&'static KeyboardShortcut, &'static KeyboardShortcut),
    /// Literal keys; `" / "` separates alternatives, `+` joins a chord.
    Text(&'static str),
    /// A mouse gesture. Leading `{cmd}+`, `{alt}+` and `{shift}+` become
    /// key caps with the platform's modifier names; `" / "` separates
    /// alternatives.
    Mouse(&'static str),
    /// Only reachable from a menu.
    Menu(&'static str),
}

/// `(what you want, how to do it)`.
pub(super) type Row = (&'static str, Keys);

/// A titled group of Help rows.
pub(super) struct Section {
    pub(super) title: &'static str,
    pub(super) rows: &'static [Row],
}

use Keys::{Either, Menu, Mouse, Shortcut, Text};

/// The Shortcuts page: the first two sections fill the left column.
const SHORTCUT_SECTIONS: &[Section] = &[
    Section {
        title: "TRANSPORT",
        rows: &[
            ("Play / pause", Shortcut(&super::SC_PLAY)),
            ("Return to start", Shortcut(&super::SC_HOME)),
            ("Loop on / off", Shortcut(&super::SC_LOOP)),
            ("Record what you play", Shortcut(&super::SC_RECORD)),
            ("Metronome", Menu("Transport menu")),
            ("Tap the tempo", Mouse("Click TAP in rhythm")),
            ("Move the playhead", Mouse("Click the ruler")),
        ],
    },
    Section {
        title: "FILE",
        rows: &[
            ("New song", Shortcut(&super::SC_NEW)),
            ("Open", Shortcut(&super::SC_OPEN)),
            ("Save", Shortcut(&super::SC_SAVE)),
            ("Save as", Shortcut(&super::SC_SAVE_AS)),
            ("Export WAV", Shortcut(&super::SC_EXPORT_WAV)),
            ("Export stems or MIDI", Menu("File menu")),
            ("Import MIDI, audio, packs", Menu("File menu")),
            ("Quit", Shortcut(&super::SC_QUIT)),
        ],
    },
    Section {
        title: "EDITING",
        rows: &[
            ("Undo", Shortcut(&super::SC_UNDO)),
            ("Redo", Either(&super::SC_REDO, &super::SC_REDO_ALT)),
            ("Cut", Shortcut(&super::SC_CUT)),
            ("Copy", Shortcut(&super::SC_COPY)),
            (
                "Paste at the pointer or playhead",
                Shortcut(&super::SC_PASTE),
            ),
            ("Duplicate", Shortcut(&super::SC_DUPLICATE)),
            ("Select all", Shortcut(&super::SC_SELECT_ALL)),
            ("Delete", Text("Del / Backspace")),
            ("Deselect, close a dialog", Text("Esc")),
            ("Nudge the selection", Text("← / → / ↑ / ↓")),
            ("Nudge by an octave", Text("Shift+↑ / Shift+↓")),
            ("Arpeggiator", Menu("Edit menu")),
        ],
    },
    Section {
        title: "VIEW & PANELS",
        rows: &[
            ("Pattern editor", Shortcut(&super::SC_EDITOR)),
            ("Mixer", Shortcut(&super::SC_MIXER)),
            ("Keyboard", Shortcut(&super::SC_KEYS)),
            ("Maximize the editor", Shortcut(&super::SC_MAX_EDITOR)),
            ("Maximize the arranger", Shortcut(&super::SC_MAX_ARRANGER)),
            ("Resize arranger and editor", Mouse("Drag the divider")),
            (
                "Maximize, again to restore",
                Mouse("Double-click the divider"),
            ),
            ("Theme, fit song", Menu("View menu")),
            ("These tips", Shortcut(&super::SC_HELP)),
        ],
    },
    Section {
        title: "LIBRARY",
        rows: &[
            (
                "Add a sound as a new track",
                Mouse("Double-click it / click +"),
            ),
            ("Add it at a bar", Mouse("Drag it onto the arranger")),
            ("Preview a sound", Mouse("Click ▶")),
            ("Fold a section", Mouse("Click its header")),
            ("Center a track's pan", Mouse("Double-click the Pan slider")),
        ],
    },
];

/// Scrolling and zooming: the same in the arranger and both editors.
const NAVIGATION: Section = Section {
    title: "NAVIGATION",
    rows: &[
        ("Scroll up and down", Mouse("Wheel")),
        ("Scroll left and right", Mouse("{shift}+wheel")),
        ("Zoom time at the pointer", Mouse("{cmd}+wheel / pinch")),
        ("Taller or shorter lanes and rows", Mouse("{alt}+wheel")),
        ("Pan freely", Mouse("Middle-drag / two-finger swipe")),
        ("Zoom in / out", Text("= / -")),
    ],
};

const ARRANGER_SECTIONS: &[Section] = &[
    Section {
        title: "CLIPS",
        rows: &[
            ("Select a clip and show its pattern", Mouse("Click it")),
            (
                "Add to or remove from the selection",
                Mouse("{shift}+click"),
            ),
            (
                "Box-select clips across tracks",
                Mouse("Drag empty lane space"),
            ),
            (
                "Move (selected clips move as a group)",
                Mouse("Drag a clip"),
            ),
            ("Copy", Mouse("{alt}+drag a clip")),
            ("Fade in / fade out", Mouse("Drag a top corner")),
            ("Loop the clip", Mouse("Drag the right edge")),
            ("Edit its pattern", Mouse("Double-click it")),
            (
                "Place the track's pattern",
                Mouse("Double-click an empty lane"),
            ),
            ("Split at the pointer or playhead", Text("B")),
            (
                "Repeat, fades, unique copy, delete",
                Mouse("Right-click a clip"),
            ),
            (
                "Paste here, place a pattern, markers",
                Mouse("Right-click a lane"),
            ),
            ("New track with a sound", Mouse("Drag a library sound in")),
        ],
    },
    Section {
        title: "RULER & MARKERS",
        rows: &[
            (
                "Move the playhead (snaps to beats)",
                Mouse("Click or drag the ruler"),
            ),
            ("Exact 1/16 positions", Mouse("{shift}+drag the ruler")),
            ("Set a loop region", Mouse("Drag the thin top strip")),
            ("Clear the loop region", Mouse("Click ✕ next to its bars")),
            ("Add a section marker", Mouse("Double-click the ruler")),
            ("Move a marker", Mouse("Drag its flag")),
            (
                "Rename / delete a marker",
                Mouse("Double-click / right-click its flag"),
            ),
            ("Markers and song end", Mouse("Right-click the ruler")),
        ],
    },
    Section {
        title: "CHORD LANE (BOTTOM OF THE RULER)",
        rows: &[
            ("Add the key's chord", Mouse("Click the lane")),
            ("Change the chord quality", Mouse("Click a chord")),
            ("Change the chord root", Mouse("{shift}+click a chord")),
            ("Move a chord", Mouse("Drag it")),
            ("Delete a chord", Mouse("Right-click it")),
        ],
    },
    Section {
        title: "TRACKS",
        rows: &[
            ("Select a track", Mouse("Click its header")),
            ("Track settings", Mouse("Double-click its header")),
            ("Track height and more", Mouse("Right-click its header")),
            ("Mute / solo", Mouse("Click M / S")),
        ],
    },
    NAVIGATION,
];

const EDITOR_SECTIONS: &[Section] = &[
    Section {
        title: "PIANO ROLL",
        rows: &[
            ("Draw a note (Note length, Snap)", Mouse("Click empty grid")),
            ("Box-select notes", Mouse("Drag empty grid")),
            (
                "Select a note / add to the selection",
                Mouse("Click / {shift}+click a note"),
            ),
            ("Move the selected notes", Mouse("Drag a note")),
            ("Change a note's length", Mouse("Drag its right edge")),
            ("Erase", Mouse("Right-click / right-drag")),
            ("Set velocity", Mouse("Drag in the velocity lane")),
            ("Hear a pitch", Mouse("Click or drag the keys")),
        ],
    },
    Section {
        title: "DRUM GRID",
        rows: &[
            ("Paint or erase steps", Mouse("Click or drag")),
            ("Accent: 100 → 70 → 40%", Mouse("{shift}+click a hit")),
            ("Erase", Mouse("Right-click / right-drag")),
            ("Hear a piece", Mouse("Click its name")),
            ("Four on the floor on the top row", Mouse("Click Fill")),
        ],
    },
    Section {
        title: "CHORD STRIP (MELODIC TRACKS)",
        rows: &[
            ("Open or close it", Mouse("Click Chords above the roll")),
            ("Hear a chord", Mouse("Hover a chord")),
            ("Add it at the pointer or playhead", Mouse("Click a chord")),
            (
                "Add a progression, one chord per bar",
                Mouse("Click a preset"),
            ),
            (
                "Type, inversion, bass, strum",
                Menu("Menus on the Chords row"),
            ),
            ("Tint the key's notes in the roll", Mouse("Click Scale")),
        ],
    },
    Section {
        title: "PATTERNS",
        rows: &[
            ("Switch pattern", Mouse("Click the pattern name")),
            ("Rename the pattern", Mouse("Double-click the pattern name")),
            ("Rename, clear notes, delete pattern", Mouse("Click ⋯")),
            (
                "Place it at the playhead",
                Mouse("Click Add to arrangement"),
            ),
        ],
    },
    NAVIGATION,
];

const MIXER_SECTIONS: &[Section] = &[Section {
    title: "MIXER",
    rows: &[
        ("Volume, pan, effect amount", Mouse("Drag a fader or knob")),
        ("Fine adjustment", Mouse("{shift}+drag / {alt}+wheel")),
        ("Back to the default", Mouse("Double-click a fader or knob")),
        ("Jump to a level", Mouse("Click the fader groove")),
        ("Scroll the strips", Mouse("Wheel / swipe over them")),
        ("Mute / solo", Mouse("Click M / S")),
        ("Drive, filter, delay, reverb", Mouse("Click FX on a strip")),
        ("Send a track to a bus", Mouse("Pick it under the name")),
        ("Clear a peak or clip light", Mouse("Click the meter")),
        ("Clear every peak", Mouse("Click Clear peaks")),
        ("Narrower strips", Mouse("Click Compact")),
    ],
}];

const KEYBOARD_SECTIONS: &[Section] = &[Section {
    title: "PLAYING FROM THE COMPUTER KEYBOARD",
    rows: &[
        ("White keys C D E F G A B", Text("Z X C V B N M")),
        ("Black keys C♯ D♯ F♯ G♯ A♯", Text("S D G H J")),
        ("Octave down / up", Text("Q / W")),
        ("That note an octave up", Mouse("{shift}+key")),
        ("Glissando", Mouse("Drag across the piano")),
        ("Drum pad (higher click = louder)", Mouse("Click a pad")),
        (
            "Drum pieces from the keyboard",
            Mouse("The key shown on each pad"),
        ),
        ("Record into the pattern", Shortcut(&super::SC_RECORD)),
    ],
}];

/// `{cmd}`, `{alt}`, `{shift}` → the modifier, or `None` for any other token.
fn gesture_modifier(token: &str) -> Option<Modifiers> {
    match token {
        "{cmd}" => Some(Modifiers::COMMAND),
        "{alt}" => Some(Modifiers::ALT),
        "{shift}" => Some(Modifiers::SHIFT),
        _ => None,
    }
}

/// Splits one gesture alternative into its leading modifiers and the mouse
/// action: `"{alt}+drag a clip"` → ([ALT], "drag a clip").
pub(super) fn split_gesture(text: &str) -> (Vec<Modifiers>, &str) {
    let mut mods = Vec::new();
    let mut rest = text;
    while rest.starts_with('{') {
        let Some(m) = rest
            .find('}')
            .and_then(|end| gesture_modifier(&rest[..=end]).map(|m| (m, end)))
        else {
            break;
        };
        mods.push(m.0);
        rest = rest[m.1 + 1..].trim_start_matches('+');
    }
    (mods, rest)
}

/// A titled group of `what → how` rows.
fn help_section(ui: &mut Ui, pal: Palette, ctx: &Context, section: &Section) {
    ui.label(theme::kicker_text(pal, section.title));
    ui.add_space(theme::GAP_XS);
    let left = (ui.available_width() * 0.5).floor();
    ui.spacing_mut().item_spacing.y = theme::GAP_XS;
    for (label, keys) in section.rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::GAP_S;
            ui.allocate_ui_with_layout(
                vec2(left, 22.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(left);
                    ui.add(egui::Label::new(RichText::new(*label).color(pal.text_dim)).wrap());
                },
            );
            match keys {
                Shortcut(sc) => keycaps(ui, pal, &ctx.format_shortcut(sc)),
                Either(a, b) => keycaps(
                    ui,
                    pal,
                    &format!("{} / {}", ctx.format_shortcut(a), ctx.format_shortcut(b)),
                ),
                Text(text) => keycaps(ui, pal, text),
                Mouse(text) => gesture(ui, pal, ctx, text),
                Menu(menu) => {
                    ui.label(RichText::new(*menu).color(pal.text_faint));
                }
            }
        });
    }
    ui.add_space(theme::GAP_L);
}

/// A mouse gesture: modifier key caps, then the action in words.
fn gesture(ui: &mut Ui, pal: Palette, ctx: &Context, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (n, alternative) in text.split(" / ").enumerate() {
            if n > 0 {
                ui.label(RichText::new("or").small().color(pal.text_faint));
            }
            let (mods, action) = split_gesture(alternative);
            for m in mods {
                keycap(ui, pal, &ctx.format_modifiers(m));
                ui.label(RichText::new("+").small().color(pal.text_faint));
            }
            ui.label(RichText::new(action).color(pal.text));
        }
    });
}

/// Splits a shortcut string into its keys: `"Ctrl+Shift+S"` → Ctrl, Shift,
/// S; macOS `"⇧⌘S"` → ⇧, ⌘, S; `"Del"` → Del. A lone `+` or `-` is a key.
fn split_keys(combo: &str) -> Vec<&str> {
    const MAC_MODIFIERS: [char; 4] = ['⌘', '⇧', '⌥', '⌃'];
    if combo.chars().count() > 1 && combo.contains('+') {
        let mut keys = Vec::new();
        let mut rest = combo;
        while let Some(at) = rest.find('+') {
            if at == 0 {
                // A literal "+" key ("Ctrl++").
                keys.push(&rest[..1]);
                rest = &rest[1..];
                rest = rest.strip_prefix('+').unwrap_or(rest);
                continue;
            }
            keys.push(&rest[..at]);
            rest = &rest[at + 1..];
        }
        if !rest.is_empty() {
            keys.push(rest);
        }
        return keys;
    }
    if combo.starts_with(MAC_MODIFIERS) {
        let mut keys = Vec::new();
        let mut rest = combo;
        while let Some(c) = rest.chars().next().filter(|c| MAC_MODIFIERS.contains(c)) {
            keys.push(&rest[..c.len_utf8()]);
            rest = &rest[c.len_utf8()..];
        }
        if !rest.is_empty() {
            keys.push(rest);
        }
        return keys;
    }
    vec![combo]
}

/// Draws a shortcut as key caps; `" / "` separates alternatives.
fn keycaps(ui: &mut Ui, pal: Palette, combo: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (n, alternative) in combo.split(" / ").enumerate() {
            if n > 0 {
                ui.label(RichText::new("or").small().color(pal.text_faint));
            }
            for key in split_keys(alternative) {
                keycap(ui, pal, key);
            }
        }
    });
}

fn keycap(ui: &mut Ui, pal: Palette, key: &str) {
    let font = theme::strong_font(ui, theme::TEXT_SMALL);
    let galley = ui.painter().layout_no_wrap(key.to_owned(), font, pal.text);
    let width = (galley.size().x + 12.0).max(22.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        theme::RADIUS_S,
        pal.control,
        Stroke::new(1.0f32, pal.control_border),
        StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() * 0.5, galley, pal.text);
}

/// The generated third-party notices (crates + bundled fonts), embedded at
/// build time.
const NOTICES: &str = include_str!("../../THIRD-PARTY-NOTICES.txt");

/// [`NOTICES`] wrapped to `columns`, computed once per column count (the
/// last few widths stay cached).
fn notices_lines(columns: usize) -> Arc<Vec<&'static str>> {
    type Cache = Vec<(usize, Arc<Vec<&'static str>>)>;
    static CACHE: Mutex<Cache> = Mutex::new(Vec::new());
    let columns = columns.max(20);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, lines)) = cache.iter().find(|(cols, _)| *cols == columns) {
        return Arc::clone(lines);
    }
    let lines = Arc::new(wrap_lines(NOTICES, columns));
    if cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push((columns, Arc::clone(&lines)));
    lines
}

/// Splits `text` into display lines of at most `columns` characters,
/// breaking long lines after a space where there is one in reach.
fn wrap_lines(text: &str, columns: usize) -> Vec<&str> {
    let columns = columns.max(1);
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line.trim_end();
        loop {
            // Byte offset just past the `columns`-th character.
            let Some((cut, _)) = rest.char_indices().nth(columns) else {
                out.push(rest);
                break;
            };
            let head = &rest[..cut];
            let split = match head.rfind(' ') {
                Some(space) if space > 0 => space + 1,
                _ => cut,
            };
            out.push(rest[..split].trim_end());
            rest = &rest[split..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::files::PendingAction;
    use egui::{Context, Event, Key, Modifiers, RawInput};

    fn headless() -> CadenceApp {
        CadenceApp::headless(Project::demo())
    }

    /// Runs one full UI frame with the given raw input.
    fn run_frame(app: &mut CadenceApp, ctx: &Context, input: RawInput) {
        let _ = ctx.run(input, |ctx| app.frame(ctx));
    }

    fn escape_input() -> RawInput {
        let mut input = RawInput::default();
        input.events.push(Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        input
    }

    /// Dirty song + New must keep the unsaved-changes guard up on every
    /// frame until the user actually picks Save / Don't Save / Cancel.
    #[test]
    fn unsaved_guard_survives_plain_frames() {
        let mut app = headless();
        app.edit("Rename", None, |p| p.name.push('!'));
        assert!(app.dirty());
        app.request(PendingAction::New);
        assert!(matches!(app.dialog, Dialog::Unsaved(PendingAction::New)));

        let ctx = Context::default();
        for _ in 0..4 {
            run_frame(&mut app, &ctx, RawInput::default());
        }
        assert!(matches!(app.dialog, Dialog::Unsaved(PendingAction::New)));
        assert!(app.dirty());
        assert!(app.project.name.ends_with('!'));
    }

    /// Escape cancels the unsaved guard: the pending action is dropped and the
    /// project (still dirty) is untouched.
    #[test]
    fn escape_cancels_unsaved_guard() {
        let mut app = headless();
        app.edit("Rename", None, |p| p.name.push('!'));
        app.request(PendingAction::New);
        let project = app.project.clone();
        let state = app.history.state();

        let ctx = Context::default();
        run_frame(&mut app, &ctx, escape_input());

        assert!(matches!(app.dialog, Dialog::None));
        assert_eq!(app.project, project);
        assert_eq!(app.history.state(), state);
        assert!(app.dirty());
    }

    /// The recovery offer survives plain frames AND Escape: only an explicit
    /// Restore/Discard resolves it, so the autosave is never silently lost.
    #[test]
    fn recovery_offer_survives_frames_and_escape() {
        let mut app = headless();
        app.dialog = Dialog::Recovery(Box::new(persistence::RecoveredWork {
            project: Project::demo(),
            source: None,
            saved_at: None,
        }));
        app.recovery_offered = true;
        let project = app.project.clone();
        let state = app.history.state();

        let ctx = Context::default();
        for _ in 0..3 {
            run_frame(&mut app, &ctx, RawInput::default());
        }
        assert!(matches!(app.dialog, Dialog::Recovery(_)));
        assert!(app.recovery_offered);

        run_frame(&mut app, &ctx, escape_input());
        assert!(matches!(app.dialog, Dialog::Recovery(_)));
        assert!(app.recovery_offered);
        assert_eq!(app.project, project);
        assert_eq!(app.history.state(), state);
    }

    /// Dialogs size to their content, whatever the window height: the button
    /// row once claimed all the height the modal offered, floating the
    /// recovery offer's buttons in a gap above its footer.
    #[test]
    fn dialogs_size_to_their_content() {
        fn recovery() -> Dialog {
            Dialog::Recovery(Box::new(persistence::RecoveredWork {
                project: Project::demo(),
                source: Some("/songs/Neon Circuits.cadence".into()),
                saved_at: None,
            }))
        }
        /// Height of dialog `id` after a few frames in a `window_h` window.
        fn height(id: &str, make: fn() -> Dialog, window_h: f32) -> f32 {
            let mut app = headless();
            app.dialog = make();
            let ctx = Context::default();
            for _ in 0..4 {
                let input = RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1360.0, window_h),
                    )),
                    ..Default::default()
                };
                run_frame(&mut app, &ctx, input);
            }
            ctx.memory(|m| m.area_rect(egui::Id::new(id)))
                .unwrap_or_else(|| panic!("{id} was not shown"))
                .height()
        }
        fn check(id: &str, make: fn() -> Dialog) -> f32 {
            let (short, tall) = (height(id, make, 860.0), height(id, make, 1400.0));
            assert!(
                (short - tall).abs() < 1.0,
                "{id} is {short} px tall in an 860 px window but {tall} px in a 1400 px one"
            );
            short
        }
        // Content plus one 32 px button row: about 300 px. The stretched row
        // made it 438 px with the buttons floating in a gap above the footer.
        let recovery_h = check("cadence-recovery", recovery);
        assert!(
            recovery_h < 340.0,
            "recovery dialog stretched to {recovery_h} px"
        );
        check("cadence-unsaved", || Dialog::Unsaved(PendingAction::Quit));
        check("cadence-new-song", || {
            Dialog::NewSong(NewSongDraft::default())
        });
        check("cadence-clean-audio", || {
            Dialog::CleanAudio(persistence::UnusedAudio {
                files: vec!["/store/0000000000000000.wav".into()],
                bytes: 3 << 20,
            })
        });
        let prefs_h = check("cadence-preferences", || Dialog::Preferences);
        assert!(
            prefs_h < 420.0,
            "preferences dialog stretched to {prefs_h} px"
        );
    }

    /// ⌘, opens Preferences; Escape closes it; its controls change the
    /// live settings without touching the song.
    #[test]
    fn preferences_open_with_the_shortcut_and_apply_live() {
        let mut app = headless();
        let ctx = Context::default();
        let mut input = RawInput {
            modifiers: Modifiers::COMMAND,
            ..Default::default()
        };
        input.events.push(Event::Key {
            key: Key::Comma,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        });
        run_frame(&mut app, &ctx, input);
        assert!(matches!(app.dialog, Dialog::Preferences));
        for _ in 0..2 {
            run_frame(&mut app, &ctx, RawInput::default());
        }
        assert!(matches!(app.dialog, Dialog::Preferences));
        assert!(ctx
            .memory(|m| m.area_rect(egui::Id::new("cadence-preferences")))
            .is_some());

        // The update checkbox writes the live prefs (headless: never saved).
        let before = app.update.prefs.auto_check;
        app.update.prefs.auto_check = !before;
        run_frame(&mut app, &ctx, RawInput::default());
        assert_eq!(app.update.prefs.auto_check, !before);
        assert!(!app.dirty());

        run_frame(&mut app, &ctx, escape_input());
        assert!(matches!(app.dialog, Dialog::None));
    }

    #[test]
    fn notices_wrap_into_short_lines_without_losing_text() {
        let text = "short\r\n\r\nword word word word wordy\nabcdefghijkl\n";
        let lines = wrap_lines(text, 10);
        assert_eq!(
            lines,
            vec![
                "short",
                "",
                "word word",
                "word word",
                "wordy",
                "abcdefghij",
                "kl"
            ]
        );
        let lines = notices_lines(80);
        assert!(lines.len() > 1000, "{} lines", lines.len());
        assert!(lines.iter().all(|l| l.chars().count() <= 80));
        let words = |s: &str| s.split_whitespace().map(str::len).sum::<usize>();
        let wrapped: usize = lines.iter().map(|l| words(l)).sum();
        // Hard cuts inside a long word split it in two; nothing is dropped.
        assert_eq!(wrapped, words(NOTICES));
        assert!(Arc::ptr_eq(&lines, &notices_lines(80)), "cached per width");
    }

    /// The notices dialog lays out only the rows on screen, not the whole
    /// 1.6 MB text.
    #[test]
    fn notices_dialog_lays_out_only_visible_rows() {
        let mut app = headless();
        app.dialog = Dialog::Notices;
        let ctx = Context::default();
        let mut output = None;
        for _ in 0..4 {
            let input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1360.0, 900.0),
                )),
                ..Default::default()
            };
            output = Some(ctx.run(input, |ctx| app.frame(ctx)));
        }
        let output = output.unwrap();
        let notice_chars: usize = output
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().len()),
                _ => None,
            })
            .sum();
        assert!(matches!(app.dialog, Dialog::Notices));
        assert!(
            notice_chars < 40_000,
            "{notice_chars} chars of text painted in one frame"
        );
    }

    /// Every dialog fits the window and keeps its button row on screen,
    /// however long its body: the notices text once pushed the modal past
    /// the window bottom and hid its Close button. Below the 760 px minimum
    /// window a short dialog may scroll instead, but still fits; the
    /// long-bodied ones scroll only their text.
    #[test]
    fn dialogs_keep_their_buttons_inside_short_windows() {
        fn pack_errors() -> Dialog {
            Dialog::PackErrors((0..200).map(|i| format!("pack {i}: bad field")).collect())
        }
        /// Area id, dialog, and whether its body is long enough to scroll.
        type Case = (&'static str, fn() -> Dialog, bool);
        let dialogs: [Case; 5] = [
            ("cadence-notices", || Dialog::Notices, true),
            ("cadence-help", || Dialog::Help, true),
            ("cadence-pack-errors", pack_errors, true),
            ("cadence-about", || Dialog::About, false),
            (
                "cadence-unsaved",
                || Dialog::Unsaved(PendingAction::Quit),
                false,
            ),
        ];
        for window_h in [480.0, 760.0, 1400.0] {
            for (id, make, long) in dialogs {
                let mut app = headless();
                app.dialog = make();
                let ctx = Context::default();
                for _ in 0..4 {
                    let input = RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1360.0, window_h),
                        )),
                        ..Default::default()
                    };
                    run_frame(&mut app, &ctx, input);
                }
                let area = ctx
                    .memory(|m| m.area_rect(egui::Id::new(id)))
                    .unwrap_or_else(|| panic!("{id} was not shown"));
                assert!(
                    area.top() >= 0.0 && area.bottom() <= window_h,
                    "{id} spans {area:?} in a {window_h} px window"
                );
                if window_h < 760.0 && !long {
                    continue;
                }
                let row = ctx
                    .data(|d| d.get_temp::<egui::Rect>(button_row_probe_id()))
                    .unwrap_or_else(|| panic!("{id} drew no button row"));
                assert!(
                    area.contains_rect(row) && row.bottom() <= window_h,
                    "{id}: buttons at {row:?} outside {area:?} ({window_h} px window)"
                );
            }
        }
    }

    fn close_input() -> RawInput {
        let mut input = RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        input
    }

    fn enter_input() -> RawInput {
        let mut input = RawInput::default();
        input.events.push(Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        input
    }

    fn named(name: &str) -> Project {
        let mut p = Project::demo();
        p.name = name.to_owned();
        p
    }

    /// A headless app whose recovery I/O goes to `dir/recovery`.
    fn with_recovery(dir: &std::path::Path) -> CadenceApp {
        let mut app = CadenceApp::headless(Project::empty());
        app.recovery_dir = Some(dir.join("recovery"));
        app
    }

    fn autosaved_name(dir: &std::path::Path) -> Option<String> {
        persistence::load_recovery_in(&dir.join("recovery"))
            .unwrap()
            .map(|w| w.project.name)
    }

    /// clm_0016: launching with a song file still offers the autosave (on top
    /// of the opened song), and neither a clean close, a save of the opened
    /// song nor an autosave tick deletes or overwrites it before the user
    /// decides. Restore reattaches the file the work came from.
    #[test]
    fn launch_file_still_offers_recovery_and_keeps_autosave() {
        let tmp = tempfile::tempdir().unwrap();
        let song = tmp.path().join("song.cadence");
        persistence::save_project(&named("Saved"), &song).unwrap();
        persistence::save_recovery_in(&tmp.path().join("recovery"), &named("Unsaved"), Some(&song))
            .unwrap();

        let mut app = with_recovery(tmp.path());
        app.startup(Some(song.clone()));
        assert_eq!(app.project.name, "Saved");
        assert_eq!(app.doc.path.as_deref(), Some(song.as_path()));
        assert!(app.recovery_offered);
        match &app.dialog {
            Dialog::Recovery(work) => {
                assert_eq!(work.project.name, "Unsaved");
                assert_eq!(work.source.as_deref(), Some(song.as_path()));
                assert!(work.saved_at.is_some());
            }
            other => panic!("expected the recovery offer, got {other:?}"),
        }

        // A clean close while the offer is pending keeps the autosave.
        let ctx = Context::default();
        run_frame(&mut app, &ctx, close_input());
        assert_eq!(autosaved_name(tmp.path()).as_deref(), Some("Unsaved"));
        // So does saving the opened song, and an autosave tick.
        assert!(app.save_current());
        app.edit("Rename", None, |p| p.name = "Edited".into());
        app.last_edit = None;
        app.autosave(&ctx);
        assert_eq!(autosaved_name(tmp.path()).as_deref(), Some("Unsaved"));
        assert!(matches!(app.dialog, Dialog::Recovery(_)));

        // Enter restores: the recovered work, attached to its song file, dirty.
        run_frame(&mut app, &ctx, enter_input());
        assert!(matches!(app.dialog, Dialog::None));
        assert!(!app.recovery_offered);
        assert_eq!(app.project.name, "Unsaved");
        assert_eq!(app.doc.path.as_deref(), Some(song.as_path()));
        assert!(app.dirty());
        assert_eq!(autosaved_name(tmp.path()).as_deref(), Some("Unsaved"));
    }

    /// A launch file that fails to open still gets the recovery offer.
    #[test]
    fn failed_launch_open_still_offers_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        persistence::save_recovery_in(&tmp.path().join("recovery"), &named("Lost"), None).unwrap();
        let mut app = with_recovery(tmp.path());
        app.startup(Some(tmp.path().join("missing.cadence")));
        assert!(app.recovery_offered);
        match &app.dialog {
            Dialog::Recovery(work) => {
                assert_eq!(work.project.name, "Lost");
                assert_eq!(work.source, None);
            }
            other => panic!("expected the recovery offer, got {other:?}"),
        }
    }

    /// Without an autosave nothing is offered; once the user decided
    /// (Discard), a clean close clears as before.
    #[test]
    fn discard_then_clean_close_clears_autosave() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = with_recovery(tmp.path());
        app.startup(None);
        assert!(!app.recovery_offered);
        assert!(matches!(app.dialog, Dialog::None));

        persistence::save_recovery_in(&tmp.path().join("recovery"), &named("Old"), None).unwrap();
        app.check_recovery();
        assert!(app.recovery_offered);
        app.dialog = Dialog::None;
        app.recovery_offered = false;
        let ctx = Context::default();
        run_frame(&mut app, &ctx, close_input());
        assert_eq!(autosaved_name(tmp.path()), None);
    }

    #[test]
    fn autosave_backoff_doubles_to_a_cap() {
        let secs: Vec<u64> = (1..=8)
            .map(|n| super::super::autosave_backoff(n).as_secs())
            .collect();
        assert_eq!(secs, [2, 4, 8, 16, 32, 60, 60, 60]);
    }

    /// clm_0018: a failed autosave is retried (after a backoff) for the same
    /// document state, warns once per failure streak, keeps an indicator up
    /// and recovers on its own once the folder is writable again.
    #[test]
    fn failed_autosave_is_retried_and_warns_once() {
        let tmp = tempfile::tempdir().unwrap();
        // A plain file where the recovery folder's parent should be makes
        // every write fail until it is removed.
        let blocker = tmp.path().join("blocked");
        std::fs::write(&blocker, b"not a folder").unwrap();
        let mut app = with_recovery(&blocker);
        let ctx = Context::default();
        app.edit("Rename", None, |p| p.name = "Precious".into());
        let state = app.history.state();
        app.last_edit = None;

        app.autosave(&ctx);
        assert_eq!(app.autosave_failures, 1);
        assert_ne!(app.autosaved_state, state, "a failed write must not count");
        assert!(app.autosave_retry_at.is_some());
        let warned = app.status.clone().unwrap();
        assert_eq!(warned.kind, StatusKind::Warning);
        assert!(
            warned.text.starts_with("Autosave failed"),
            "{}",
            warned.text
        );

        // Inside the backoff window nothing is attempted.
        app.autosave(&ctx);
        assert_eq!(app.autosave_failures, 1);

        // A second failure does not flood the status line.
        app.set_status(StatusKind::Info, "Other news");
        app.autosave_retry_at = None;
        app.autosave(&ctx);
        assert_eq!(app.autosave_failures, 2);
        assert_eq!(app.status.as_ref().unwrap().text, "Other news");
        assert!(app.autosave_error.is_some());

        // The folder becomes writable: the same state is saved on retry.
        std::fs::remove_file(&blocker).unwrap();
        app.autosave_retry_at = None;
        app.autosave(&ctx);
        assert_eq!(app.autosave_failures, 0);
        assert_eq!(app.autosaved_state, state);
        assert_eq!(autosaved_name(&blocker).as_deref(), Some("Precious"));
        assert_eq!(
            app.status.as_ref().unwrap().text,
            "Autosave is working again."
        );
    }

    /// Merely rendering a modal must not mutate the song or its undo history.
    #[test]
    fn showing_dialogs_leaves_project_untouched() {
        let mut app = headless();
        app.edit("Rename", None, |p| p.name.push('!'));
        let project = app.project.clone();
        let state = app.history.state();
        app.dialog = Dialog::Unsaved(PendingAction::New);

        let ctx = Context::default();
        for _ in 0..3 {
            run_frame(&mut app, &ctx, RawInput::default());
        }
        assert_eq!(app.project, project);
        assert_eq!(app.history.state(), state);
        assert!(app.dirty());
    }

    #[test]
    fn gestures_parse_their_modifiers() {
        assert_eq!(
            split_gesture("{alt}+drag a clip"),
            (vec![Modifiers::ALT], "drag a clip")
        );
        assert_eq!(
            split_gesture("{cmd}+{shift}+wheel"),
            (vec![Modifiers::COMMAND, Modifiers::SHIFT], "wheel")
        );
        assert_eq!(split_gesture("Wheel"), (vec![], "Wheel"));
        // Every placeholder in the Help is one the parser knows.
        for page in HelpPage::ALL {
            for section in page.sections() {
                for (label, keys) in section.rows {
                    assert!(!label.contains('{'), "{label}");
                    if let Keys::Mouse(text) = keys {
                        for alt in text.split(" / ") {
                            let (_, action) = split_gesture(alt);
                            assert!(
                                !action.contains('{')
                                    && !action.contains('}')
                                    && !action.is_empty(),
                                "{label}: {text}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Help is the single list of shortcuts: every app shortcut is in it.
    #[test]
    fn help_lists_every_shortcut() {
        use crate::gui as g;
        let mut listed: Vec<KeyboardShortcut> = Vec::new();
        for page in HelpPage::ALL {
            for section in page.sections() {
                for (_, keys) in section.rows {
                    match keys {
                        Keys::Shortcut(sc) => listed.push(**sc),
                        Keys::Either(a, b) => listed.extend([**a, **b]),
                        _ => {}
                    }
                }
            }
        }
        for sc in [
            g::SC_NEW,
            g::SC_OPEN,
            g::SC_SAVE,
            g::SC_SAVE_AS,
            g::SC_EXPORT_WAV,
            g::SC_QUIT,
            g::SC_UNDO,
            g::SC_REDO,
            g::SC_REDO_ALT,
            g::SC_DUPLICATE,
            g::SC_COPY,
            g::SC_PASTE,
            g::SC_CUT,
            g::SC_SELECT_ALL,
            g::SC_EDITOR,
            g::SC_MIXER,
            g::SC_KEYS,
            g::SC_HELP,
            g::SC_PLAY,
            g::SC_HOME,
            g::SC_LOOP,
            g::SC_MAX_EDITOR,
            g::SC_MAX_ARRANGER,
            g::SC_RECORD,
        ] {
            assert!(listed.contains(&sc), "{sc:?} is missing from Help");
        }
    }

    /// Every Help page renders in every theme, at the smallest window, with
    /// the platform's modifier names.
    #[test]
    fn help_pages_render_with_platform_modifier_names() {
        let ctx = Context::default();
        ctx.set_os(egui::os::OperatingSystem::Windows);
        let _ = ctx.run(RawInput::default(), |_| {});
        assert_eq!(ctx.format_modifiers(Modifiers::COMMAND), "Ctrl");
        assert_eq!(ctx.format_modifiers(Modifiers::ALT), "Alt");
        assert_eq!(
            ctx.format_shortcut(&super::super::SC_SAVE_AS),
            "Ctrl+Shift+S"
        );
        let mac = Context::default();
        mac.set_os(egui::os::OperatingSystem::Mac);
        let _ = mac.run(RawInput::default(), |_| {});
        // ⌘ once a font has the glyph (Cadence bundles one), else "Cmd".
        assert!(["⌘", "Cmd"].contains(&mac.format_modifiers(Modifiers::COMMAND).as_str()));
        for theme in crate::gui::theme::ThemeChoice::ALL {
            for page in HelpPage::ALL {
                let mut app = headless();
                app.theme = theme;
                app.dialog = Dialog::Help;
                let ctx = Context::default();
                ctx.set_os(egui::os::OperatingSystem::Windows);
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("cadence-help-page"), page));
                for _ in 0..3 {
                    let input = RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            vec2(1100.0, 760.0),
                        )),
                        ..Default::default()
                    };
                    run_frame(&mut app, &ctx, input);
                }
                assert!(matches!(app.dialog, Dialog::Help), "{theme:?} {page:?}");
            }
        }
    }
}
