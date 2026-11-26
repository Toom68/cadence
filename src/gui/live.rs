//! Held live notes: which played keys are sounding right now, and the
//! engine commands that start and end them.
//!
//! Computer keys, the on-screen Keys tab and a hardware MIDI controller
//! sound through [`crate::audio::AudioEngine::live_note_on`] /
//! [`live_note_off`](crate::audio::AudioEngine::live_note_off): a note rings
//! for as long as its key (or the sustain pedal, which `NoteTracker`
//! already folds into MIDI note ends) is held. This module is the
//! engine-free bookkeeping — pure, so headless tests can check every
//! decision through [`LiveNotes::sent`] — and the [`CadenceApp`] glue that
//! forwards the commands and ends held notes whenever their key can no
//! longer report its release (focus loss, track or tab switch, dialogs,
//! window close, project reload).

use egui::{Context, Key};

use super::{ops, CadenceApp, Tab};

/// Which input a live note came from. The engine keys held notes by
/// source and pitch, so one input letting go never ends another's note.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum LiveSource {
    Computer,
    Screen,
    Midi,
}

impl LiveSource {
    /// The engine's `source` id.
    pub(super) fn id(self) -> u32 {
        match self {
            LiveSource::Computer => 1,
            LiveSource::Screen => 2,
            LiveSource::Midi => 3,
        }
    }
}

/// The physical thing holding a note down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Holder {
    /// A computer key: its release ends the pitch it started, even after
    /// the octave or Shift changed in between.
    Computer(Key),
    /// The mouse on the Keys-tab piano (one pitch at a time).
    Screen,
    /// A controller pitch (`NoteTracker` pairs every start with one end).
    Midi(u8),
}

impl Holder {
    fn source(self) -> LiveSource {
        match self {
            Holder::Computer(_) => LiveSource::Computer,
            Holder::Screen => LiveSource::Screen,
            Holder::Midi(_) => LiveSource::Midi,
        }
    }
}

/// One engine command for live notes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LiveCmd {
    On {
        track_id: u64,
        source: LiveSource,
        pitch: u8,
        velocity: f32,
    },
    Off {
        source: LiveSource,
        pitch: u8,
    },
    AllOff,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    holder: Holder,
    pitch: u8,
    track_id: u64,
}

/// What ends every held note when it changes: the selected track, the
/// tab and whether a dialog is open.
type Context3 = (Option<u64>, Tab, bool);

/// Held live notes and the state that decides when to end them.
#[derive(Debug, Default)]
pub(super) struct LiveNotes {
    held: Vec<Held>,
    context: Option<Context3>,
    focused: bool,
    /// A note-off could not be queued (engine busy): an all-off is retried
    /// every frame until it goes through, so nothing can hang.
    retry_all_off: bool,
    /// Every command handed to the engine (or that would have been, in
    /// headless sessions), for tests.
    #[cfg(test)]
    pub sent: Vec<LiveCmd>,
}

impl LiveNotes {
    /// `holder` went down on `pitch`. A holder already sounding another
    /// pitch (the mouse gliding across keys) lets that one go first.
    pub(super) fn press(
        &mut self,
        holder: Holder,
        pitch: u8,
        velocity: f32,
        track_id: u64,
    ) -> Vec<LiveCmd> {
        let mut cmds = self.release(holder);
        let source = holder.source();
        // The new strike sounds anew, so drop an Off for this very pitch.
        cmds.retain(
            |c| !matches!(c, LiveCmd::Off { source: s, pitch: p } if *s == source && *p == pitch),
        );
        self.held.push(Held {
            holder,
            pitch,
            track_id,
        });
        cmds.push(LiveCmd::On {
            track_id,
            source,
            pitch,
            velocity,
        });
        cmds
    }

    /// `holder` went up: its note ends unless another holder of the same
    /// source still holds that pitch (two computer keys can map to it).
    pub(super) fn release(&mut self, holder: Holder) -> Vec<LiveCmd> {
        let Some(i) = self.held.iter().position(|h| h.holder == holder) else {
            return Vec::new();
        };
        let gone = self.held.swap_remove(i);
        let source = holder.source();
        let still = self
            .held
            .iter()
            .any(|h| h.holder.source() == source && h.pitch == gone.pitch);
        if still {
            Vec::new()
        } else {
            vec![LiveCmd::Off {
                source,
                pitch: gone.pitch,
            }]
        }
    }

