//! Modal dialogs: the unsaved-changes guard, Help, About, the launch-time
//! recovery offer and the instrument-pack error list.
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
use super::theme::{self, BORDER, CORAL, LIME, PANEL, TEXT, TEXT_DIM, TEXT_FAINT};
use super::{CadenceApp, StatusKind};
use crate::model::Project;
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
    Recovery(Project),
    /// Non-fatal instrument-pack load problems.
    PackErrors(Vec<String>),
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
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if matches!(self.dialog, Dialog::None) {
            return;
        }
        let dialog = std::mem::replace(&mut self.dialog, Dialog::None);
        let keep = match &dialog {
            Dialog::None => false,
            Dialog::Help => self.help_dialog(ctx),
            Dialog::About => self.about_dialog(ctx),
            Dialog::Unsaved(action) => self.unsaved_dialog(ctx, *action),
            Dialog::Recovery(project) => self.recovery_dialog(ctx, project),
            Dialog::PackErrors(errors) => self.pack_errors_dialog(ctx, errors),
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
        #[derive(Clone, Copy)]
        enum Choice {
            Save,
            Discard,
            Cancel,
        }
        let mut choice = None;
        let name = self.project.name.clone();
        let path = self.doc.path.clone();
        let resp = modal("cadence-unsaved").show(ctx, |ui| {
            ui.set_min_width(410.0);
            header(ui, "UNSAVED CHANGES", "Save your work first?");
            ui.add_space(6.0);
            match &path {
                Some(p) => {
                    ui.label(RichText::new(&name).color(TEXT));
                    ui.label(
                        RichText::new(p.display().to_string())
                            .small()
                            .color(TEXT_FAINT),
                    );
                }
                None => {
                    ui.label(RichText::new(format!("{name} (not saved yet)")).color(TEXT));
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
                .color(TEXT_DIM),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "Save").clicked() {
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
        let _resp = modal("cadence-recovery").show(ctx, |ui| {
            ui.set_min_width(430.0);
            header(ui, "RECOVERY", "Restore unsaved work?");
            ui.add_space(6.0);
            ui.label(RichText::new("Cadence found an autosave from a previous session:").color(TEXT_DIM));
            ui.add_space(8.0);
            ui.label(RichText::new(&summary).color(TEXT));
            ui.add_space(4.0);
            ui.label(RichText::new("Restoring replaces the song currently open.").small().color(TEXT_FAINT));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if theme::primary_button(ui, "Restore Work").clicked() {
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
            ui.label(RichText::new("This offer stays until you choose — closing Cadence keeps the autosave for next time.").small().color(TEXT_FAINT));
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

    // ----- help ---------------------------------------------------------------

    /// Shortcuts and concrete editing instructions (F1 / Help menu).
    fn help_dialog(&mut self, ctx: &Context) -> bool {
        let mut closed = false;
        let resp = modal("cadence-help").show(ctx, |ui| {
            ui.set_min_width(580.0);
            ui.set_max_width(580.0);
            header(ui, "SHORTCUTS & TIPS", "How to write a song");
            ui.add_space(6.0);
            ScrollArea::vertical().max_height(430.0).auto_shrink([false, true]).show(ui, |ui| {
                help_section(ui, ctx, "FILE", &[
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
                help_section(ui, ctx, "EDIT", &[
                    ("Undo", Some(&super::SC_UNDO)),
                    ("Redo", Some(&super::SC_REDO)),
                    ("Duplicate selection", Some(&super::SC_DUPLICATE)),
                    ("Delete selection", None),
                    ("Select all notes", Some(&super::SC_SELECT_ALL)),
                    ("Deselect", None),
                ]);
                help_section(ui, ctx, "TRANSPORT", &[
                    ("Play / pause", None),
                    ("Stop", None),
                    ("Return to start", None),
                    ("Loop song", None),
                    ("Metronome", None),
                ]);
                help_section(ui, ctx, "VIEW", &[
                    ("Pattern editor", Some(&super::SC_EDITOR)),
                    ("Mixer", Some(&super::SC_MIXER)),
                    ("Zoom in / out", None),
                    ("Fit song", None),
                    ("Follow playhead", None),
                ]);
                ui.add_space(10.0);
                ui.label(theme::kicker_text("EDITING"));
                ui.add_space(4.0);
                for tip in [
                    "Arranger: drag empty lane space to box-select clips across tracks; drag any selected clip to move the group, or Alt-drag to copy it. Clicking an unselected clip selects it alone (clicking a selected clip keeps the group); Shift-click toggles it, double-click empty lane space places the selected pattern, clip top corners set fades, and the right edge changes loops.",
                    "Piano roll (melodic tracks): click empty grid to draw a note, drag empty grid to box-select notes, drag any selected note to move the group, pull the right edge to resize, and right-click erases. Drag the velocity lane under the roll to shape note loudness.",
                    "Drum grid (drum tracks): click or drag across the eight kit rows to paint hits; Shift+click cycles accent levels; right-click erases. The Fill button adds a four-on-the-floor kick.",
                    "Mixer: drag channel faders for volume and pan; turn the knobs for filter, reverb, delay and drive. Mute/solo per channel; the rightmost strip is the master.",
                    "Songs save as .cadence JSON. MIDI and WAV export live in the File menu. Instrument packs are plain JSON — Import Pack… in the library adds new synth sounds with no rebuild.",
                ] {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new("•").color(LIME));
                        ui.label(RichText::new(tip).small().color(TEXT_DIM));
                    });
                    ui.add_space(4.0);
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, "Done").clicked() {
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
        let mut closed = false;
        let resp =
            modal("cadence-about").show(ctx, |ui| {
                ui.set_min_width(430.0);
                header(ui, "CADENCE", "A native music sequencer");
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Version {}", crate::APP_VERSION))
                        .small()
                        .color(TEXT_FAINT),
                );
                ui.add_space(10.0);
                ui.label(RichText::new(
                    "Cadence is written entirely in Rust: an eframe/egui interface, CPAL audio \
                     and pure-Rust synthesis and effects. It runs fully offline — no account, \
                     no cloud, no internet connection needed.",
                ).color(TEXT_DIM));
                ui.add_space(8.0);
                ui.label(RichText::new(
                    "Songs are .cadence JSON files you can keep and share. MIDI and WAV export \
                     are in the File menu. Instrument packs add new synth presets as plain \
                     JSON — use Import Pack… in the library; no rebuild needed.",
                ).color(TEXT_DIM));
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "Plugin instruments (VST3, AU, CLAP) are planned but not supported yet.",
                    )
                    .small()
                    .color(TEXT_FAINT),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if theme::primary_button(ui, "Close").clicked() {
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
        let mut closed = false;
        let dir = crate::packs::user_packs_dir();
        let resp = modal("cadence-pack-errors").show(ctx, |ui| {
            ui.set_min_width(520.0);
            ui.set_max_width(560.0);
            header(ui, "INSTRUMENT PACKS", "Some packs could not be loaded");
            ui.add_space(4.0);
            ui.label(RichText::new("Cadence keeps working with the packs that did load. Fix or remove these files, then press Reload.").small().color(TEXT_DIM));
            ui.add_space(8.0);
            ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                for err in errors {
                    ui.label(RichText::new(err).small().color(CORAL));
                    ui.add_space(3.0);
                }
            });
            ui.add_space(8.0);
            ui.label(RichText::new(format!("Packs folder: {}", dir.display())).small().color(TEXT_FAINT));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, "Close").clicked() {
                        closed = true;
                    }
                });
            });
        });
        !closed && !resp.should_close()
    }
}

/// Centered modal with the app's dark styling and a dimmed backdrop.
fn modal(id: &str) -> egui::Modal {
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(
            Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0f32, BORDER))
                .corner_radius(12)
                .inner_margin(Margin::same(18)),
        )
}

/// Kicker + title header shared by all dialogs.
fn header(ui: &mut Ui, kicker: &str, title: &str) {
    theme::section_header(ui, kicker, title);
}

/// One Help row: label on the left, shortcut (or menu path) on the right.
fn help_section(
    ui: &mut Ui,
    ctx: &Context,
    title: &str,
    rows: &[(&str, Option<&egui::KeyboardShortcut>)],
) {
    ui.add_space(8.0);
    ui.label(theme::kicker_text(title));
    ui.add_space(3.0);
    for (label, sc) in rows {
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                vec2(190.0, f32::INFINITY),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(RichText::new(*label).color(TEXT_DIM))
                        .on_hover_text(*label);
                },
            );
            let right = match sc {
                Some(sc) => ctx.format_shortcut(sc),
                None => "File menu".to_owned(),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(right).small().color(TEXT_FAINT));
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
        app.dialog = Dialog::Recovery(Project::demo());
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
