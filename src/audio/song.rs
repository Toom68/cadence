//! Immutable, audio-thread-friendly snapshot of a [`Project`]. Built on the caller's thread
//! (allocation allowed there), then handed to the renderer by pointer.

use super::voice::{FontKey, KitParams, PresetParams, Sound};
use crate::model::{
    audio_ref_name_ok, swung_step, ArpDirection, ArpSettings, Automation, Bus, Project, SongEvent,
    Track, MAX_ARP_OCTAVES, MAX_AUDIO_CLIPS_PER_TRACK, MAX_BPM, MAX_CUTOFF_HZ, MAX_EQ_DB,
    MAX_GLIDE_SECONDS, MAX_SONG_EVENTS, MAX_TRACKS, MIN_ARP_GATE, MIN_BPM, MIN_CUTOFF_HZ,
    NUM_BUSES, STEPS_PER_BEAT,
};

/// Mixer and sound settings of one track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackParams {
    pub id: u64,
    pub sound: Sound,
    /// Sound used when `sound` is [`Sound::Font`] but the font file is not
    /// installed (or cannot run at the current sample rate): the track's pack
    /// preset or built-in instrument, computed alongside `sound`.
    pub fallback: Sound,
    /// Linear fader gain (perceptual curve applied).
    pub gain: f32,
    pub pan: f32,
    pub audible: bool,
    pub cutoff: f32,
    pub reverb: f32,
    pub delay: f32,
    pub drive: f32,
    /// Channel EQ `[low, mid, high]` gains in dB (all 0 = bypass).
    pub eq: [f32; 3],
    /// Per-track portamento slide time in seconds, 0 = off. While above
    /// zero, melodic voices on the track retarget legato-style instead of
    /// stacking (see [`crate::model::Track::glide`]).
    pub glide: f32,
    /// Subgroup bus the strip sums into: 0 = straight to the master,
    /// `1..=NUM_BUSES` = `SongData::buses[bus - 1]`. The bus's own mute/solo
    /// gate is already folded into `audible` by
    /// [`Project::is_track_audible`], so a gated track neither triggers nor
    /// feeds the shared sends.
    pub bus: u8,
}

/// Mixer settings of one fixed subgroup bus — the [`crate::model::Bus`]
/// counterpart of [`TrackParams`]. Buses deliberately carry no effects:
/// the renderer still runs them through a full channel strip with sends,
/// drive and filter parked neutral.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusParams {
    /// Linear fader gain (perceptual curve applied).
    pub gain: f32,
    pub pan: f32,
    /// The bus's own mute/solo gate. Tracks routed here are already gated
    /// through [`Project::is_track_audible`]; keeping it on the bus strip
    /// too fences the group even if a track slips through un-gated.
    pub audible: bool,
}

