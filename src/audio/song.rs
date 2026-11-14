//! Immutable, audio-thread-friendly snapshot of a [`Project`]. Built on the caller's thread
//! (allocation allowed there), then handed to the renderer by pointer.

use super::voice::{PresetParams, Sound};
use crate::model::{
    swung_step, Project, Track, MAX_BPM, MAX_CUTOFF_HZ, MAX_TRACKS, MIN_BPM, MIN_CUTOFF_HZ,
    STEPS_PER_BEAT,
};

/// Mixer and sound settings of one track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackParams {
    pub id: u64,
    pub sound: Sound,
    /// Linear fader gain (perceptual curve applied).
    pub gain: f32,
    pub pan: f32,
    pub audible: bool,
    pub cutoff: f32,
    pub reverb: f32,
    pub delay: f32,
    pub drive: f32,
}

fn unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Fader position (0..1) to linear gain: a squared curve gives roughly 40 dB of useful range
/// with fine control near unity.
pub fn fader_gain(volume: f32) -> f32 {
    let v = unit(volume);
    v * v
}

impl TrackParams {
    pub fn from_track(track: &Track, audible: bool) -> Self {
        let sound = match &track.preset {
            Some(p) => Sound::Preset(PresetParams::from_preset(p)),
            None => Sound::Builtin(track.instrument),
        };
        let cutoff = if track.effects.cutoff_hz.is_finite() {
            track.effects.cutoff_hz
        } else {
            MAX_CUTOFF_HZ
        };
        TrackParams {
            id: track.id,
            sound,
            gain: fader_gain(track.volume),
            pan: if track.pan.is_finite() {
                track.pan.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            audible,
            cutoff: cutoff.clamp(MIN_CUTOFF_HZ, MAX_CUTOFF_HZ),
            reverb: unit(track.effects.reverb),
            delay: unit(track.effects.delay),
            drive: unit(track.effects.drive),
        }
    }
}

/// A note on the swung timeline. `clock`/`end` are in swung steps, i.e. the transport clock
/// at which the note starts and is released.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SchedEvent {
    pub clock: f64,
    pub end: f64,
    pub pitch: u8,
    pub velocity: f32,
    pub track: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SongData {
    /// Sorted by `clock`.
    pub events: Vec<SchedEvent>,
    pub tracks: Vec<TrackParams>,
    pub bpm: f64,
    pub total_steps: f64,
    pub master_volume: f32,
}

impl SongData {
    /// Flattens the project with [`Project::events`] (all tracks; audibility is applied at
    /// trigger time so mute/solo changes take effect immediately and identically everywhere).
    pub fn from_project(project: &Project) -> Self {
        let swing = if project.swing.is_finite() {
            project.swing
        } else {
            0.0
        };
        let tracks: Vec<TrackParams> = project
            .tracks
            .iter()
            .take(MAX_TRACKS)
            .enumerate()
            .map(|(i, t)| TrackParams::from_track(t, project.is_track_audible(i)))
            .collect();
        let mut events: Vec<SchedEvent> = project
            .events(false)
            .into_iter()
            .filter(|e| e.track_index < tracks.len() && e.velocity > 0.0)
            .map(|e| SchedEvent {
                clock: swung_step(e.step, swing),
                end: swung_step(e.step + e.duration, swing),
                pitch: e.pitch.min(127),
                velocity: e.velocity,
                track: e.track_index,
            })
            .collect();
        // Swing is monotonic so the order is preserved; sort anyway to be robust.
        events.sort_by(|a, b| a.clock.total_cmp(&b.clock));
        SongData {
            events,
            tracks,
            bpm: if project.bpm.is_finite() {
                project.bpm.clamp(MIN_BPM, MAX_BPM)
            } else {
                120.0
            },
            total_steps: project.total_steps().max(1.0),
            master_volume: unit(project.master_volume),
        }
    }

    pub fn empty() -> Self {
        SongData {
            events: Vec::new(),
            tracks: Vec::new(),
            bpm: 120.0,
            total_steps: 16.0,
            master_volume: 0.8,
        }
    }

    pub fn seconds_per_step(&self) -> f64 {
        60.0 / (self.bpm * STEPS_PER_BEAT as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_applies_swing_and_audibility() {
        let mut p = Project::demo();
        p.swing = 0.5;
        p.tracks[1].mute = true;
        let s = SongData::from_project(&p);
        assert_eq!(s.tracks.len(), 5);
        assert!(!s.tracks[1].audible && s.tracks[0].audible);
        assert_eq!(s.events.len(), p.events(false).len());
        assert!(s.events.windows(2).all(|w| w[0].clock <= w[1].clock));
        // The ghost snare on step 9 lands on the triplet; the step-8 kick does not move.
        assert!(s
            .events
            .iter()
            .any(|e| e.track == 0 && (e.clock - (8.0 + 4.0 / 3.0)).abs() < 1e-9));
        assert!(s.events.iter().any(|e| e.track == 0 && e.clock == 8.0));
        assert!(!s.events.iter().any(|e| e.track == 0 && e.clock == 9.0));
        assert!(s
            .events
            .iter()
            .all(|e| e.end > e.clock && e.end <= s.total_steps + 1e-9));
    }

    #[test]
    fn preset_tracks_use_preset_sound() {
        let mut p = Project::empty();
        let mut preset = crate::packs::template_pack("test-pack", "Test")
            .unwrap()
            .instruments[0]
            .clone();
        preset.monophonic = true;
        p.tracks[0].preset = Some(preset);
        let s = SongData::from_project(&p);
        assert!(matches!(s.tracks[0].sound, Sound::Preset(pp) if pp.mono));
    }
}
