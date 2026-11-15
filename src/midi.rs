//! Standard MIDI File exchange.
//!
//! **Export** writes a format-1 file at 480 PPQ: a conductor track (song name, tempo, 4/4)
//! followed by one MIDI track per Cadence track with its name, General MIDI program, volume
//! (CC7) and pan (CC10). Drum tracks use channel 10; melodic tracks get their own channels
//! (shared round-robin after fifteen). Every track is exported, including muted ones, and
//! notes sit on the straight grid: swing is a playback feel and is not baked in. Pack presets
//! export as the General MIDI program of their track's instrument.
//!
//! **Import** accepts format 0 and 1 files with metrical timing. Each (MIDI track, channel)
//! pair with notes becomes a Cadence track; channel 10 becomes drums (mapped onto the eight
//! kit voices), other channels choose an instrument from their GM program or, without one,
//! from their register. Positions keep full tick precision (fractional steps). Limitations:
//! only the first tempo is used, time signatures are ignored (Cadence is 4/4), controllers
//! other than initial volume/pan, pitch bend and SysEx are dropped, at most 32 parts and 128
//! bars are kept, and SMPTE-timed or format-2 files are rejected.

use crate::model::{
    create_track, map_drum_pitch, next_id, Clip, Instrument, Note, Pattern, Project, Track,
    MAX_BARS, MAX_BPM, MAX_NOTES_PER_PATTERN, MAX_SONG_EVENTS, MAX_TRACKS, MIN_BPM,
    MIN_NOTE_DURATION, PROJECT_VERSION, STEPS_PER_BAR, STEPS_PER_BEAT,
};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::collections::{HashMap, VecDeque};

/// Ticks per quarter note in exported files.
pub const PPQ: u16 = 480;
const TICKS_PER_STEP: f64 = PPQ as f64 / STEPS_PER_BEAT as f64;
/// Largest MIDI file accepted for import.
pub const MAX_MIDI_BYTES: usize = 8 * 1024 * 1024;
const DRUM_CHANNEL: u8 = 9;
/// Steps per imported pattern when a part is too dense for a single song-length pattern.
const SPLIT_STEPS: u32 = 4 * STEPS_PER_BAR;

/// General MIDI program (0-based) used when exporting `instrument`.
pub fn gm_program(instrument: Instrument) -> u8 {
    match instrument {
        Instrument::Drums => 0,
        Instrument::Piano => 4,   // Electric Piano 1
        Instrument::Bass => 38,   // Synth Bass 1
        Instrument::Pluck => 46,  // Orchestral Harp
        Instrument::Guitar => 24, // Acoustic Guitar (steel)
        Instrument::Pad => 89,    // Pad 2 (warm)
        Instrument::Lead => 81,   // Lead 2 (sawtooth)
    }
}

/// Cadence instrument for a General MIDI program; `average_pitch` breaks ties for parts
/// without a program change.
pub fn instrument_for_program(program: Option<u8>, average_pitch: f64) -> Instrument {
    match program {
        None => {
            if average_pitch < 48.0 {
                Instrument::Bass
            } else {
                Instrument::Piano
            }
        }
        Some(p) => match p {
            0..=7 | 16..=22 => Instrument::Piano,
            8..=15 | 45 | 46 | 104..=119 => Instrument::Pluck,
            23..=30 => Instrument::Guitar, // Guitars: nylon…harmonics
            31..=39 => Instrument::Bass,
            56..=87 => Instrument::Lead,
            _ => Instrument::Pad,
        },
    }
}

fn ticks(step: f64) -> u64 {
    (step * TICKS_PER_STEP).round().max(0.0) as u64
}

