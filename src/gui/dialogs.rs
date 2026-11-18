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

use egui::{
    vec2, Align, Color32, Context, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui,
};

use super::files::PendingAction;
use super::ops::{self, ArpDirection, ArpSpec};
use super::theme::{self, Palette};
use super::{CadenceApp, StatusKind};
use crate::model::{
    note_name, ChordQuality, KeyMode, Project, SongKey, MAX_NAME_CHARS, NOTE_NAMES,
};
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
    /// A file action waiting on a save / don't-save / cancel decision.
    Unsaved(PendingAction),
    /// Launch-time offer to restore the crash-recovery autosave.
    Recovery(Box<Project>),
    /// Non-fatal instrument-pack load problems.
    PackErrors(Vec<String>),
    /// Name + key for a new song (after any unsaved-changes decision).
    NewSong(NewSongDraft),
    /// Chord arpeggio generator for the selected track's current pattern.
    Arp(ArpDraft),
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
            key.degree_chord_quality(d).symbol(key.degree_root(d))
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
        for (pc, name) in NOTE_NAMES.iter().enumerate() {
            let on = key.root as usize == pc;
            let tip = format!("Key of {name}");
            if theme::chip(ui, pal, name, on, pal.accent, vec2(30.0, 24.0), &tip).clicked() && !on {
                key.root = pc as u8;
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
            Dialog::Unsaved(action) => self.unsaved_dialog(ctx, *action),
            Dialog::Recovery(project) => self.recovery_dialog(ctx, project),
            Dialog::PackErrors(errors) => self.pack_errors_dialog(ctx, errors),
            Dialog::NewSong(draft) => self.new_song_dialog(ctx, draft),
            Dialog::Arp(draft) => self.arp_dialog(ctx, draft),
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
    /// pending action is never lost silently.
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
        let resp = modal("cadence-unsaved", pal).show(ctx, |ui| {
            ui.set_min_width(410.0);
            header(ui, pal, "UNSAVED CHANGES", "Save your work first?");
            ui.add_space(6.0);
            match &path {
                Some(p) => {
                    ui.label(RichText::new(&name).color(pal.text));
                    ui.label(
                        RichText::new(p.display().to_string())
                            .small()
                            .color(pal.text_faint),
                    );
                }
                None => {
                    ui.label(RichText::new(format!("{name} (not saved yet)")).color(pal.text));
                }
            }
            ui.add_space(4.0);
            let verb = if action == PendingAction::Quit {
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
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, pal, "Save").clicked() {
                    choice = Some(Choice::Save);
                }
                if ui
                    .add(egui::Button::new("Don't Save").min_size(vec2(96.0, 30.0)))
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
    /// to an accidental dismissal. `handle_close_request` preserves the file
    /// while `recovery_offered` is set, so the user can also simply close the
    /// app and decide next launch.
    fn recovery_dialog(&mut self, ctx: &Context, project: &Project) -> bool {
        let pal = self.palette();
        #[derive(Clone, Copy)]
        enum Choice {
            Restore,
            Discard,
        }
        let mut choice = None;
        let summary = format!(
            "{}\n{} track{} · {} bars · {:.0} BPM",
            project.name,
            project.tracks.len(),
            if project.tracks.len() == 1 { "" } else { "s" },
            project.bars,
            project.bpm,
        );
        let _resp = modal("cadence-recovery", pal).show(ctx, |ui| {
            ui.set_min_width(430.0);
            header(ui, pal, "RECOVERY", "Restore unsaved work?");
            ui.add_space(6.0);
            ui.label(RichText::new("Cadence found an autosave from a previous session:").color(pal.text_dim));
            ui.add_space(8.0);
            ui.label(RichText::new(&summary).color(pal.text));
            ui.add_space(4.0);
            ui.label(RichText::new("Restoring replaces the song currently open.").small().color(pal.text_faint));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, pal, "Restore Work").clicked() {
                    choice = Some(Choice::Restore);
                }
                if ui
                    .add(egui::Button::new("Discard").min_size(vec2(96.0, 30.0)))
                    .on_hover_text("Delete the autosave and keep the current song")
                    .clicked()
                {
                    choice = Some(Choice::Discard);
                }
            });
            ui.add_space(6.0);
            ui.label(RichText::new("This offer stays until you choose — closing Cadence keeps the autosave for next time.").small().color(pal.text_faint));
        });
        match choice {
            Some(Choice::Restore) => {
                let project = project.clone();
                self.recovery_offered = false;
                self.replace_project(project, None, false);
                self.set_status(
                    StatusKind::Success,
                    "Restored the recovered autosave — Save As… to keep it.",
                );
                false
            }
            Some(Choice::Discard) => {
                if self.io_enabled {
                    match persistence::clear_recovery() {
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
        let resp = modal("cadence-new-song", pal).show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.set_max_width(460.0);
            header(ui, pal, "NEW SONG", "Start a new song");
            ui.add_space(8.0);
            ui.label(theme::kicker_text(pal, "NAME"));
            ui.add_space(3.0);
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .desired_width(f32::INFINITY)
                    .hint_text("Song name")
                    .char_limit(MAX_NAME_CHARS),
            );
            ui.add_space(12.0);
            ui.label(theme::kicker_text(pal, "KEY"));
            ui.add_space(3.0);
            key_picker(ui, pal, &mut draft.key);
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "{} — the Arpeggiator offers this key's chords. You can change it later.",
                    draft.key.name()
                ))
                .small()
                .color(pal.text_faint),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
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
                "Select a melodic track with a pattern to arpeggiate.",
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
                "Select a melodic track with a pattern to arpeggiate.",
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
                    spec.quality.symbol(spec.chord_root),
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
            self.set_status(StatusKind::Info, format!("Song key: {}.", key.name()));
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
        let resp = modal("cadence-arp", pal).show(ctx, |ui| {
            ui.set_min_width(470.0);
            ui.set_max_width(490.0);
            header(ui, pal, "ARPEGGIATOR", "Arpeggiate a chord");
            ui.label(RichText::new(&target).small().color(pal.text_faint));
            ui.add_space(10.0);

            ui.label(theme::kicker_text(pal, "SONG KEY"));
            ui.add_space(3.0);
            key_picker(ui, pal, &mut new_key);
            ui.add_space(12.0);

            ui.label(theme::kicker_text(pal, "CHORD"));
            ui.add_space(3.0);
            egui::Grid::new("arp-chord")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Chord").color(pal.text_dim));
                    egui::ComboBox::from_id_salt("arp-degree")
                        .width(190.0)
                        .selected_text(degree_label(key, draft.degree))
                        .show_ui(ui, |ui| {
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
                        });
                    ui.end_row();
                    if draft.degree.is_none() {
                        ui.label(RichText::new("Root").color(pal.text_dim));
                        egui::ComboBox::from_id_salt("arp-root")
                            .width(190.0)
                            .selected_text(NOTE_NAMES[draft.custom_root as usize % 12])
                            .show_ui(ui, |ui| {
                                for (pc, name) in NOTE_NAMES.iter().enumerate() {
                                    ui.selectable_value(&mut draft.custom_root, pc as u8, *name);
                                }
                            });
                        ui.end_row();
                    }
                    ui.label(RichText::new("Type").color(pal.text_dim));
                    egui::ComboBox::from_id_salt("arp-quality")
                        .width(190.0)
                        .selected_text(draft.quality.label())
                        .show_ui(ui, |ui| {
                            for q in ChordQuality::ALL {
                                ui.selectable_value(&mut draft.quality, q, q.label());
                            }
                        });
                    ui.end_row();
                    ui.label(RichText::new("Start octave").color(pal.text_dim));
                    egui::ComboBox::from_id_salt("arp-octave")
                        .width(190.0)
                        .selected_text(format!("Octave {}", draft.octave))
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
            ui.add_space(12.0);

            ui.label(theme::kicker_text(pal, "PATTERN"));
            ui.add_space(3.0);
            egui::Grid::new("arp-pattern")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("Direction").color(pal.text_dim));
                    egui::ComboBox::from_id_salt("arp-direction")
                        .width(190.0)
                        .selected_text(draft.direction.label())
                        .show_ui(ui, |ui| {
                            for d in ArpDirection::ALL {
                                ui.selectable_value(&mut draft.direction, d, d.label());
                            }
                        });
                    ui.end_row();
                    ui.label(RichText::new("Octaves").color(pal.text_dim));
                    ui.add(egui::Slider::new(
                        &mut draft.octaves,
                        1..=ops::MAX_ARP_OCTAVES,
                    ));
                    ui.end_row();
                    ui.label(RichText::new("Rate").color(pal.text_dim));
                    let rate = ARP_RATES
                        .iter()
                        .find(|(r, _)| *r == draft.rate_steps)
                        .map_or("", |(_, l)| l);
                    egui::ComboBox::from_id_salt("arp-rate")
                        .width(190.0)
                        .selected_text(rate)
                        .show_ui(ui, |ui| {
                            for (r, label) in ARP_RATES {
                                ui.selectable_value(&mut draft.rate_steps, r, label);
                            }
                        });
                    ui.end_row();
                    ui.label(RichText::new("Velocity").color(pal.text_dim));
                    ui.add(
                        egui::Slider::new(&mut draft.velocity, 0.2..=1.0)
                            .custom_formatter(|x, _| format!("{:.0}%", x * 100.0)),
                    );
                    ui.end_row();
                });
            ui.add_space(6.0);
            ui.checkbox(&mut draft.accent, "Accent the first note of each bar");
            ui.checkbox(&mut draft.replace, "Replace the pattern's notes");
            ui.add_space(8.0);
            let spec = draft.spec(key);
            let tones: Vec<String> = ops::arp_tones(&spec).into_iter().map(note_name).collect();
            ui.label(
                RichText::new(format!(
                    "{}: {}",
                    spec.quality.symbol(spec.chord_root),
                    tones.join(" ")
                ))
                .small()
                .color(pal.text_dim),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, pal, "Generate").clicked() {
                    generate = true;
                }
                if theme::secondary_button(ui, "Done").clicked() {
                    done = true;
                }
            });
        });
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

    /// Shortcuts and concrete editing instructions (F1 / Help menu).
    fn help_dialog(&mut self, ctx: &Context) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let resp = modal("cadence-help", pal).show(ctx, |ui| {
            ui.set_min_width(580.0);
            ui.set_max_width(580.0);
            header(ui, pal, "SHORTCUTS & TIPS", "How to write a song");
            ui.add_space(6.0);
            ScrollArea::vertical().max_height(430.0).auto_shrink([false, true]).show(ui, |ui| {
                help_section(ui, pal, ctx, "FILE", &[
                    ("New song", Some(&super::SC_NEW)),
                    ("New from demo", None),
                    ("Open…", Some(&super::SC_OPEN)),
                    ("Save", Some(&super::SC_SAVE)),
                    ("Save as…", Some(&super::SC_SAVE_AS)),
                    ("Import MIDI…", None),
                    ("Export MIDI…", None),
                    ("Export WAV…", Some(&super::SC_EXPORT_WAV)),
                    ("Quit", Some(&super::SC_QUIT)),
                ]);
                help_section(ui, pal, ctx, "EDIT", &[
                    ("Undo", Some(&super::SC_UNDO)),
                    ("Redo", Some(&super::SC_REDO)),
                    ("Duplicate selection", Some(&super::SC_DUPLICATE)),
                    ("Delete selection", None),
                    ("Select all notes", Some(&super::SC_SELECT_ALL)),
                    ("Deselect", None),
                    ("Cut / copy / paste", None),
                    ("Arpeggiator…", None),
                ]);
                help_section(ui, pal, ctx, "TRANSPORT", &[
                    ("Play / pause", None),
                    ("Stop", None),
                    ("Return to start", None),
                    ("Loop song", None),
                    ("Metronome", None),
                ]);
                help_section(ui, pal, ctx, "VIEW", &[
                    ("Pattern editor", Some(&super::SC_EDITOR)),
                    ("Mixer", Some(&super::SC_MIXER)),
                    ("Zoom in / out", None),
                    ("Fit song", None),
                    ("Follow playhead", None),
                ]);
                help_section_text(ui, pal, "COMPUTER KEYBOARD", &[
                    ("White keys C D E F G A B", "Z X C V B N M"),
                    ("Black keys C# D# F# G# A#", "S D G H J"),
                    ("Octave down / up", "Q / W"),
                    ("One octave up, that note only", "Shift + key"),
                ]);
                ui.add_space(10.0);
                ui.label(theme::kicker_text(pal, "EDITING"));
                ui.add_space(4.0);
                for tip in [
                    "Arranger: drag empty lane space to box-select clips across tracks; drag any selected clip to move the group, or Alt-drag to copy it. Clicking an unselected clip selects it alone (clicking a selected clip keeps the group); Shift-click toggles it, double-click empty lane space places the selected pattern, clip top corners set fades, and the right edge changes loops.",
                    "Piano roll (melodic tracks): click empty grid to draw a note, drag empty grid to box-select notes, drag any selected note to move the group, pull the right edge to resize, and right-click erases. Drag the velocity lane under the roll to shape note loudness.",
                    "Computer keyboard: with the pattern editor open, the Z–M row plus S/D/G/H/J play the selected track — white keys C–B, black keys the sharps — on a fixed-length audition (drum tracks play their kit pieces instead). Q/W shift the octave, Shift raises it a step, and the keyboard strip under the roll shows what's held.",
                    "Chord strip (above the roll): click a diatonic chord of the song key to stamp it at the pointer or playhead — as a block or a strum in either direction at four spacings, in any of 18 chord qualities via the Type menu. Hovering a chord auditions it; the Scale toggle tints the key's notes in the roll.",
                    "Drum grid (drum tracks): click or drag across the kit rows — the eight built-in drums, or one row per piece for a drum-kit preset — to paint hits; Shift+click cycles accent levels; right-click erases. The Fill button adds a four-on-the-floor hit on the top row.",
                    "Mixer: drag channel faders for volume and pan; turn the knobs for filter, reverb, delay and drive. Mute/solo per channel; the rightmost strip is the master.",
                    "Songs save as .cadence JSON. MIDI and WAV export live in the File menu. Instrument packs are plain JSON — Import Pack… in the library adds new synth sounds with no rebuild.",
                ] {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new("•").color(pal.accent));
                        ui.label(RichText::new(tip).small().color(pal.text_dim));
                    });
                    ui.add_space(4.0);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, pal, "Done").clicked() {
                        closed = true;
                    }
                });
            });
        });
        !closed && !resp.should_close()
    }

    // ----- about ---------------------------------------------------------------

    /// Honest about-box: what Cadence is, what it isn't (no plugins yet).
    fn about_dialog(&mut self, ctx: &Context) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let resp =
            modal("cadence-about", pal).show(ctx, |ui| {
                ui.set_min_width(430.0);
                header(ui, pal, "CADENCE", "A native music sequencer");
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Version {}", crate::APP_VERSION))
                        .small()
                        .color(pal.text_faint),
                );
                ui.add_space(10.0);
                ui.label(RichText::new(
                    "Cadence is written entirely in Rust: an eframe/egui interface, CPAL audio \
                     and pure-Rust synthesis and effects. It runs fully offline — no account, \
                     no cloud, no internet connection needed.",
                ).color(pal.text_dim));
                ui.add_space(8.0);
                ui.label(RichText::new(
                    "Songs are .cadence JSON files you can keep and share. MIDI and WAV export \
                     are in the File menu. Instrument packs add new synth presets as plain \
                     JSON — use Import Pack… in the library; no rebuild needed.",
                ).color(pal.text_dim));
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Plugin instruments (VST3, AU, CLAP) are planned but not supported yet.",
                    )
                    .small()
                    .color(pal.text_faint),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::primary_button(ui, pal, "Close").clicked() {
                            closed = true;
                        }
                    });
                });
            });
        !closed && !resp.should_close()
    }

    // ----- pack errors ---------------------------------------------------------

    /// Lists non-fatal instrument-pack load problems.
    fn pack_errors_dialog(&mut self, ctx: &Context, errors: &[String]) -> bool {
        let pal = self.palette();
        let mut closed = false;
        let dir = crate::packs::user_packs_dir();
        let resp = modal("cadence-pack-errors", pal).show(ctx, |ui| {
            ui.set_min_width(520.0);
            ui.set_max_width(560.0);
            header(ui, pal, "INSTRUMENT PACKS", "Some packs could not be loaded");
            ui.add_space(4.0);
            ui.label(RichText::new("Cadence keeps working with the packs that did load. Fix or remove these files, then press Reload.").small().color(pal.text_dim));
            ui.add_space(8.0);
            ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                for err in errors {
                    ui.label(RichText::new(err).small().color(pal.danger));
                    ui.add_space(3.0);
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Packs folder: {}", dir.display())).small().color(pal.text_faint));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, pal, "Close").clicked() {
                        closed = true;
                    }
                });
            });
        });
        !closed && !resp.should_close()
    }
}