    /// Ends every note held from `source`.
    pub(super) fn release_source(&mut self, source: LiveSource) -> Vec<LiveCmd> {
        let holders: Vec<Holder> = self
            .held
            .iter()
            .map(|h| h.holder)
            .filter(|h| h.source() == source)
            .collect();
        holders.into_iter().flat_map(|h| self.release(h)).collect()
    }

    /// Ends everything; no command when nothing is held.
    pub(super) fn all_off(&mut self) -> Vec<LiveCmd> {
        if self.held.is_empty() {
            return Vec::new();
        }
        self.held.clear();
        vec![LiveCmd::AllOff]
    }

    /// The pitch `holder` sounds, if any.
    #[cfg(test)]
    pub(super) fn pitch_of(&self, holder: Holder) -> Option<u8> {
        self.held
            .iter()
            .find(|h| h.holder == holder)
            .map(|h| h.pitch)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Per-frame checks: a changed selection/tab/dialog context ends every
    /// note; losing window focus ends the computer and on-screen notes
    /// (their key-ups will never arrive — a controller keeps reporting);
    /// a note whose track is gone ends too. `has_track` tells whether a
    /// track id still exists.
    pub(super) fn guard(
        &mut self,
        context: Context3,
        focused: bool,
        has_track: impl Fn(u64) -> bool,
    ) -> Vec<LiveCmd> {
        let changed = self.context.is_some_and(|c| c != context);
        self.context = Some(context);
        let lost_focus = self.focused && !focused;
        self.focused = focused;
        if changed || self.held.iter().any(|h| !has_track(h.track_id)) {
            return self.all_off();
        }
        if lost_focus {
            let mut cmds = self.release_source(LiveSource::Computer);
            cmds.extend(self.release_source(LiveSource::Screen));
            return cmds;
        }
        Vec::new()
    }
}

impl CadenceApp {
    /// Hands live-note commands to the engine. A note-off that cannot be
    /// queued arms an all-off retry (see [`Self::live_frame`]).
    pub(super) fn send_live(&mut self, cmds: Vec<LiveCmd>) {
        for cmd in cmds {
            #[cfg(test)]
            self.live.sent.push(cmd);
            let Some(engine) = &self.engine else {
                continue;
            };
            match cmd {
                LiveCmd::On {
                    track_id,
                    source,
                    pitch,
                    velocity,
                } => {
                    if let Some(track) = ops::track(&self.project, track_id) {
                        engine.live_note_on(track, pitch, velocity, source.id());
                    }
                }
                LiveCmd::Off { source, pitch } => {
                    if !engine.live_note_off(source.id(), pitch) {
                        self.live.retry_all_off = true;
                    }
                }
                LiveCmd::AllOff => {
                    if !engine.all_live_notes_off() {
                        self.live.retry_all_off = true;
                    }
                }
            }
        }
    }

    /// `holder` pressed `pitch` on `track_id`: the note sounds until released.
    pub(super) fn live_press(&mut self, holder: Holder, pitch: u8, velocity: f32, track_id: u64) {
        let cmds = self.live.press(holder, pitch, velocity, track_id);
        self.send_live(cmds);
    }

    /// `holder` let go.
    pub(super) fn live_release(&mut self, holder: Holder) {
        let cmds = self.live.release(holder);
        self.send_live(cmds);
    }

    /// Ends every held live note (project reload, window close).
    pub(super) fn live_all_off(&mut self) {
        let cmds = self.live.all_off();
        self.send_live(cmds);
    }