/// Encodes `project` as a format-1 Standard MIDI File.
pub fn export_midi(project: &Project) -> Result<Vec<u8>, String> {
    project.validate()?;
    let end_tick = ticks(project.total_steps());
    let all_events = project.events(false);
    let mut smf = Smf::new(Header::new(
        Format::Parallel,
        Timing::Metrical(u15::new(PPQ)),
    ));

    let tempo = (60_000_000.0 / project.bpm)
        .round()
        .clamp(1.0, 16_777_215.0) as u32;
    smf.tracks.push(vec![
        TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::TrackName(project.name.as_bytes())),
        },
        TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(tempo))),
        },
        TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
        },
        TrackEvent {
            delta: u28::new(end_tick as u32),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        },
    ]);

    let mut melodic = 0u8;
    for (index, track) in project.tracks.iter().enumerate() {
        let channel = if track.instrument.is_drums() {
            DRUM_CHANNEL
        } else {
            let c = [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15][melodic as usize % 15];
            melodic = melodic.wrapping_add(1);
            c
        };
        let ch = u4::new(channel);
        // (tick, order, message): note-offs sort before note-ons at the same tick.
        let mut timed: Vec<(u64, u8, MidiMessage)> = vec![
            (
                0,
                0,
                MidiMessage::Controller {
                    controller: u7::new(7),
                    value: u7::new((track.volume * 127.0).round() as u8),
                },
            ),
            (
                0,
                0,
                MidiMessage::Controller {
                    controller: u7::new(10),
                    value: u7::new((64.0 + track.pan * 63.0).round().clamp(0.0, 127.0) as u8),
                },
            ),
        ];
        if !track.instrument.is_drums() {
            timed.push((
                0,
                0,
                MidiMessage::ProgramChange {
                    program: u7::new(gm_program(track.instrument)),
                },
            ));
        }
        let mut notes: Vec<(u64, u64, u8, u8)> = all_events
            .iter()
            .filter(|e| e.track_index == index)
            .map(|e| {
                let start = ticks(e.step);
                let end = ticks(e.step + e.duration).max(start + 1);
                (
                    start,
                    end,
                    e.pitch,
                    ((e.velocity * 127.0).round() as u8).clamp(1, 127),
                )
            })
            .collect();
        notes.sort_by_key(|n| (n.0, n.2));
        // MIDI cannot hold two overlapping notes of one pitch on a channel: end the earlier one.
        let mut last_of_pitch: [Option<usize>; 128] = [None; 128];
        let mut keep = vec![true; notes.len()];
        for i in 0..notes.len() {
            let pitch = notes[i].2 as usize;
            if let Some(prev) = last_of_pitch[pitch] {
                if notes[prev].0 == notes[i].0 {
                    keep[prev] = false;
                } else if notes[prev].1 > notes[i].0 {
                    notes[prev].1 = notes[i].0;
                }
            }
            last_of_pitch[pitch] = Some(i);
        }
        for (i, &(start, end, pitch, vel)) in notes.iter().enumerate() {
            if keep[i] {
                timed.push((
                    start,
                    2,
                    MidiMessage::NoteOn {
                        key: u7::new(pitch),
                        vel: u7::new(vel),
                    },
                ));
                timed.push((
                    end,
                    1,
                    MidiMessage::NoteOff {
                        key: u7::new(pitch),
                        vel: u7::new(64),
                    },
                ));
            }
        }
        timed.sort_by_key(|t| (t.0, t.1));
        let mut events = vec![TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::TrackName(track.name.as_bytes())),
        }];
        let mut now = 0u64;
        for (tick, _, message) in timed {
            events.push(TrackEvent {
                delta: u28::new((tick - now) as u32),
                kind: TrackEventKind::Midi {
                    channel: ch,
                    message,
                },
            });
            now = tick;
        }
        events.push(TrackEvent {
            delta: u28::new(end_tick.saturating_sub(now) as u32),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        smf.tracks.push(events);
    }
    let mut out = Vec::new();
    smf.write_std(&mut out)
        .map_err(|e| format!("Could not encode MIDI: {e}"))?;
    Ok(out)
}

#[derive(Default)]
struct Part {
    midi_track: usize,
    channel: u8,
    notes: Vec<(u64, u64, u8, u8)>,
    program: Option<u8>,
    volume: Option<u8>,
    pan: Option<u8>,
}

