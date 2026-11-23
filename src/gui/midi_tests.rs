//! Headless tests for hardware MIDI input in the GUI: controller messages
//! are injected as [`MidiInputEvent`]s (what the input thread would send),
//! so no MIDI device or system MIDI service is needed.

use egui::{Pos2, RawInput, Rect};

use super::dialogs::{midi_input_note, Dialog};
use super::editor::NoteSource;
use super::{CadenceApp, StatusKind};
use crate::midi_in::{CloseReason, MidiInputEvent, MidiInputPref, MidiMessage};
use crate::model::{Instrument, Project};

fn frame(ctx: &egui::Context, app: &mut CadenceApp) {
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1360.0, 860.0))),
        focused: true,
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| app.frame(ctx));
}

/// A headless app with one selected piano track; returns it and the track
/// index.
fn app_with_track() -> (egui::Context, CadenceApp, usize) {
    let ctx = egui::Context::default();
    let mut app = CadenceApp::headless(Project::empty());
    frame(&ctx, &mut app);
    app.add_track(Instrument::Piano);
    let idx = app.project.tracks.len() - 1;
    app.select_track(app.project.tracks[idx].id);
    frame(&ctx, &mut app);
    assert!(app.midi_in.is_none(), "headless never opens MIDI");
    (ctx, app, idx)
}

fn send(app: &mut CadenceApp, message: MidiMessage) {
    app.midi_input_event(MidiInputEvent::Message(message));
}
fn on(app: &mut CadenceApp, pitch: u8, velocity: u8) {
    send(
        app,
        MidiMessage::NoteOn {
            channel: 0,
            pitch,
            velocity,
        },
    );
}
fn off(app: &mut CadenceApp, pitch: u8) {
    send(app, MidiMessage::NoteOff { channel: 0, pitch });
}
fn pedal(app: &mut CadenceApp, down: bool) {
    send(app, MidiMessage::Sustain { channel: 9, down });
}

fn notes(app: &CadenceApp, idx: usize) -> Vec<crate::model::Note> {
    let mut notes = app.project.tracks[idx].patterns[0].notes.clone();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    notes
}

#[test]
fn controller_notes_play_through_without_recording() {
    let (_ctx, mut app, idx) = app_with_track();
    on(&mut app, 60, 100);
    off(&mut app, 60);
    assert!(notes(&app, idx).is_empty(), "disarmed: nothing recorded");
    assert!(app.editor.record_pending.is_empty());
}

#[test]
fn live_recording_takes_controller_velocity_and_length() {
    let (_ctx, mut app, idx) = app_with_track();
    app.toggle_record();
    app.transport.playing = true;
    app.transport.position = 2.3;
    on(&mut app, 64, 127);
    app.transport.position = 3.2;
    on(&mut app, 67, 32);
    app.transport.position = 6.5;
    off(&mut app, 64);
    off(&mut app, 67);
    app.stop();
    let n = notes(&app, idx);
    assert_eq!(n.len(), 2);
    assert_eq!((n[0].pitch, n[0].start, n[0].duration), (64, 2.0, 5.0));
    assert_eq!(n[0].velocity, 1.0);
    assert_eq!((n[1].pitch, n[1].start, n[1].duration), (67, 3.0, 4.0));
    assert!((n[1].velocity - 32.0 / 127.0).abs() < 1e-6);
    // The whole take is one undo step, like computer-keyboard recording.
    app.undo();
    assert!(notes(&app, idx).is_empty());
}

#[test]
fn step_recording_from_a_controller_advances_the_playhead() {
    let (_ctx, mut app, idx) = app_with_track();
    app.transport.position = 4.0;
    app.toggle_record();
    on(&mut app, 60, 90);
    off(&mut app, 60);
    on(&mut app, 62, 90);
    off(&mut app, 62);
    assert_eq!(app.transport.position, 6.0, "one snap per note");
    let n = notes(&app, idx);
    assert_eq!(
        n.iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(),
        vec![(60, 4.0), (62, 5.0)]
    );
}

#[test]
fn computer_keys_and_controller_notes_do_not_release_each_other() {
    let (_ctx, mut app, idx) = app_with_track();
    app.toggle_record();
    app.transport.playing = true;
    app.transport.position = 0.0;
    app.record_note_on(60).expect("computer note");
    on(&mut app, 60, 100);
    assert_eq!(app.editor.record_pending.len(), 2, "both held");
    app.transport.position = 3.0;
    off(&mut app, 60);
    assert!(app
        .editor
        .record_pending
        .contains_key(&(NoteSource::Computer, 60)));
    app.transport.position = 7.0;
    app.record_note_off(60);
    assert!(app.editor.record_pending.is_empty());
    let mut durations: Vec<f64> = notes(&app, idx).iter().map(|n| n.duration).collect();
    durations.sort_by(f64::total_cmp);
    assert_eq!(durations, vec![3.0, 7.0]);
}

#[test]
fn sustain_pedal_holds_recorded_notes_until_it_lifts() {
    let (_ctx, mut app, idx) = app_with_track();
    app.toggle_record();
    app.transport.playing = true;
    app.transport.position = 0.0;
    pedal(&mut app, true);
    on(&mut app, 60, 100);
    app.transport.position = 1.0;
    off(&mut app, 60);
    assert_eq!(app.editor.record_pending.len(), 1, "held by the pedal");
    app.transport.position = 5.0;
    pedal(&mut app, false);
    assert!(app.editor.record_pending.is_empty());
    assert_eq!(notes(&app, idx)[0].duration, 5.0);
}