    /// End-of-frame live-note upkeep: the context/focus/track checks of
    /// [`LiveNotes::guard`] and the all-off retry.
    pub(super) fn live_frame(&mut self, ctx: &Context) {
        let context = (self.sel.track, self.tab, self.dialog.is_open());
        let focused = ctx.input(|i| i.focused);
        let project = &self.project;
        let cmds = self
            .live
            .guard(context, focused, |id| ops::track(project, id).is_some());
        self.send_live(cmds);
        if self.live.retry_all_off {
            self.live.retry_all_off = !self
                .engine
                .as_ref()
                .is_none_or(|engine| engine.all_live_notes_off());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 7;

    fn on(source: LiveSource, pitch: u8) -> LiveCmd {
        LiveCmd::On {
            track_id: T,
            source,
            pitch,
            velocity: 0.8,
        }
    }

    fn off(source: LiveSource, pitch: u8) -> LiveCmd {
        LiveCmd::Off { source, pitch }
    }

    #[test]
    fn press_and_release_pair_up_per_holder() {
        let mut live = LiveNotes::default();
        let z = Holder::Computer(Key::Z);
        assert_eq!(
            live.press(z, 60, 0.8, T),
            vec![on(LiveSource::Computer, 60)]
        );
        assert_eq!(live.pitch_of(z), Some(60));
        assert_eq!(live.release(z), vec![off(LiveSource::Computer, 60)]);
        assert!(live.release(z).is_empty(), "a second release is a no-op");
        assert!(live.is_empty());
    }

    #[test]
    fn two_keys_on_one_pitch_end_with_the_last() {
        let mut live = LiveNotes::default();
        let (a, b) = (Holder::Computer(Key::Z), Holder::Computer(Key::X));
        live.press(a, 60, 0.8, T);
        live.press(b, 60, 0.8, T);
        assert!(live.release(a).is_empty(), "the other key still holds 60");
        assert_eq!(live.release(b), vec![off(LiveSource::Computer, 60)]);
        // Different sources are independent: the engine tracks both.
        live.press(a, 62, 0.8, T);
        live.press(Holder::Midi(62), 62, 0.8, T);
        assert_eq!(live.release(a), vec![off(LiveSource::Computer, 62)]);
        assert_eq!(
            live.release(Holder::Midi(62)),
            vec![off(LiveSource::Midi, 62)]
        );
    }

    #[test]
    fn the_mouse_glides_from_key_to_key() {
        let mut live = LiveNotes::default();
        live.press(Holder::Screen, 60, 0.8, T);
        assert_eq!(
            live.press(Holder::Screen, 62, 0.8, T),
            vec![off(LiveSource::Screen, 60), on(LiveSource::Screen, 62)]
        );
        // Re-striking the same pitch needs no Off: the engine restarts it.
        assert_eq!(
            live.press(Holder::Screen, 62, 0.8, T),
            vec![on(LiveSource::Screen, 62)]
        );
        assert_eq!(live.pitch_of(Holder::Screen), Some(62));
    }

    #[test]
    fn guard_ends_notes_on_context_change_focus_loss_and_deleted_tracks() {
        let mut live = LiveNotes::default();
        let ctx = (Some(T), Tab::Editor, false);
        assert!(live.guard(ctx, true, |_| true).is_empty());
        // Nothing held: a context change sends nothing.
        assert!(live
            .guard((Some(T), Tab::Keys, false), true, |_| true)
            .is_empty());
        live.press(Holder::Computer(Key::Z), 60, 0.8, T);
        assert!(live
            .guard((Some(T), Tab::Keys, false), true, |_| true)
            .is_empty());
        assert_eq!(
            live.guard((Some(T), Tab::Keys, true), true, |_| true),
            vec![LiveCmd::AllOff],
            "a dialog opened"
        );
        let ctx = (Some(T), Tab::Keys, false);
        live.guard(ctx, true, |_| true);
        live.press(Holder::Computer(Key::Z), 60, 0.8, T);
        live.press(Holder::Screen, 64, 0.8, T);
        live.press(Holder::Midi(67), 67, 0.8, T);
        let cmds = live.guard(ctx, false, |_| true);
        assert_eq!(
            cmds,
            vec![off(LiveSource::Computer, 60), off(LiveSource::Screen, 64)],
            "focus loss ends keys whose release can't arrive"
        );
        assert_eq!(live.pitch_of(Holder::Midi(67)), Some(67));
        assert_eq!(
            live.guard(ctx, false, |id| id != T),
            vec![LiveCmd::AllOff],
            "the track was deleted"
        );
        assert!(live.is_empty());
    }
}