fn text(bytes: &[u8]) -> Option<String> {
    let s: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Decodes a Standard MIDI File into a new, validated project.
pub fn import_midi(bytes: &[u8]) -> Result<Project, String> {
    if bytes.is_empty() {
        return Err("The MIDI file is empty.".into());
    }
    if bytes.len() > MAX_MIDI_BYTES {
        return Err(format!(
            "The MIDI file is larger than {} MB.",
            MAX_MIDI_BYTES / (1024 * 1024)
        ));
    }
    let smf = Smf::parse(bytes).map_err(|e| format!("This is not a readable MIDI file ({e})."))?;
    let ppq = match smf.header.timing {
        Timing::Metrical(t) => t.as_int(),
        Timing::Timecode(..) => return Err("MIDI files timed in SMPTE frames are not supported; please export with beats (PPQ) timing.".into()),
    };
    if ppq == 0 {
        return Err("The MIDI file has an invalid time division.".into());
    }
    if smf.header.format == Format::Sequential {
        return Err("Format 2 (sequential) MIDI files are not supported.".into());
    }
    let steps_per_tick = STEPS_PER_BEAT as f64 / ppq as f64;

    let mut tempo: Option<(u64, u32)> = None;
    let mut song_name: Option<String> = None;
    let mut track_names: Vec<Option<String>> = Vec::new();
    let mut parts: Vec<Part> = Vec::new();
    let mut total_notes = 0usize;
    for (ti, track) in smf.tracks.iter().enumerate() {
        let mut now = 0u64;
        let mut name = None;
        let mut index: HashMap<u8, usize> = HashMap::new();
        let mut open: HashMap<(u8, u8), VecDeque<(u64, u8)>> = HashMap::new();
        let mut part_for = |parts: &mut Vec<Part>, channel: u8| -> usize {
            *index.entry(channel).or_insert_with(|| {
                parts.push(Part {
                    midi_track: ti,
                    channel,
                    ..Default::default()
                });
                parts.len() - 1
            })
        };
        for ev in track {
            now += ev.delta.as_int() as u64;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(t)) => {
                    if tempo.is_none_or(|(at, _)| now < at) {
                        tempo = Some((now, t.as_int()));
                    }
                }
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if name.is_none() => name = text(n),
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            open.entry((channel, key.as_int()))
                                .or_default()
                                .push_back((now, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            if let Some((start, vel)) = open
                                .get_mut(&(channel, key.as_int()))
                                .and_then(|q| q.pop_front())
                            {
                                let p = part_for(&mut parts, channel);
                                parts[p]
                                    .notes
                                    .push((start, now.max(start + 1), key.as_int(), vel));
                                total_notes += 1;
                            }
                        }
                        MidiMessage::ProgramChange { program } => {
                            let p = part_for(&mut parts, channel);
                            parts[p].program.get_or_insert(program.as_int());
                        }
                        MidiMessage::Controller { controller, value } => {
                            let p = part_for(&mut parts, channel);
                            match controller.as_int() {
                                7 => {
                                    parts[p].volume.get_or_insert(value.as_int());
                                }
                                10 => {
                                    parts[p].pan.get_or_insert(value.as_int());
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            if total_notes > MAX_SONG_EVENTS {
                return Err(format!(
                    "The MIDI file has more than {MAX_SONG_EVENTS} notes."
                ));
            }
        }
        // Notes never switched off end where the track ends.
        for ((channel, key), queue) in open {
            for (start, vel) in queue {
                let p = part_for(&mut parts, channel);
                let end = now.max(start + (ppq as u64 / 4).max(1));
                parts[p].notes.push((start, end, key, vel));
                total_notes += 1;
            }
        }
        if ti == 0 {
            song_name = name.clone();
        }
        track_names.push(name);
    }
    parts.retain(|p| !p.notes.is_empty());
    if parts.is_empty() {
        return Err("The MIDI file contains no notes.".into());
    }
    if parts.len() > MAX_TRACKS {
        // Keep the busiest parts, in their original order.
        let mut order: Vec<usize> = (0..parts.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(parts[i].notes.len()));
        let mut keep = vec![false; parts.len()];
        for &i in order.iter().take(MAX_TRACKS) {
            keep[i] = true;
        }
        let mut i = 0;
        parts.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }

    let max_steps = (MAX_BARS * STEPS_PER_BAR) as f64;
    let last = parts
        .iter()
        .flat_map(|p| p.notes.iter())
        .map(|n| n.1 as f64 * steps_per_tick)
        .fold(0.0, f64::max)
        .min(max_steps);
    let bars = ((last / STEPS_PER_BAR as f64).ceil() as u32).clamp(1, MAX_BARS);
    let song_steps = bars * STEPS_PER_BAR;

    let mut tracks = Vec::new();
    for part in &parts {
        let drums = part.channel == DRUM_CHANNEL;
        let average = part.notes.iter().map(|n| n.2 as f64).sum::<f64>() / part.notes.len() as f64;
        let instrument = if drums {
            Instrument::Drums
        } else {
            instrument_for_program(part.program, average)
        };
        let mut track = create_track(instrument);
        let channels_in_track = parts
            .iter()
            .filter(|p| p.midi_track == part.midi_track)
            .count();
        track.name = match &track_names[part.midi_track] {
            Some(n) if channels_in_track == 1 => n.clone(),
            Some(n) => format!("{n} (ch {})", part.channel + 1),
            None => format!("{} (ch {})", instrument.name(), part.channel + 1),
        }
        .chars()
        .take(crate::model::MAX_NAME_CHARS)
        .collect();
        if let Some(v) = part.volume {
            track.volume = v as f32 / 127.0;
        }
        if let Some(p) = part.pan {
            track.pan = ((p as f32 - 64.0) / 63.0).clamp(-1.0, 1.0);
        }
        let mut notes: Vec<Note> = Vec::with_capacity(part.notes.len());
        for &(start, end, key, vel) in &part.notes {
            let s = start as f64 * steps_per_tick;
            if s >= song_steps as f64 {
                continue;
            }
            let mut d = ((end - start) as f64 * steps_per_tick).max(MIN_NOTE_DURATION);
            let pitch = if drums {
                d = d.max(0.25);
                map_drum_pitch(key)
            } else {
                key
            };
            notes.push(Note::new(
                pitch,
                s,
                d.min(song_steps as f64 - s).max(MIN_NOTE_DURATION),
                vel as f32 / 127.0,
            ));
        }
        notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
        if drums {
            // Several GM drums can map to one kit voice: keep the loudest hit.
            notes.dedup_by(|b, a| {
                let same = a.pitch == b.pitch && (a.start - b.start).abs() < 1e-9;
                if same {
                    a.velocity = a.velocity.max(b.velocity);
                }
                same
            });
        }
        if notes.is_empty() {
            continue;
        }
        track.patterns.clear();
        track.clips.clear();
        if notes.len() <= MAX_NOTES_PER_PATTERN {
            push_pattern(&mut track, "Imported".into(), 0, song_steps, notes);
        } else {
            for seg in 0..song_steps.div_ceil(SPLIT_STEPS) {
                let from = (seg * SPLIT_STEPS) as f64;
                let len = SPLIT_STEPS.min(song_steps - seg * SPLIT_STEPS);
                let to = from + len as f64;
                let seg_notes: Vec<Note> = notes
                    .iter()
                    .filter(|n| n.start >= from && n.start < to)
                    .take(MAX_NOTES_PER_PATTERN)
                    .map(|n| Note {
                        id: next_id(),
                        start: n.start - from,
                        duration: n.duration.min(to - n.start).max(MIN_NOTE_DURATION),
                        ..n.clone()
                    })
                    .collect();
                if !seg_notes.is_empty() {
                    push_pattern(
                        &mut track,
                        format!("Imported bar {}", seg * SPLIT_STEPS / STEPS_PER_BAR + 1),
                        seg * SPLIT_STEPS,
                        len,
                        seg_notes,
                    );
                }
            }
        }
        tracks.push(track);
    }

    let bpm = tempo
        .map_or(120.0, |(_, us)| {
            ((60_000_000.0 / us.max(1) as f64) * 1000.0).round() / 1000.0
        })
        .clamp(MIN_BPM, MAX_BPM);
    let project = Project {
        version: PROJECT_VERSION,
        id: next_id(),
        name: song_name
            .unwrap_or_else(|| "Imported MIDI".into())
            .chars()
            .take(crate::model::MAX_NAME_CHARS)
            .collect(),
        bpm,
        swing: 0.0,
        bars,
        master_volume: 0.8,
        tracks,
        key: Default::default(),
    };
    project
        .validate()
        .map_err(|e| format!("The imported song is not usable: {e}"))?;
    Ok(project)
}

fn push_pattern(track: &mut Track, name: String, start: u32, length: u32, notes: Vec<Note>) {
    let pattern = Pattern {
        id: next_id(),
        name,
        length,
        notes,
    };
    track.clips.push(Clip {
        id: next_id(),
        pattern_id: pattern.id,
        start: start as f64,
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    });
    track.patterns.push(pattern);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{create_pattern, SongEvent};

    fn header(format: u16, ntracks: u16, division: u16) -> Vec<u8> {
        let mut v = b"MThd\0\0\0\x06".to_vec();
        v.extend(format.to_be_bytes());
        v.extend(ntracks.to_be_bytes());
        v.extend(division.to_be_bytes());
        v
    }

    fn track(body: &[u8]) -> Vec<u8> {
        let mut v = b"MTrk".to_vec();
        v.extend((body.len() as u32).to_be_bytes());
        v.extend(body);
        v
    }

    fn by_track(events: &[SongEvent], i: usize) -> Vec<(f64, f64, u8, f32)> {
        events
            .iter()
            .filter(|e| e.track_index == i)
            .map(|e| (e.step, e.duration, e.pitch, e.velocity))
            .collect()
    }

    #[test]
    fn export_structure() {
        let demo = Project::demo();
        let bytes = export_midi(&demo).unwrap();
        let smf = Smf::parse(&bytes).unwrap();
        assert_eq!(smf.header.format, Format::Parallel);
        assert_eq!(smf.header.timing, Timing::Metrical(u15::new(480)));
        assert_eq!(smf.tracks.len(), demo.tracks.len() + 1);
        assert!(smf.tracks[0].iter().any(|e| matches!(e.kind, TrackEventKind::Meta(MetaMessage::Tempo(t)) if t.as_int() == 576_923)));
        let channels = |t: usize| -> Vec<u8> {
            smf.tracks[t]
                .iter()
                .filter_map(|e| match e.kind {
                    TrackEventKind::Midi {
                        channel,
                        message: MidiMessage::NoteOn { .. },
                    } => Some(channel.as_int()),
                    _ => None,
                })
                .collect()
        };
        assert!(channels(1).iter().all(|&c| c == 9), "drums on channel 10");
        assert!(channels(2).iter().all(|&c| c == 0));
        assert!(channels(3).iter().all(|&c| c == 1));
        assert!(smf.tracks[2].iter().any(|e| matches!(e.kind, TrackEventKind::Midi { message: MidiMessage::ProgramChange { program }, .. } if program.as_int() == 38)));
        let ons = smf.tracks[1..]
            .iter()
            .flatten()
            .filter(|e| {
                matches!(
                    e.kind,
                    TrackEventKind::Midi {
                        message: MidiMessage::NoteOn { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(ons, demo.events(false).len());
    }

    #[test]
    fn demo_round_trips_exactly() {
        let demo = Project::demo();
        let back = import_midi(&export_midi(&demo).unwrap()).unwrap();
        assert_eq!(back.bpm, 104.0);
        assert_eq!(back.bars, 8);
        assert_eq!(back.name, "Afterglow");
        assert_eq!(back.tracks.len(), demo.tracks.len());
        let (a, b) = (demo.events(false), back.events(false));
        for (i, (orig, imported)) in demo.tracks.iter().zip(back.tracks.iter()).enumerate() {
            assert_eq!(imported.instrument, orig.instrument);
            assert_eq!(imported.name, orig.name);
            assert!((imported.pan - orig.pan).abs() < 0.02);
            let (x, y) = (by_track(&a, i), by_track(&b, i));
            assert_eq!(x.len(), y.len(), "track {i}");
            for (p, q) in x.iter().zip(&y) {
                assert!(
                    (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9 && p.2 == q.2,
                    "{p:?} vs {q:?}"
                );
                assert!((p.3 - q.3).abs() <= 0.5 / 127.0 + 1e-6);
            }
        }
    }

    #[test]
    fn fractional_positions_survive() {
        let mut p = Project::empty();
        p.tracks = vec![create_track(Instrument::Lead)];
        let mut pat = create_pattern(Instrument::Lead, 16);
        pat.notes = vec![
            Note::new(70, 1.5, 0.75, 0.5),
            Note::new(72, 2.25, 0.125, 0.9),
        ];
        p.tracks[0].clips[0].pattern_id = pat.id;
        p.tracks[0].patterns = vec![pat];
        let back = import_midi(&export_midi(&p).unwrap()).unwrap();
        let n = &back.tracks[0].patterns[0].notes;
        assert_eq!(
            (n[0].start, n[0].duration, n[1].start, n[1].duration),
            (1.5, 0.75, 2.25, 0.125)
        );
        assert_eq!(back.tracks[0].instrument, Instrument::Lead);
    }

    #[test]
    fn running_status_and_zero_velocity_note_off() {
        // Format 0, 96 PPQ: C4 on, running-status "on" with velocity 0 after a quarter.
        let mut bytes = header(0, 1, 96);
        bytes.extend(track(&[
            0x00, 0x90, 60, 100, 0x60, 60, 0, 0x00, 0x91, 64, 80, 0x30, 0x81, 64, 0, 0x00, 0xFF,
            0x2F, 0x00,
        ]));
        let p = import_midi(&bytes).unwrap();
        assert_eq!(p.tracks.len(), 2, "two channels become two parts");
        let n = &p.tracks[0].patterns[0].notes[0];
        assert_eq!((n.pitch, n.start, n.duration), (60, 0.0, 4.0));
        assert!((n.velocity - 100.0 / 127.0).abs() < 1e-6);
        assert_eq!(p.tracks[1].patterns[0].notes[0].duration, 2.0);
        assert_eq!(p.bpm, 120.0);
    }

    #[test]
    fn drum_channel_maps_to_kit() {
        let mut bytes = header(0, 1, 480);
        // Acoustic bass drum (35) and pedal hi-hat (44) on channel 10.
        bytes.extend(track(&[
            0x00, 0x99, 35, 100, 0x00, 0x99, 44, 90, 0x83, 0x60, 0x89, 35, 0, 0x00, 0x89, 44, 0,
            0x00, 0xFF, 0x2F, 0x00,
        ]));
        let p = import_midi(&bytes).unwrap();
        assert_eq!(p.tracks[0].instrument, Instrument::Drums);
        let pitches: Vec<u8> = p.tracks[0].patterns[0]
            .notes
            .iter()
            .map(|n| n.pitch)
            .collect();
        assert_eq!(pitches, vec![36, 42]);
    }

    #[test]
    fn rejects_bad_files_cleanly() {
        assert!(import_midi(&[]).is_err());
        assert!(import_midi(b"MThd").is_err());
        assert!(import_midi(&[0u8; 64]).is_err());
        assert!(import_midi(&vec![0u8; MAX_MIDI_BYTES + 1])
            .unwrap_err()
            .contains("larger"));
        // SMPTE: -25 fps, 40 ticks per frame.
        let mut smpte = header(1, 1, 0xE728);
        smpte.extend(track(&[
            0x00, 0x90, 60, 100, 0x10, 0x80, 60, 0, 0x00, 0xFF, 0x2F, 0x00,
        ]));
        assert!(import_midi(&smpte).unwrap_err().contains("SMPTE"));
        let mut seq = header(2, 1, 96);
        seq.extend(track(&[
            0x00, 0x90, 60, 100, 0x10, 0x80, 60, 0, 0x00, 0xFF, 0x2F, 0x00,
        ]));
        assert!(import_midi(&seq).unwrap_err().contains("Format 2"));
        let mut silent = header(0, 1, 96);
        silent.extend(track(&[0x00, 0xFF, 0x2F, 0x00]));
        assert!(import_midi(&silent).unwrap_err().contains("no notes"));
        let mut truncated = header(0, 1, 96);
        truncated.extend(b"MTrk\0\0\0\x40\x00\x90");
        assert!(import_midi(&truncated).is_err());
    }

    #[test]
    fn limits_parts_bars_and_dense_patterns() {
        // 40 channels' worth of parts across three tracks, plus a note far past 128 bars.
        let mut smf = Smf::new(Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(96)),
        ));
        for t in 0..3u8 {
            let mut events = Vec::new();
            for ch in 0..16u8 {
                if t * 16 + ch >= 40 {
                    break;
                }
                events.push(TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(ch),
                        message: MidiMessage::NoteOn {
                            key: u7::new(60),
                            vel: u7::new(90),
                        },
                    },
                });
                events.push(TrackEvent {
                    delta: u28::new(24),
                    kind: TrackEventKind::Midi {
                        channel: u4::new(ch),
                        message: MidiMessage::NoteOff {
                            key: u7::new(60),
                            vel: u7::new(0),
                        },
                    },
                });
            }
            events.push(TrackEvent {
                delta: u28::new(96 * 4 * 200),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key: u7::new(62),
                        vel: u7::new(90),
                    },
                },
            });
            events.push(TrackEvent {
                delta: u28::new(24),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOff {
                        key: u7::new(62),
                        vel: u7::new(0),
                    },
                },
            });
            events.push(TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            });
            smf.tracks.push(events);
        }
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        let p = import_midi(&bytes).unwrap();
        assert_eq!(p.tracks.len(), MAX_TRACKS);
        assert_eq!(p.bars, MAX_BARS);
        assert!(p.events(false).iter().all(|e| e.step < p.total_steps()));

        // A part denser than one pattern allows is split into 4-bar patterns. At 96 PPQ a
        // 3-tick note is 1/8 step, so MAX_NOTES_PER_PATTERN + 500 of them span ~1087 steps:
        // dense enough to force the split yet inside the 128-bar song bound (the cap itself
        // is covered by the 200-bar part above).
        let mut dense = Smf::new(Header::new(
            Format::SingleTrack,
            Timing::Metrical(u15::new(96)),
        ));
        let mut events = Vec::new();
        for i in 0..(MAX_NOTES_PER_PATTERN + 500) {
            let key = u7::new(40 + (i % 40) as u8);
            events.push(TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOn {
                        key,
                        vel: u7::new(80),
                    },
                },
            });
            events.push(TrackEvent {
                delta: u28::new(3),
                kind: TrackEventKind::Midi {
                    channel: u4::new(0),
                    message: MidiMessage::NoteOff {
                        key,
                        vel: u7::new(0),
                    },
                },
            });
        }
        events.push(TrackEvent {
            delta: u28::new(0),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        dense.tracks.push(events);
        let mut bytes = Vec::new();
        dense.write_std(&mut bytes).unwrap();
        let p = import_midi(&bytes).unwrap();
        assert!(p.tracks[0].patterns.len() > 1);
        assert_eq!(p.events(false).len(), MAX_NOTES_PER_PATTERN + 500);
    }
}
