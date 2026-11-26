//! Headless tests for held live notes: played keys (computer, Keys tab,
//! MIDI controller) must produce paired live note-on/off engine commands,
//! and every situation where a key-up can be lost must end the note. The
//! commands are read from `LiveNotes::sent`, which records what the app
//! hands the engine (headless sessions have none).

use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};

use super::dialogs::Dialog;
use super::live::{LiveCmd, LiveSource};
use super::{CadenceApp, Focus, Tab};
use crate::midi_in::{CloseReason, MidiInputEvent, MidiMessage};
use crate::model::{Instrument, Project};

struct Ui {
    ctx: egui::Context,
    app: CadenceApp,
    events: Vec<Event>,
    focused: bool,
    time: f64,
}

impl Ui {
    /// One selected piano track with the note editor focused.
    fn new() -> Self {
        let mut ui = Ui {
            ctx: egui::Context::default(),
            app: CadenceApp::headless(Project::empty()),
            events: Vec::new(),
            focused: true,
            time: 0.0,
        };
        ui.step();
        ui.app.add_track(Instrument::Piano);
        let id = ui.track();
        ui.app.select_track(id);
        ui.app.focus = Focus::Editor;
        ui.step();
        ui.app.live.sent.clear();
        ui
    }

    fn track(&self) -> u64 {
        self.app.project.tracks.last().unwrap().id
    }

    fn step(&mut self) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1360.0, 860.0))),
            focused: self.focused,
            time: Some(self.time),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        let app = &mut self.app;
        let _ = self.ctx.run(input, |ctx| app.frame(ctx));
        self.time += 1.0 / 60.0;
    }

    fn key(&mut self, key: Key, pressed: bool) {
        self.key_event(key, pressed, false);
    }

    fn key_event(&mut self, key: Key, pressed: bool, repeat: bool) {
        self.events.push(Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        });
        self.step();
    }

    fn pointer(&mut self, pos: Pos2, pressed: Option<bool>) {
        self.events.push(Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            self.events.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        self.step();
    }

    fn midi(&mut self, message: MidiMessage) {
        self.app
            .midi_input_event(MidiInputEvent::Message { conn: 1, message });
    }

    /// Commands sent since the last call.
    fn sent(&mut self) -> Vec<LiveCmd> {
        std::mem::take(&mut self.app.live.sent)
    }
}

fn on(track_id: u64, source: LiveSource, pitch: u8, velocity: f32) -> LiveCmd {
    LiveCmd::On {
        track_id,
        source,
        pitch,
        velocity,
    }
}

fn off(source: LiveSource, pitch: u8) -> LiveCmd {
    LiveCmd::Off { source, pitch }
}

fn z_pitch(ui: &Ui) -> u8 {
    super::editor::key_to_pitch(Key::Z, ui.app.editor.kb_octave).unwrap()
}

#[test]
fn computer_key_down_and_up_start_and_end_a_live_note() {
    let mut ui = Ui::new();
    let (track, pitch, vel) = (ui.track(), z_pitch(&ui), ui.app.editor.velocity);
    ui.key(Key::Z, true);
    assert_eq!(
        ui.sent(),
        vec![on(track, LiveSource::Computer, pitch, vel)],
        "key down: a held note, not a fixed preview"
    );
    // Held for many frames (and OS auto-repeat): nothing else is sent.
    ui.key_event(Key::Z, true, true);
    for _ in 0..30 {
        ui.step();
    }
    assert!(ui.sent().is_empty(), "the note keeps sounding");
    ui.key(Key::Z, false);
    assert_eq!(ui.sent(), vec![off(LiveSource::Computer, pitch)]);
}

#[test]
fn a_key_released_after_an_octave_change_ends_its_own_pitch() {
    let mut ui = Ui::new();
    let pitch = z_pitch(&ui);
    ui.key(Key::Z, true);
    ui.key(Key::W, true);
    ui.key(Key::W, false);
    assert_ne!(z_pitch(&ui), pitch, "octave moved");
    ui.sent();
    ui.key(Key::Z, false);
    assert_eq!(ui.sent(), vec![off(LiveSource::Computer, pitch)]);
}

#[test]
fn chords_from_two_keys_hold_independently() {
    let mut ui = Ui::new();
    ui.key(Key::Z, true);
    ui.key(Key::C, true);
    ui.sent();
    let c = super::editor::key_to_pitch(Key::Z, ui.app.editor.kb_octave).unwrap();
    let e = super::editor::key_to_pitch(Key::C, ui.app.editor.kb_octave).unwrap();
    ui.key(Key::C, false);
    assert_eq!(ui.sent(), vec![off(LiveSource::Computer, e)]);
    ui.key(Key::Z, false);
    assert_eq!(ui.sent(), vec![off(LiveSource::Computer, c)]);
}

#[test]
fn focus_loss_ends_held_computer_keys() {
    let mut ui = Ui::new();
    let pitch = z_pitch(&ui);
    ui.key(Key::Z, true);
    ui.sent();
    ui.focused = false;
    ui.step();
    assert_eq!(ui.sent(), vec![off(LiveSource::Computer, pitch)]);
    // The late key-up (if the OS delivers one) sends nothing more.
    ui.focused = true;
    ui.key(Key::Z, false);
    assert!(ui.sent().is_empty());
}