/// Centered modal with the app's styling and a dimmed backdrop.
fn modal(id: &str, pal: Palette) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            Frame::new()
                .fill(pal.panel)
                .stroke(Stroke::new(1.0f32, pal.border))
                .corner_radius(12)
                .inner_margin(Margin::same(18)),
        )
}

/// Kicker + title header shared by all dialogs.
fn header(ui: &mut Ui, pal: Palette, kicker: &str, title: &str) {
    theme::section_header(ui, pal, kicker, title);
}

/// One Help row: label on the left, shortcut (or menu path) on the right.
fn help_section(
    ui: &mut Ui,
    pal: Palette,
    ctx: &Context,
    title: &str,
    rows: &[(&str, Option<&egui::KeyboardShortcut>)],
) {
    ui.add_space(8.0);
    ui.label(theme::kicker_text(pal, title));
    ui.add_space(3.0);
    for (label, sc) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                vec2(190.0, f32::INFINITY),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(RichText::new(*label).color(pal.text_dim))
                        .on_hover_text(*label);
                },
            );
            let right = match sc {
                Some(sc) => ctx.format_shortcut(sc),
                // Title-cased section name: "EDIT" -> "Edit menu".
                None => {
                    let mut chars = title.chars();
                    let first = chars.next().map(|c| c.to_string()).unwrap_or_default();
                    format!("{first}{} menu", chars.as_str().to_lowercase())
                }
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(right).small().color(pal.text_faint));
            });
        });
    }
}

/// Same layout as [`help_section`] but with literal right-column text — for
/// multi-key mappings that aren't single shortcuts.
fn help_section_text(ui: &mut Ui, pal: Palette, title: &str, rows: &[(&str, &str)]) {
    ui.add_space(8.0);
    ui.label(theme::kicker_text(pal, title));
    ui.add_space(3.0);
    for (label, right) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                vec2(190.0, f32::INFINITY),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(RichText::new(*label).color(pal.text_dim))
                        .on_hover_text(*label);
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(*right).small().color(pal.text_faint));
            });
        });
    }
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
        app.dialog = Dialog::Recovery(Box::new(Project::demo()));
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
}