#[test]
fn a_disconnect_ends_held_controller_notes() {
    let (_ctx, mut app, idx) = app_with_track();
    app.toggle_record();
    app.transport.playing = true;
    app.transport.position = 0.0;
    on(&mut app, 60, 100);
    app.record_note_on(62).expect("computer note");
    app.transport.position = 4.0;
    app.midi_input_event(MidiInputEvent::Disconnected {
        name: "Keys".to_owned(),
        reason: CloseReason::Unplugged,
    });
    let status = app.status.as_ref().unwrap();
    assert_eq!(status.kind, StatusKind::Warning);
    assert!(status.text.contains("Keys"), "{}", status.text);
    assert_eq!(
        app.editor
            .record_pending
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        vec![(NoteSource::Computer, 62)],
        "the computer key is still held"
    );
    let n = notes(&app, idx);
    assert_eq!(n.iter().find(|n| n.pitch == 60).unwrap().duration, 4.0);
}

#[test]
fn a_replug_ends_held_and_sustained_controller_notes_quietly() {
    let (_ctx, mut app, idx) = app_with_track();
    app.toggle_record();
    app.transport.playing = true;
    app.transport.position = 0.0;
    pedal(&mut app, true);
    on(&mut app, 60, 100);
    off(&mut app, 60);
    on(&mut app, 64, 100);
    app.transport.position = 2.0;
    app.status = None;
    app.midi_input_event(MidiInputEvent::Disconnected {
        name: "Keys".to_owned(),
        reason: CloseReason::Replugged,
    });
    assert!(app.editor.record_pending.is_empty(), "nothing left hanging");
    assert!(app.status.is_none(), "the reconnect is announced instead");
    let n = notes(&app, idx);
    assert!(n.iter().all(|n| n.duration == 2.0), "{n:?}");
    // The pedal is up again for the reopened connection.
    on(&mut app, 67, 100);
    off(&mut app, 67);
    assert!(app.editor.record_pending.is_empty());
}

#[test]
fn deselecting_an_input_leaves_the_new_inputs_notes_alone() {
    let (_ctx, mut app, _) = app_with_track();
    app.toggle_record();
    app.transport.playing = true;
    app.set_midi_input(MidiInputPref::Device("Keys".to_owned()));
    on(&mut app, 60, 100);
    app.midi_input_event(MidiInputEvent::Disconnected {
        name: "Pad".to_owned(),
        reason: CloseReason::Deselected,
    });
    assert_eq!(app.editor.record_pending.len(), 1, "still held on Keys");
    off(&mut app, 60);
    assert!(app.editor.record_pending.is_empty());
}

#[test]
fn connection_reports_become_status_messages() {
    let (_ctx, mut app, _) = app_with_track();
    app.status = None;
    app.midi_input_event(MidiInputEvent::Connected {
        name: "Keys".to_owned(),
        initial: true,
    });
    assert!(app.status.is_none(), "startup connections are quiet");
    app.midi_input_event(MidiInputEvent::Connected {
        name: "Keys".to_owned(),
        initial: false,
    });
    let status = app.status.clone().unwrap();
    assert_eq!(status.kind, StatusKind::Success);
    assert!(status.text.contains("connected: Keys"), "{}", status.text);
    app.midi_input_event(MidiInputEvent::Failed {
        name: "Pad".to_owned(),
        error: "busy".to_owned(),
    });
    assert!(app.status.as_ref().unwrap().text.contains("busy"));
}

#[test]
fn notes_without_a_selected_track_are_ignored() {
    let ctx = egui::Context::default();
    let mut app = CadenceApp::headless(Project::empty());
    frame(&ctx, &mut app);
    app.toggle_record();
    on(&mut app, 60, 100);
    off(&mut app, 60);
    assert!(app.editor.record_pending.is_empty());
}

#[test]
fn choosing_an_input_updates_the_pref_and_releases_held_notes() {
    let (_ctx, mut app, _) = app_with_track();
    assert_eq!(app.midi_pref, MidiInputPref::All, "default");
    app.toggle_record();
    app.transport.playing = true;
    on(&mut app, 60, 100);
    app.set_midi_input(MidiInputPref::Device("Keys".to_owned()));
    assert_eq!(app.midi_pref, MidiInputPref::Device("Keys".to_owned()));
    assert!(app.editor.record_pending.is_empty());
    assert!(app.status.as_ref().unwrap().text.contains("Keys"));
    app.set_midi_input(MidiInputPref::None);
    assert_eq!(app.status.as_ref().unwrap().text, "MIDI input off.");
}

#[test]
fn preferences_dialog_shows_the_midi_input_section() {
    let (ctx, mut app, _) = app_with_track();
    app.dialog = Dialog::Preferences;
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    assert!(matches!(app.dialog, Dialog::Preferences));
}

#[test]
fn midi_input_note_describes_the_connection_state() {
    let all = MidiInputPref::All;
    let keys = MidiInputPref::Device("Keys".to_owned());
    let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(midi_input_note(&all, None).contains("not available"));
    let found = (s(&["Keys", "Pad"]), s(&["Keys", "Pad"]), true);
    assert_eq!(
        midi_input_note(&all, Some(&found)),
        "Playing the selected track from Keys, Pad."
    );
    assert_eq!(
        midi_input_note(&MidiInputPref::None, Some(&found)),
        "MIDI input is off."
    );
    let nothing = (Vec::new(), Vec::new(), true);
    assert!(midi_input_note(&all, Some(&nothing)).contains("No MIDI devices"));
    assert!(midi_input_note(&keys, Some(&nothing)).contains("Waiting for Keys"));
    let early = (Vec::new(), Vec::new(), false);
    assert!(midi_input_note(&all, Some(&early)).contains("Looking"));
}