#[test]
fn track_switch_tab_switch_and_dialogs_end_held_notes() {
    let mut ui = Ui::new();
    let first = ui.track();
    ui.app.add_track(Instrument::Bass);
    let second = ui.track();
    ui.app.select_track(first);
    ui.step();
    ui.sent();

    ui.key(Key::Z, true);
    ui.app.select_track(second);
    ui.step();
    assert!(ui.sent().ends_with(&[LiveCmd::AllOff]), "track switch");

    ui.key(Key::Z, false);
    ui.key(Key::Z, true);
    ui.sent();
    ui.app.tab = Tab::Mixer;
    ui.step();
    assert_eq!(ui.sent(), vec![LiveCmd::AllOff], "tab switch");

    ui.app.tab = Tab::Editor;
    ui.step();
    ui.key(Key::Z, false);
    ui.key(Key::Z, true);
    ui.sent();
    ui.app.dialog = Dialog::Help;
    ui.step();
    assert_eq!(ui.sent(), vec![LiveCmd::AllOff], "dialog opened");
}

#[test]
fn deleting_the_track_or_replacing_the_song_ends_held_notes() {
    let mut ui = Ui::new();
    let track = ui.track();
    ui.midi(MidiMessage::NoteOn {
        channel: 0,
        pitch: 60,
        velocity: 100,
    });
    ui.sent();
    ui.app.delete_track(track);
    ui.step();
    assert_eq!(ui.sent(), vec![LiveCmd::AllOff], "track deleted");

    let mut ui = Ui::new();
    ui.key(Key::Z, true);
    ui.sent();
    ui.app.replace_project(Project::empty(), None, true);
    assert_eq!(ui.sent(), vec![LiveCmd::AllOff], "song replaced");
}

#[test]
fn controller_notes_hold_until_note_off_or_pedal_up() {
    let mut ui = Ui::new();
    let track = ui.track();
    ui.midi(MidiMessage::NoteOn {
        channel: 0,
        pitch: 64,
        velocity: 127,
    });
    assert_eq!(ui.sent(), vec![on(track, LiveSource::Midi, 64, 1.0)]);
    ui.midi(MidiMessage::NoteOff {
        channel: 0,
        pitch: 64,
    });
    assert_eq!(ui.sent(), vec![off(LiveSource::Midi, 64)]);

    // Sustain pedal: the release waits for the pedal.
    ui.midi(MidiMessage::Sustain {
        channel: 0,
        down: true,
    });
    ui.midi(MidiMessage::NoteOn {
        channel: 0,
        pitch: 67,
        velocity: 64,
    });
    ui.midi(MidiMessage::NoteOff {
        channel: 0,
        pitch: 67,
    });
    assert_eq!(
        ui.sent(),
        vec![on(track, LiveSource::Midi, 67, 64.0 / 127.0)],
        "pedal down: still sounding"
    );
    ui.midi(MidiMessage::Sustain {
        channel: 0,
        down: false,
    });
    assert_eq!(ui.sent(), vec![off(LiveSource::Midi, 67)]);

    // Unplugging ends what the controller held.
    ui.midi(MidiMessage::NoteOn {
        channel: 0,
        pitch: 72,
        velocity: 90,
    });
    ui.sent();
    ui.app.midi_input_event(MidiInputEvent::Disconnected {
        name: "Keys".into(),
        conn: 1,
        reason: CloseReason::Unplugged,
    });
    assert_eq!(ui.sent(), vec![off(LiveSource::Midi, 72)]);
}

#[test]
fn controller_notes_survive_focus_loss() {
    let mut ui = Ui::new();
    ui.midi(MidiMessage::NoteOn {
        channel: 0,
        pitch: 60,
        velocity: 100,
    });
    ui.sent();
    // The controller still reports its key-up while the window is in the
    // background, so its note keeps sounding.
    ui.focused = false;
    ui.step();
    assert!(ui.sent().is_empty());
    ui.midi(MidiMessage::NoteOff {
        channel: 0,
        pitch: 60,
    });
    assert_eq!(ui.sent(), vec![off(LiveSource::Midi, 60)]);
}

#[test]
fn computer_and_controller_on_one_pitch_are_separate_sources() {
    let mut ui = Ui::new();
    let pitch = z_pitch(&ui);
    ui.key(Key::Z, true);
    ui.midi(MidiMessage::NoteOn {
        channel: 0,
        pitch,
        velocity: 100,
    });
    ui.sent();
    ui.key(Key::Z, false);
    assert_eq!(
        ui.sent(),
        vec![off(LiveSource::Computer, pitch)],
        "only the computer key's hold ends; the engine keeps the controller's"
    );
}

#[test]
fn keys_tab_mouse_press_glide_and_release() {
    let mut ui = Ui::new();
    let track = ui.track();
    let vel = ui.app.editor.velocity;
    ui.app.tab = Tab::Keys;
    ui.step();
    ui.step();
    ui.sent();
    let area = ui.app.keys_piano_rect();
    assert!(area.width() > 240.0, "the piano is laid out");
    // Lower edge of the first two white keys: C and D of kb_octave - 1.
    let white_w = area.width() / 21.0;
    let y = area.bottom() - 6.0;
    let c = pos(area.left() + white_w * 0.5, y);
    let d = pos(area.left() + white_w * 1.5, y);
    let low_c = (ui.app.editor.kb_octave * 12) as u8;
    ui.pointer(c, Some(true));
    ui.step();
    assert_eq!(ui.sent(), vec![on(track, LiveSource::Screen, low_c, vel)]);
    for _ in 0..20 {
        ui.step();
    }
    assert!(ui.sent().is_empty(), "held while the button is down");
    ui.pointer(d, None);
    assert_eq!(
        ui.sent(),
        vec![
            off(LiveSource::Screen, low_c),
            on(track, LiveSource::Screen, low_c + 2, vel)
        ],
        "glissando"
    );
    ui.pointer(d, Some(false));
    ui.step();
    assert_eq!(ui.sent(), vec![off(LiveSource::Screen, low_c + 2)]);
}

fn pos(x: f32, y: f32) -> Pos2 {
    Pos2::new(x, y)
}