impl BusParams {
    pub fn from_bus(bus: &Bus, audible: bool) -> Self {
        BusParams {
            gain: fader_gain(bus.volume),
            pan: if bus.pan.is_finite() {
                bus.pan.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            audible,
        }
    }
}

impl Default for BusParams {
    fn default() -> Self {
        BusParams::from_bus(&Bus::default(), true)
    }
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
        // Kit-ness is keyed on `preset.kit` alone: `track.instrument` only
        // picks the editor (drum grid vs piano roll), never the sound. A
        // kit preset therefore plays as a kit on any track.
        let fallback = match &track.preset {
            Some(p) if p.kit.is_some() => Sound::Kit(KitParams::from_kit(p)),
            Some(p) => Sound::Preset(PresetParams::from_preset(p)),
            None => Sound::Builtin(track.instrument),
        };
        // A font reference plays sampled GM audio through the renderer's font
        // bank; `fallback` covers the missing-font case at note-on time. An
        // empty name or out-of-range program (only reachable without
        // validation) plays the fallback directly.
        let sound = match &track.font {
            Some(f) if !f.name.trim().is_empty() && f.program <= 127 => Sound::Font(FontKey {
                font_key: f.key(),
                program: f.program,
            }),
            _ => fallback,
        };
        let cutoff = if track.effects.cutoff_hz.is_finite() {
            track.effects.cutoff_hz
        } else {
            MAX_CUTOFF_HZ
        };
        TrackParams {
            id: track.id,
            sound,
            fallback,
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
            eq: track.eq.bands().map(|db| {
                if db.is_finite() {
                    db.clamp(-MAX_EQ_DB, MAX_EQ_DB)
                } else {
                    0.0
                }
            }),
            glide: if track.glide.is_finite() {
                track.glide.clamp(0.0, MAX_GLIDE_SECONDS)
            } else {
                0.0
            },
            // `min` keeps unvalidated data routable instead of indexing
            // past the bus array.
            bus: track.bus.min(NUM_BUSES as u8),
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

/// An imported-WAV clip scheduled on the transport clock: the model's
/// [`crate::model::AudioClip`] with its staged file resolved to an entry of
/// [`SongData::audio`]. Positions are in *straight* steps — audio clips do
/// not swing. The clip is stateless (the renderer maps clock→source
/// position directly), so seeking into the middle of a clip and transport
/// looping both work without any chase bookkeeping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SchedAudio {
    /// Steps from song start where the clip begins.
    pub start: f64,
    /// Steps one repetition occupies.
    pub length: f64,
    /// Repetitions (`1..=MAX_CLIP_LOOPS` after clamping).
    pub loops: u32,
    /// Seconds into the decoded source where playback starts.
    pub offset: f64,
    /// Fade-in length in steps (linear gain ramp at the clip start).
    pub fade_in: f64,
    /// Fade-out length in steps (linear gain ramp at the clip end).
    pub fade_out: f64,
    /// Track index — the same space `SchedEvent::track` uses — so the clip
    /// feeds the track's strip (fader/pan/drive/filter/sends/bus routing).
    pub track: usize,
    /// Index into [`SongData::audio`] — the decoded source.
    pub source: usize,
}

impl SchedAudio {
    /// Total steps the clip occupies (`length × loops`), `<= 0` on bad data.
    pub fn span(&self) -> f64 {
        if self.length.is_finite() && self.length > 0.0 {
            self.length * self.loops as f64
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SongData {
    /// Sorted by `clock`.
    pub events: Vec<SchedEvent>,
    pub tracks: Vec<TrackParams>,
    /// Imported-WAV clips on the timeline, sorted by `start`. Sources are
    /// already decoded into [`Self::audio`]; a clip whose file was missing
    /// or unreadable never reaches this list — the project stays valid and
    /// it simply plays silence.
    pub audio_clips: Vec<SchedAudio>,
    /// Decoded clip sources (deduped by staged file name). The buffers are
    /// built in [`SongData::from_project`] — off the audio thread — and the
    /// renderer only ever borrows them.
    pub audio: Vec<crate::persistence::DecodedWav>,
    /// Mixer automation per track, parallel to `tracks`: `Some` where the
    /// track actually has points, `None` otherwise. The renderer chases the
    /// playhead's interpolated value through the strip's fader/pan smoothers,
    /// so `None` keeps the static mix with zero per-block cost.
    pub automation: Vec<Option<Automation>>,
    /// The fixed subgroup buses; `TrackParams::bus` indexes this as
    /// `bus - 1` (bus 0 is the master itself and has no entry).
    pub buses: [BusParams; NUM_BUSES],
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
        // Parallel to `tracks`: only tracks carrying points get an entry —
        // an empty Automation behaves exactly like `None`.
        let automation: Vec<Option<Automation>> = project
            .tracks
            .iter()
            .take(MAX_TRACKS)
            .map(|t| t.automation.clone().filter(|a| !a.is_empty()))
            .collect();
        // The live arpeggiator subdivides chords into hits on the straight
        // timeline; each hit is swung below like a written note.
        let flattened = expand_track_arpeggios(project, &tracks, project.events(false));
        let mut events: Vec<SchedEvent> = flattened
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
        // Flattened events are already sorted and swing preserves that order
        // on valid projects. Avoid scratch allocation for the usual case.
        if !events.is_sorted_by(|a, b| a.clock.total_cmp(&b.clock).is_le()) {
            events.sort_by(|a, b| a.clock.total_cmp(&b.clock));
        }
        // Imported-WAV clips: resolve each staged file name to decoded audio
        // (deduped — several clips may share one file), skipping anything
        // missing or unreadable so a lost file can never break playback.
        // This is the only place file I/O happens, and it always runs on
        // the caller's thread — never inside the render loop.
        let mut audio: Vec<crate::persistence::DecodedWav> = Vec::new();
        let mut source_by_name: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        let mut audio_clips = Vec::new();
        for (track_index, track) in project.tracks.iter().take(MAX_TRACKS).enumerate() {
            for clip in track.audio_clips.iter().take(MAX_AUDIO_CLIPS_PER_TRACK) {
                if !audio_ref_name_ok(&clip.file)
                    || !clip.start.is_finite()
                    || clip.start < 0.0
                    || clip.span() <= 0.0
                {
                    continue;
                }
                let source = match source_by_name.get(clip.file.as_str()) {
                    Some(&i) => i,
                    None => {
                        let decoded = crate::persistence::read_audio_file(&clip.file)
                            .and_then(|b| crate::persistence::decode_wav_f32(&b).ok());
                        match decoded {
                            Some(d) if !d.left.is_empty() && d.sample_rate > 0 => {
                                let i = audio.len();
                                audio.push(d);
                                source_by_name.insert(clip.file.as_str(), i);
                                i
                            }
                            // Missing or unreadable staged file: drop the clip
                            // from the schedule — it stays valid in the model
                            // and simply makes no sound.
                            _ => continue,
                        }
                    }
                };
                audio_clips.push(SchedAudio {
                    start: clip.start,
                    length: clip.length,
                    loops: clip.loop_count(),
                    offset: if clip.offset.is_finite() {
                        clip.offset.max(0.0)
                    } else {
                        0.0
                    },
                    fade_in: if clip.fade_in.is_finite() {
                        clip.fade_in.max(0.0)
                    } else {
                        0.0
                    },
                    fade_out: if clip.fade_out.is_finite() {
                        clip.fade_out.max(0.0)
                    } else {
                        0.0
                    },
                    track: track_index,
                    source,
                });
            }
        }
        audio_clips.sort_by(|a, b| a.start.total_cmp(&b.start));
        SongData {
            events,
            tracks,
            audio_clips,
            audio,
            automation,
            buses: std::array::from_fn(|i| {
                BusParams::from_bus(&project.buses[i], project.is_bus_audible(i as u8 + 1))
            }),
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
            audio_clips: Vec::new(),
            audio: Vec::new(),
            buses: [BusParams::default(); NUM_BUSES],
            automation: Vec::new(),
            bpm: 120.0,
            total_steps: 16.0,
            master_volume: 0.8,
        }
    }

    pub fn seconds_per_step(&self) -> f64 {
        60.0 / (self.bpm * STEPS_PER_BEAT as f64)
    }
}

// ----- live arpeggiator ------------------------------------------------------
// The per-track arpeggiator (`Track::arp`) is a playback effect: it rewrites
// the flattened event list before scheduling, so the renderer needs no
// changes and export sounds identical to live playback. It runs on straight
// (unswung) steps — every generated hit is mapped through `swung_step` like a
// written note, so arp hits swing with everything else.

/// Applies each track's live arpeggiator to the flattened event list. A
/// track's arp is inert while disabled and on drum or kit sounds — the
/// primary sound decides (`!params.sound.is_drums()`), so a pack kit on a
/// melodic track is skipped while a SoundFont program always counts as
/// melodic. On an arp-active track every *chord* — two or more note events
/// sharing one start step — is subdivided into a cycling run of hits;
/// single notes always pass through unchanged. `events` must be the output
/// of [`Project::events`] (sorted by step, then track, then pitch); the
/// result is re-sorted the same way.
fn expand_track_arpeggios(
    project: &Project,
    tracks: &[TrackParams],
    events: Vec<SongEvent>,
) -> Vec<SongEvent> {
    let arp_on = |i: usize| {
        project.tracks.get(i).is_some_and(|t| t.arp.enabled)
            && tracks.get(i).is_some_and(|p| !p.sound.is_drums())
    };
    if !(0..tracks.len()).any(&arp_on) {
        return events;
    }
    // Partition by track: the global sort keeps each track's events ordered
    // by (step, pitch), which is all the grouping below needs.
    let mut by_track: Vec<Vec<SongEvent>> = vec![Vec::new(); tracks.len()];
    for e in events {
        if e.track_index < tracks.len() {
            by_track[e.track_index].push(e);
        }
    }
    let mut out = Vec::new();
    for (i, track_events) in by_track.into_iter().enumerate() {
        if arp_on(i) {
            let track = &project.tracks[i];
            expand_chords(&mut out, &track_events, &track.arp, track.id, i);
        } else {
            out.extend(track_events);
        }
    }
    out.sort_by(|a, b| {
        a.step
            .total_cmp(&b.step)
            .then(a.track_index.cmp(&b.track_index))
            .then(a.pitch.cmp(&b.pitch))
    });
    out
}

/// Expands one arp-active track's events (sorted by start, then pitch).
///
/// Chord boundary rule: a chord is a maximal run of events sharing one start
/// step; it occupies `[start, chord_end)` where `chord_end` is the
/// longest-held note's release. Hits land every `rate` steps strictly inside
/// that span — a hit starting exactly at `chord_end` would sound a note the
/// chord already released — and each sounds `rate * gate` steps clipped at
/// `chord_end`, so no arp note ever rings past the chord's own end. If the
/// chord's notes disagree on duration the pool still uses the pitches that
/// started together. Once [`MAX_SONG_EVENTS`] is reached the remaining
/// events pass through unexpanded.
fn expand_chords(
    out: &mut Vec<SongEvent>,
    events: &[SongEvent],
    arp: &ArpSettings,
    track_id: u64,
    track_index: usize,
) {
    let rate = arp.rate.steps();
    let gate = if arp.gate.is_finite() {
        arp.gate.clamp(MIN_ARP_GATE, 1.0) as f64
    } else {
        0.9
    };
    let octaves = arp.octaves.clamp(1, MAX_ARP_OCTAVES);
    let mut chord_index = 0u64;
    let mut i = 0;
    while i < events.len() {
        let start = events[i].step;
        let mut j = i + 1;
        while j < events.len() && events[j].step == start {
            j += 1;
        }
        let group = &events[i..j];
        i = j;
        if group.len() < 2 || out.len() >= MAX_SONG_EVENTS {
            out.extend_from_slice(group);
            continue;
        }
        // Tone pool: the chord's pitches (deduped, each keeping the velocity
        // of its loudest occurrence) plus the same shape transposed up
        // `octaves - 1` times; tones past MIDI 127 drop like `arp_tones`.
        let mut tones: Vec<(u8, f32)> = group.iter().map(|e| (e.pitch, e.velocity)).collect();
        tones.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
        tones.dedup_by_key(|t| t.0);
        let base = tones.clone();
        for octave in 1..octaves {
            for &(pitch, velocity) in &base {
                let up = pitch as u32 + 12 * octave;
                if up > 127 {
                    continue;
                }
                let up = up as u8;
                if !tones.iter().any(|(p, _)| *p == up) {
                    tones.push((up, velocity));
                }
            }
        }
        tones.sort_by_key(|t| t.0);
        // Cycle order — the same shapes the pattern arpeggiator generates.
        let order: Vec<usize> = match arp.direction {
            ArpDirection::Up | ArpDirection::Random | ArpDirection::Block => {
                (0..tones.len()).collect()
            }
            ArpDirection::Down => (0..tones.len()).rev().collect(),
            ArpDirection::UpDown => {
                let mut v: Vec<usize> = (0..tones.len()).collect();
                if tones.len() > 2 {
                    v.extend((1..tones.len() - 1).rev());
                }
                v
            }
        };
        let chord_end = group
            .iter()
            .map(|e| e.step + e.duration)
            .fold(start, f64::max);
        // Deterministic per chord (track id and its index among the track's
        // chords) so Random plays the same picks every render.
        let mut seed = track_id
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(chord_index.wrapping_mul(0x2545_F491_4F6C_DD1D))
            | 1;
        chord_index += 1;
        let mut index = 0usize;
        let mut last_pick = usize::MAX;
        let mut t = start;
        while t < chord_end - 1e-9 && out.len() < MAX_SONG_EVENTS {
            let duration = (rate * gate).min(chord_end - t);
            match arp.direction {
                ArpDirection::Block => {
                    for &(pitch, velocity) in &tones {
                        out.push(SongEvent {
                            step: t,
                            duration,
                            pitch,
                            velocity,
                            track_index,
                        });
                    }
                }
                ArpDirection::Random => {
                    let mut pick = (arp_rand(&mut seed) % tones.len() as u64) as usize;
                    if pick == last_pick && tones.len() > 1 {
                        pick = (pick + 1) % tones.len();
                    }
                    last_pick = pick;
                    let (pitch, velocity) = tones[pick];
                    out.push(SongEvent {
                        step: t,
                        duration,
                        pitch,
                        velocity,
                        track_index,
                    });
                }
                _ => {
                    let (pitch, velocity) = tones[order[index % order.len()]];
                    out.push(SongEvent {
                        step: t,
                        duration,
                        pitch,
                        velocity,
                        track_index,
                    });
                }
            }
            index += 1;
            t += rate;
        }
    }
}

/// xorshift64* step — the same stream the pattern arpeggiator seeds, so
/// Random picks feel consistent between the two features. Deterministic so
/// arpeggios (and tests) are repeatable.
fn arp_rand(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
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

    #[test]
    fn kit_preset_track_uses_kit_sound() {
        use crate::model::Instrument;
        use crate::packs::InstrumentPreset;
        let preset: InstrumentPreset = serde_json::from_str(
            r#"{
                "id": "test.kit", "name": "Kit",
                "kit": { "pieces": [
                    { "name": "Kick", "pitch": 36, "voice": {
                        "oscillators": [{"waveform":"sine"}] } },
                    { "name": "Hat", "pitch": 42, "voice": {
                        "oscillators": [{"waveform":"noise"}] } }
                ] } }"#,
        )
        .unwrap();
        crate::packs::validate_preset(&preset).unwrap();
        // Kit-ness is keyed on `preset.kit` alone: the track's `instrument`
        // only picks the editor, so a kit preset plays as a kit on ANY track.
        for editor in [Instrument::Drums, Instrument::Piano] {
            let mut p = Project::empty();
            p.tracks[0] = Track::from_preset(editor, preset.clone());
            let s = SongData::from_project(&p);
            assert!(
                matches!(s.tracks[0].sound, Sound::Kit(_)),
                "{editor:?} track with kit preset must produce Sound::Kit"
            );
        }
        // A melodic preset on the same track still produces Sound::Preset.
        let mut p = Project::empty();
        p.tracks[0].preset = Some(
            crate::packs::template_pack("test-pack", "Test")
                .unwrap()
                .instruments[0]
                .clone(),
        );
        let s = SongData::from_project(&p);
        assert!(matches!(s.tracks[0].sound, Sound::Preset(_)));
    }

    #[test]
    fn font_track_uses_font_sound_with_builtin_fallback() {
        use crate::model::{FontRef, Instrument};
        let mut p = Project::empty();
        p.tracks[0] = Track::from_font(Instrument::Piano, FontRef::new("GeneralUser", 0));
        let s = SongData::from_project(&p);
        assert!(
            matches!(s.tracks[0].sound, Sound::Font(k) if k.program == 0
                && k.font_key == crate::font::font_key("GeneralUser")),
            "font track snapshots to Sound::Font"
        );
        assert_eq!(
            s.tracks[0].fallback,
            Sound::Builtin(Instrument::Piano),
            "no preset: built-in instrument is the fallback"
        );

        // A pack preset remains the fallback when the font is missing.
        let pack = crate::packs::template_pack("unit-pack", "Unit").unwrap();
        p.tracks[0].preset = Some(pack.instruments[0].clone());
        let s = SongData::from_project(&p);
        assert!(matches!(s.tracks[0].sound, Sound::Font(_)));
        assert!(matches!(s.tracks[0].fallback, Sound::Preset(_)));

        // Invalid references (empty name, program > 127 — only reachable
        // without validation) snapshot straight to the fallback sound.
        for bad in [FontRef::new("  ", 0), FontRef::new("GeneralUser", 200)] {
            p.tracks[0].font = Some(bad);
            let s = SongData::from_project(&p);
            assert!(!matches!(s.tracks[0].sound, Sound::Font(_)));
            assert_eq!(s.tracks[0].sound, s.tracks[0].fallback);
        }
    }

    #[test]
    fn snapshot_carries_only_nonempty_automation() {
        use crate::model::{create_track, Automation, AutomationLane, AutomationPoint, Instrument};
        let mut p = Project::empty();
        p.tracks.push(create_track(Instrument::Piano));
        assert_eq!(SongData::from_project(&p).automation.len(), p.tracks.len());
        assert!(SongData::from_project(&p)
            .automation
            .iter()
            .all(Option::is_none));

        // An empty block snapshots as `None` — identical to no automation.
        p.tracks[0].automation = Some(Automation::default());
        p.tracks[1].automation = Some(Automation {
            volume: vec![
                AutomationPoint {
                    step: 0.0,
                    value: 1.0,
                },
                AutomationPoint {
                    step: 16.0,
                    value: 0.25,
                },
            ],
            pan: vec![],
        });
        let s = SongData::from_project(&p);
        assert!(s.automation[0].is_none());
        let auto = s.automation[1].as_ref().unwrap();
        assert_eq!(auto.lane(AutomationLane::Volume).len(), 2);
        assert!(auto.lane(AutomationLane::Pan).is_empty());
    }

    // ----- live arpeggiator --------------------------------------------------

    use crate::model::{create_track, ArpRate, Instrument, Note};

    /// A one-track project whose pattern (length `len`) holds `notes` as
    /// (pitch, start, duration, velocity); the clip spans the pattern.
    fn note_project(instrument: Instrument, len: u32, notes: &[(u8, f64, f64, f32)]) -> Project {
        let mut p = Project::empty();
        let mut track = create_track(instrument);
        track.patterns[0].length = len;
        track.patterns[0].notes = notes
            .iter()
            .map(|&(pitch, start, duration, velocity)| Note::new(pitch, start, duration, velocity))
            .collect();
        p.tracks[0] = track;
        p
    }

    /// Scheduled `(clock, pitch, length, velocity)` for track 0's events.
    fn hits(s: &SongData) -> Vec<(f64, u8, f64, f32)> {
        s.events
            .iter()
            .filter(|e| e.track == 0)
            .map(|e| (e.clock, e.pitch, e.end - e.clock, e.velocity))
            .collect()
    }

    fn enable_arp(p: &mut Project, settings: ArpSettings) {
        p.tracks[0].arp = settings;
        p.tracks[0].arp.enabled = true;
    }

    const CHORD: &[(u8, f64, f64, f32)] = &[
        (60, 0.0, 4.0, 0.9),
        (64, 0.0, 4.0, 0.7),
        (67, 0.0, 4.0, 0.8),
    ];

    #[test]
    fn arp_subdivides_chord_into_cycling_hits() {
        let mut p = note_project(Instrument::Piano, 16, CHORD);
        let off = hits(&SongData::from_project(&p));
        enable_arp(&mut p, ArpSettings::default());
        let s = SongData::from_project(&p);
        // A 4-step chord at sixteenth rate: four hits cycling low to high,
        // each 0.9 steps long (the default gate), velocity inherited per
        // chord tone. Durations ride through f32 gate math, so compare the
        // float fields with an epsilon rather than tuple equality.
        let got = hits(&s);
        assert_eq!(got.len(), 4);
        let want = [
            (0.0, 60u8, 0.9f64, 0.9f32),
            (1.0, 64, 0.9, 0.7),
            (2.0, 67, 0.9, 0.8),
            (3.0, 60, 0.9, 0.9),
        ];
        for ((gs, gp, gd, gv), (ws, wp, wd, wv)) in got.iter().zip(want) {
            assert_eq!((*gs, *gp), (ws, wp));
            assert!((gd - wd).abs() < 1e-6, "duration {gd} != {wd}");
            assert!((gv - wv).abs() < 1e-6, "velocity {gv} != {wv}");
        }
        // Disabled: the three written chord tones come through untouched.
        assert_eq!(off.len(), 3);
        assert!(off.iter().all(|(_, _, d, _)| (*d - 4.0).abs() < 1e-9));
    }

    #[test]
    fn arp_single_notes_pass_through_unchanged() {
        let notes = &[
            (60, 0.0, 2.0, 0.9),
            (64, 4.0, 2.0, 0.8),
            (67, 8.0, 2.0, 0.7),
        ];
        let mut p = note_project(Instrument::Piano, 16, notes);
        let before = hits(&SongData::from_project(&p));
        enable_arp(&mut p, ArpSettings::default());
        let after = hits(&SongData::from_project(&p));
        assert_eq!(before.len(), 3);
        assert_eq!(before, after, "a melodic line is not a chord");
    }

    #[test]
    fn arp_rates_and_chord_boundaries() {
        // Eighth-note rate: a hit every 2 steps; a chord ending on the grid
        // gets no hit on its release step.
        let mut p = note_project(Instrument::Piano, 16, CHORD);
        enable_arp(
            &mut p,
            ArpSettings {
                rate: ArpRate::Eighth,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        assert_eq!(
            hits(&s).iter().map(|h| (h.0, h.1)).collect::<Vec<_>>(),
            [(0.0, 60), (2.0, 64)]
        );

        // Thirty-second rate fills half-steps: a 4-step chord, 8 hits.
        enable_arp(
            &mut p,
            ArpSettings {
                rate: ArpRate::ThirtySecond,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        let clocks: Vec<f64> = hits(&s).iter().map(|h| h.0).collect();
        assert_eq!(clocks, [0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5]);

        // A chord whose length is not a multiple of the rate: the last hit
        // is clipped at the chord's release, never past it.
        let mut p = note_project(
            Instrument::Piano,
            16,
            &[(60, 0.0, 3.0, 0.9), (64, 0.0, 3.0, 0.8)],
        );
        enable_arp(
            &mut p,
            ArpSettings {
                rate: ArpRate::Eighth,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        let got = hits(&s);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, 0.0);
        assert_eq!(got[1].0, 2.0);
        assert!((got[1].2 - 1.0).abs() < 1e-9, "clipped to the chord end");
        assert!(got.iter().all(|h| h.0 + h.2 <= 3.0 + 1e-9));
    }

    #[test]
    fn arp_directions_cycle_the_pool() {
        let mut p = note_project(Instrument::Piano, 16, CHORD);
        enable_arp(
            &mut p,
            ArpSettings {
                direction: ArpDirection::Down,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        assert_eq!(
            hits(&s).iter().map(|h| h.1).collect::<Vec<_>>(),
            [67, 64, 60, 67]
        );

        enable_arp(
            &mut p,
            ArpSettings {
                direction: ArpDirection::UpDown,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        assert_eq!(
            hits(&s).iter().map(|h| h.1).collect::<Vec<_>>(),
            [60, 64, 67, 64],
            "UpDown bounces without repeating the endpoints"
        );
    }

    #[test]
    fn arp_octaves_extend_the_pool_upward() {
        // An 8-step chord at sixteenth rate fits the whole 6-tone pool plus
        // the start of the next cycle.
        let mut p = note_project(
            Instrument::Piano,
            16,
            &[
                (60, 0.0, 8.0, 0.9),
                (64, 0.0, 8.0, 0.9),
                (67, 0.0, 8.0, 0.9),
            ],
        );
        enable_arp(
            &mut p,
            ArpSettings {
                octaves: 2,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        assert_eq!(
            hits(&s).iter().map(|h| h.1).collect::<Vec<_>>(),
            [60, 64, 67, 72, 76, 79, 60, 64]
        );
    }

    #[test]
    fn arp_gate_scales_hit_length() {
        let mut p = note_project(Instrument::Piano, 16, CHORD);
        enable_arp(
            &mut p,
            ArpSettings {
                gate: 0.5,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        assert!(hits(&s).iter().all(|h| (h.2 - 0.5).abs() < 1e-9));
    }

    #[test]
    fn arp_block_hits_every_tone() {
        let mut p = note_project(Instrument::Piano, 16, CHORD);
        enable_arp(
            &mut p,
            ArpSettings {
                direction: ArpDirection::Block,
                rate: ArpRate::Quarter,
                ..ArpSettings::default()
            },
        );
        let s = SongData::from_project(&p);
        // One hit at the chord start (4-step chord at a quarter rate),
        // all three tones together.
        let got = hits(&s);
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|h| h.0 == 0.0));
        assert_eq!(got.iter().map(|h| h.1).collect::<Vec<_>>(), [60, 64, 67]);
    }

    #[test]
    fn arp_ignores_drum_tracks() {
        let mut p = note_project(Instrument::Drums, 16, CHORD);
        enable_arp(&mut p, ArpSettings::default());
        let s = SongData::from_project(&p);
        // The kit keeps its three simultaneous hits — an arp would be
        // meaningless on one-shot voices.
        assert_eq!(hits(&s).len(), 3);
        assert!(hits(&s).iter().all(|h| h.0 == 0.0));
    }

    #[test]
    fn arp_random_is_deterministic_and_stays_in_chord() {
        let chord = &[
            (60, 0.0, 16.0, 0.9),
            (64, 0.0, 16.0, 0.8),
            (67, 0.0, 16.0, 0.7),
        ];
        let mut p = note_project(Instrument::Piano, 16, chord);
        enable_arp(
            &mut p,
            ArpSettings {
                direction: ArpDirection::Random,
                ..ArpSettings::default()
            },
        );
        let a = hits(&SongData::from_project(&p));
        let b = hits(&SongData::from_project(&p));
        assert_eq!(a, b, "seeded per chord — identical every render");
        assert_eq!(a.len(), 16);
        assert!(a.iter().all(|h| [60, 64, 67].contains(&h.1)));
        assert!(
            a.windows(2).all(|w| w[0].1 != w[1].1),
            "no immediate repeats"
        );
    }

    #[test]
    fn snapshot_captures_bus_routing_and_gate() {
        let mut p = Project::empty();
        p.tracks.push(create_track(Instrument::Piano));
        // Track 0 routes to Bus A with a non-default fader/pan.
        p.tracks[0].bus = 1;
        p.buses[0].volume = 0.5;
        p.buses[0].pan = 0.5;
        let s = SongData::from_project(&p);
        assert_eq!(s.tracks[0].bus, 1);
        assert_eq!(s.tracks[1].bus, 0, "master-direct stays 0");
        assert_eq!(s.buses.len(), NUM_BUSES);
        assert!(s.buses[0].audible);
        assert!((s.buses[0].gain - fader_gain(0.5)).abs() < 1e-6);
        assert_eq!(s.buses[0].pan, 0.5);

        // A muted bus gates its members at snapshot time: the track neither
        // triggers notes nor feeds the shared sends.
        p.buses[0].mute = true;
        let s = SongData::from_project(&p);
        assert!(!s.tracks[0].audible, "member of a muted bus is gated");
        assert!(s.tracks[1].audible, "master-direct track unaffected");
        assert!(!s.buses[0].audible);

        // A soloed bus isolates its group — the master-direct track drops out.
        p.buses[0].mute = false;
        p.buses[0].solo = true;
        let s = SongData::from_project(&p);
        assert!(s.tracks[0].audible);
        assert!(!s.tracks[1].audible);

        // An unvalidated out-of-range route clamps into the bus array.
        p.buses[0].solo = false;
        p.tracks[0].bus = NUM_BUSES as u8 + 9;
        let s = SongData::from_project(&p);
        assert_eq!(s.tracks[0].bus, NUM_BUSES as u8);
    }

    #[test]
    fn arp_expansion_stays_bounded_for_dense_chords() {
        // One 12-note chord held across the whole 128-bar song per track,
        // Block arp at thirty-second rate over three octaves: each track
        // wants 4096 iterations × 36 tones = 147 456 hits.
        fn dense_chord_project(tracks: usize) -> Project {
            let mut p = Project::empty();
            p.bars = 128;
            p.tracks.clear();
            for _ in 0..tracks {
                let mut t = create_track(Instrument::Piano);
                t.patterns[0].length = 2048;
                t.patterns[0].notes = (60..72)
                    .map(|pitch| Note::new(pitch, 0.0, 2048.0, 0.9))
                    .collect();
                t.arp = ArpSettings {
                    enabled: true,
                    rate: ArpRate::ThirtySecond,
                    octaves: 3,
                    direction: ArpDirection::Block,
                    ..ArpSettings::default()
                };
                p.tracks.push(t);
            }
            p.validate().unwrap();
            p
        }
        // Below the cap the expansion is exact and trivially bounded.
        let s = SongData::from_project(&dense_chord_project(1));
        assert_eq!(s.events.len(), 147_456);
        assert!(s.events.len() <= MAX_SONG_EVENTS);

        // Two tracks want ~295k hits — past MAX_SONG_EVENTS. Expansion stops
        // at the cap; the final Block hit (36 tones at once) is the only
        // possible overshoot, and nothing remains to pass through, so the
        // snapshot lands within one hit of the cap.
        let s = SongData::from_project(&dense_chord_project(2));
        assert!(
            s.events.len() >= MAX_SONG_EVENTS,
            "the cap was reached: {}",
            s.events.len()
        );
        assert!(
            s.events.len() <= MAX_SONG_EVENTS + 36,
            "bounded by the cap plus one Block hit: {}",
            s.events.len()
        );
        assert!(s.events.windows(2).all(|w| w[0].clock <= w[1].clock));
    }

    #[test]
    fn arp_fires_in_every_looped_clip_iteration() {
        // A 16-step pattern holding one 4-step chord, played twice via
        // clip.loops = 2: the arp hits in both iterations — steps 0..3 and
        // 16..19 — and nowhere else.
        let mut p = note_project(Instrument::Piano, 16, CHORD);
        p.bars = 2;
        p.tracks[0].clips[0].loops = 2;
        enable_arp(&mut p, ArpSettings::default());
        let s = SongData::from_project(&p);
        let clocks: Vec<f64> = hits(&s).iter().map(|h| h.0).collect();
        assert_eq!(clocks, [0.0, 1.0, 2.0, 3.0, 16.0, 17.0, 18.0, 19.0]);
        assert!(hits(&s)
            .iter()
            .all(|h| h.0 >= 0.0 && h.0 + h.2 <= s.total_steps + 1e-9));
    }
}
