//! `bank_audit` — verification, listening reels and showcase songs for the
//! seven `psy-*` instrument packs (128 presets).
//!
//! ```text
//! cargo run --release --locked --offline --example bank_audit -- OUTDIR
//! cargo run --release --example bank_audit -- --write-songs
//! cargo run --release --example bank_audit -- --help
//! ```
//!
//! With `OUTDIR` this writes, and only writes, into that directory:
//!
//! - `audit.csv` / `audit.json` — per-preset metrics (peak, RMS, DC, crest
//!   factor, duration) rendered through the real playback DSP;
//! - `reels/<pack>.wav` + `reels/<pack>.manifest.json` — one listening reel per
//!   pack category. Each preset plays a short musical phrase (held notes,
//!   chords, lines or drum hits at its recommended register) separated by a
//!   clear gap; the manifest maps every preset id to its timestamp;
//! - `songs/*.cadence` — the two showcase songs, generated fresh into OUTDIR;
//! - `previews/*.wav` — rendered previews of both showcase songs.
//!
//! `--write-songs` is the ONLY mode that (re)generates the checked-in
//! `examples/songs/*.cadence` files; a plain `OUTDIR` run never writes outside
//! `OUTDIR`. No user data directory is ever read or written: packs are parsed
//! from the bundled JSON via `include_str!` and all renders are offline.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use cadence_studio::audio::{export_wav, render_project, RenderedAudio};
use cadence_studio::model::{
    create_pattern, next_id, Clip, Effects, Instrument, Note, Pattern, Project, Track,
    MAX_CUTOFF_HZ, STEPS_PER_BAR,
};
use cadence_studio::packs::{self, InstrumentPack, InstrumentPreset};
use cadence_studio::persistence;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Pack sources and audition metadata
// ---------------------------------------------------------------------------

const PSY_PACKS: [(&str, &str); 7] = [
    ("psy-arps", include_str!("../packs/psy-arps/pack.json")),
    ("psy-bass", include_str!("../packs/psy-bass/pack.json")),
    ("psy-brass", include_str!("../packs/psy-brass/pack.json")),
    ("psy-keys", include_str!("../packs/psy-keys/pack.json")),
    ("psy-leads", include_str!("../packs/psy-leads/pack.json")),
    ("psy-rhythm", include_str!("../packs/psy-rhythm/pack.json")),
    ("psy-space", include_str!("../packs/psy-space/pack.json")),
];

const SONG_DIR: &str = "examples/songs";
const REEL_BPM: f64 = 120.0;
const REEL_STEPS_PER_SECOND: f64 = REEL_BPM * 4.0 / 60.0;
const REEL_GAP_STEPS: f64 = 3.0; // ~0.4 s of silence between presets
const REEL_SAMPLE_RATE: u32 = 22_050;
const DRY: Effects = Effects {
    cutoff_hz: MAX_CUTOFF_HZ,
    reverb: 0.0,
    delay: 0.0,
    drive: 0.0,
};

fn load_packs() -> Result<Vec<InstrumentPack>, String> {
    PSY_PACKS
        .iter()
        .map(|(pack_id, json)| {
            packs::parse_pack(json).map_err(|err| format!("pack {pack_id}: {err}"))
        })
        .collect()
}

/// `(pitch, seconds)` audition hint from the optional preset metadata,
/// read tolerantly from the JSON document itself.
fn audition_meta(pack_id: &str, preset_id: &str) -> Option<(u8, f64)> {
    let (_, json) = PSY_PACKS.iter().find(|(id, _)| *id == pack_id)?;
    let value: Value = serde_json::from_str(json).ok()?;
    let preset = value
        .get("instruments")?
        .as_array()?
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(preset_id))?;
    let note = preset.get("audition_note")?.as_u64()?;
    let duration = preset
        .get("audition_duration")
        .and_then(Value::as_f64)
        .unwrap_or(0.8);
    Some((note.min(127) as u8, duration.clamp(0.1, 8.0)))
}

/// Register fallback when a preset carries no `audition_note`: percussion
/// maps onto kit-like pitches, the rest use mid-range family defaults.
fn fallback_pitch(pack_id: &str, preset: &InstrumentPreset) -> u8 {
    if pack_id == "psy-rhythm" {
        let id = preset.id.as_str();
        return if id.contains("kick-deep") {
            24
        } else if id.contains("kick") {
            36
        } else if id.contains("snare") {
            40
        } else if id.contains("rim") {
            38
        } else if id.contains("tom-low") || id.contains("floor-tom") {
            33
        } else if id.contains("tom-high") || id.contains("conga") {
            50
        } else if id.contains("bongo") {
            57
        } else if id.contains("tick") || id.contains("hat") {
            72
        } else {
            60
        };
    }
    match pack_id {
        "psy-bass" => 33,
        "psy-space" => 46,
        "psy-leads" | "psy-arps" => 72,
        _ => 60,
    }
}

fn audition_pitch(pack_id: &str, preset: &InstrumentPreset) -> u8 {
    audition_meta(pack_id, &preset.id)
        .map(|(pitch, _)| pitch)
        .unwrap_or_else(|| fallback_pitch(pack_id, preset))
}

fn editor_for(pack_id: &str) -> Instrument {
    match pack_id {
        // psy-rhythm presets are standalone pitched voices, not kit pieces:
        // they belong in the piano roll, not the eight-row drum grid.
        "psy-rhythm" => Instrument::Piano,
        "psy-bass" => Instrument::Bass,
        "psy-brass" => Instrument::Lead,
        "psy-keys" => Instrument::Piano,
        "psy-arps" => Instrument::Pluck,
        "psy-space" => Instrument::Pad,
        _ => Instrument::Lead,
    }
}

// ---------------------------------------------------------------------------
// Audition phrases
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Percussion,
    Pad,
    Mono,
    Poly,
}

fn role_of(pack_id: &str, preset: &InstrumentPreset) -> Role {
    if pack_id == "psy-rhythm" {
        return Role::Percussion;
    }
    let env = &preset.envelope;
    if env.attack > 0.7 || env.release > 2.5 {
        return Role::Pad;
    }
    if preset.monophonic {
        Role::Mono
    } else {
        Role::Poly
    }
}

fn nt(pitch: u8, start: f64, duration: f64, velocity: f32) -> Note {
    Note::new(pitch, start, duration, velocity)
}

/// A short musical phrase for one preset, in sixteenth-note steps at
/// [`REEL_BPM`]. Returns the pattern (named after the preset) — clips position
/// it on the reel timeline.
fn audition_pattern(
    pack_id: &str,
    preset: &InstrumentPreset,
    pitch: u8,
    meta_seconds: f64,
) -> Pattern {
    let env = &preset.envelope;
    let role = role_of(pack_id, preset);
    let steps = |secs: f64| secs * REEL_STEPS_PER_SECOND;
    let mut notes: Vec<Note> = Vec::new();

    let length = match role {
        Role::Percussion => {
            // A main hit plus two ghosts and an octave answer: shows the
            // attack, the held decay and the release.
            let gate = (f64::from(env.decay) * 1.2).clamp(0.15, 1.6);
            notes.push(nt(pitch, 0.0, steps(gate), 0.95));
            let second = steps(gate) + 2.0;
            notes.push(nt(pitch, second, steps(0.15), 0.6));
            notes.push(nt(pitch + 12, second + 4.0, steps(0.2), 0.45));
            second + 7.0
        }
        Role::Pad => {
            let hold = (f64::from(env.attack) * 1.2 + 0.5).clamp(1.2, 4.0);
            let tones: &[u8] = if preset.monophonic {
                &[0]
            } else {
                &[0, 4, 7, 12]
            };
            for (i, &semi) in tones.iter().enumerate() {
                notes.push(nt(
                    pitch.saturating_add(semi),
                    0.0,
                    steps(hold),
                    0.5 + i as f32 * 0.05,
                ));
            }
            steps(hold) + 2.0
        }
        Role::Mono => {
            // Five-note motif around the audition pitch: shows pitch tracking
            // and the preset's own articulation.
            let gate = meta_seconds.clamp(0.3, 1.0);
            let motif: [u8; 5] = [0, 7, 12, 9, 5];
            for (i, &semi) in motif.iter().enumerate() {
                let start = i as f64 * 3.0;
                notes.push(nt(
                    pitch.saturating_add(semi),
                    start,
                    steps(gate),
                    0.5 + i as f32 * 0.09,
                ));
            }
            12.0 + steps(gate) + 2.0
        }
        Role::Poly => {
            // Chord stab then a sustained chord: both articulations in one slot.
            let stab = steps(meta_seconds.clamp(0.15, 0.5));
            let hold = (f64::from(env.attack) * 1.2 + 0.4).clamp(0.8, 2.2);
            let tones: [u8; 4] = [0, 4, 7, 12];
            for (i, &semi) in tones.iter().enumerate() {
                let vel = 0.5 + i as f32 * 0.04;
                notes.push(nt(pitch.saturating_add(semi), 0.0, stab, vel));
                notes.push(nt(pitch.saturating_add(semi), 8.0, steps(hold), vel));
            }
            8.0 + steps(hold) + 2.0
        }
    };

    let mut pattern = create_pattern(editor_for(pack_id), length.ceil() as u32);
    pattern.name = format!("Audition {}", preset.name);
    pattern.notes = notes;
    pattern.sort_notes();
    pattern
}

fn tail_seconds(preset: &InstrumentPreset) -> f64 {
    let env = &preset.envelope;
    (f64::from(env.release) * 1.2 + f64::from(env.decay) * 0.6 + 0.8).clamp(1.4, 7.0)
}

fn preset_track(pack_id: &str, preset: &InstrumentPreset) -> (Track, Pattern) {
    let (pitch, secs) =
        audition_meta(pack_id, &preset.id).unwrap_or((fallback_pitch(pack_id, preset), 0.8));
    let pattern = audition_pattern(pack_id, preset, pitch, secs);
    let mut track = Track::from_preset(editor_for(pack_id), preset.clone());
    track.name = preset.name.chars().take(60).collect();
    track.volume = 0.75;
    track.pan = 0.0;
    track.effects = Effects { reverb: 0.1, ..DRY };
    (track, pattern)
}

// ---------------------------------------------------------------------------
// Rendered-audio metrics
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Metrics {
    peak: f32,
    rms: f64,
    dc: f64,
    crest: f64,
    seconds: f64,
}

fn frame_metrics(audio: &RenderedAudio) -> Metrics {
    let frames = audio.left.len().max(1);
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    for i in 0..frames {
        let l = audio.left[i];
        let r = audio.right[i];
        peak = peak.max(l.abs()).max(r.abs());
        sum += 0.5 * (l as f64 + r as f64);
        sum_sq += (l as f64).powi(2) + (r as f64).powi(2);
    }
    let rms = (sum_sq / (frames * 2) as f64).sqrt();
    Metrics {
        peak,
        rms,
        dc: sum / frames as f64,
        crest: if rms > 1e-9 { peak as f64 / rms } else { 0.0 },
        seconds: frames as f64 / audio.sample_rate as f64,
    }
}

// ---------------------------------------------------------------------------
// Audit output
// ---------------------------------------------------------------------------

fn audit_preset(pack_id: &str, preset: &InstrumentPreset) -> Result<(Value, Metrics), String> {
    let (mut track, pattern) = preset_track(pack_id, preset);
    track.effects = DRY;
    let pattern_len = pattern.length as f64;
    let pattern_id = pattern.id;
    track.patterns = vec![pattern];
    track.clips = vec![Clip {
        id: next_id(),
        pattern_id,
        start: 0.0,
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    }];

    let mut project = Project::empty();
    project.name = format!("Audit {}", preset.id);
    project.bpm = REEL_BPM;
    project.swing = 0.0;
    project.bars = (pattern_len as u32).div_ceil(STEPS_PER_BAR).max(1);
    project.master_volume = 0.85;
    project.tracks = vec![track];
    project
        .validate()
        .map_err(|err| format!("{}: invalid audition project: {err}", preset.id))?;

    let audio = render_project(
        &project,
        REEL_SAMPLE_RATE,
        project.duration_seconds() + tail_seconds(preset),
    )?;
    let metrics = frame_metrics(&audio);
    let phrase_seconds = pattern_len / REEL_STEPS_PER_SECOND;
    let role = role_of(pack_id, preset);
    let entry = json!({
        "pack": pack_id,
        "id": preset.id,
        "name": preset.name,
        "role": format!("{role:?}").to_lowercase(),
        "monophonic": preset.monophonic,
        "audition_pitch": audition_pitch(pack_id, preset),
        "phrase_seconds": phrase_seconds,
        "render_seconds": metrics.seconds,
        "peak": metrics.peak,
        "rms": metrics.rms,
        "dc": metrics.dc,
        "crest": metrics.crest,
    });
    Ok((entry, metrics))
}

fn write_audit(out_dir: &Path, entries: &[Value]) -> Result<(), String> {
    // JSON audit: the full record.
    let json_path = out_dir.join("audit.json");
    fs::write(
        &json_path,
        serde_json::to_string_pretty(entries)
            .map_err(|err| format!("cannot serialize audit: {err}"))?,
    )
    .map_err(|err| format!("cannot write {}: {err}", json_path.display()))?;

    // CSV audit: the same numbers in a spreadsheet-friendly shape.
    let csv_path = out_dir.join("audit.csv");
    let mut csv = String::from(
        "pack,id,name,role,monophonic,audition_pitch,phrase_s,render_s,peak,rms,dc,crest\n",
    );
    for entry in entries {
        let get = |key: &str| entry.get(key).cloned().unwrap_or(Value::Null);
        let text = |key: &str| match get(key) {
            Value::String(s) => s,
            other => other.to_string(),
        };
        let num = |key: &str| match get(key) {
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        let name = text("name").replace(',', ";");
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}\n",
            text("pack"),
            text("id"),
            name,
            text("role"),
            text("monophonic"),
            num("audition_pitch"),
            num("phrase_seconds"),
            num("render_seconds"),
            num("peak"),
            num("rms"),
            num("dc"),
            num("crest"),
        ));
    }
    fs::write(&csv_path, csv)
        .map_err(|err| format!("cannot write {}: {err}", csv_path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Listening reels
// ---------------------------------------------------------------------------

struct ReelEntry {
    id: String,
    name: String,
    start_seconds: f64,
    phrase_seconds: f64,
    slot_seconds: f64,
}

fn build_reel(pack: &InstrumentPack, pack_id: &str) -> Result<(Project, Vec<ReelEntry>), String> {
    let mut project = Project::empty();
    project.name = format!("{} audition reel", pack.name);
    project.bpm = REEL_BPM;
    project.swing = 0.0;
    project.master_volume = 0.85;

    let mut cursor = 0.0f64;
    let mut entries = Vec::new();
    for preset in &pack.instruments {
        let (mut track, pattern) = preset_track(pack_id, preset);
        let pattern_id = pattern.id;
        let pattern_steps = pattern.length as f64;
        let phrase_seconds = pattern_steps / REEL_STEPS_PER_SECOND;
        let slot_seconds = phrase_seconds + REEL_GAP_STEPS / REEL_STEPS_PER_SECOND;
        track.clips = vec![Clip {
            id: next_id(),
            pattern_id,
            start: cursor,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        }];
        track.patterns = vec![pattern];
        project.tracks.push(track);
        entries.push(ReelEntry {
            id: preset.id.clone(),
            name: preset.name.clone(),
            start_seconds: cursor / REEL_STEPS_PER_SECOND,
            phrase_seconds,
            slot_seconds,
        });
        cursor += pattern_steps + REEL_GAP_STEPS;
    }
    project.bars = (cursor.ceil() as u32).div_ceil(STEPS_PER_BAR).max(1);
    project
        .validate()
        .map_err(|err| format!("reel for {pack_id} is invalid: {err}"))?;
    Ok((project, entries))
}

/// Writes interleaved stereo f32 audio as a 16-bit PCM WAV.
fn write_wav_i16(path: &Path, sample_rate: u32, audio: &RenderedAudio) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|err| format!("cannot create {}: {err}", path.display()))?;
    for (&l, &r) in audio.left.iter().zip(audio.right.iter()) {
        for x in [l, r] {
            let v = (x.clamp(-1.0, 1.0) * 32767.0).round() as i32;
            writer
                .write_sample(v.clamp(-32768, 32767) as i16)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
        }
    }
    writer
        .finalize()
        .map_err(|err| format!("cannot finish {}: {err}", path.display()))
}

fn write_reels(out_dir: &Path, packs: &[InstrumentPack]) -> Result<(), String> {
    let reels_dir = out_dir.join("reels");
    fs::create_dir_all(&reels_dir)
        .map_err(|err| format!("cannot create {}: {err}", reels_dir.display()))?;
    for (pack_index, pack) in packs.iter().enumerate() {
        let pack_id = PSY_PACKS[pack_index].0;
        let (project, entries) = build_reel(pack, pack_id)?;
        // The tail only needs to cover the slowest release in the pack; the
        // offline render stops early once the signal is quiet anyway.
        let tail = pack
            .instruments
            .iter()
            .map(tail_seconds)
            .fold(0.0f64, f64::max);
        let audio = render_project(
            &project,
            REEL_SAMPLE_RATE,
            project.duration_seconds() + tail,
        )?;
        let wav_path = reels_dir.join(format!("{pack_id}.wav"));
        write_wav_i16(&wav_path, REEL_SAMPLE_RATE, &audio)?;

        let manifest: Vec<Value> = entries
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "name": e.name,
                    "start_seconds": (e.start_seconds * 1000.0).round() / 1000.0,
                    "phrase_seconds": (e.phrase_seconds * 1000.0).round() / 1000.0,
                    "slot_seconds": (e.slot_seconds * 1000.0).round() / 1000.0,
                })
            })
            .collect();
        let manifest_path = reels_dir.join(format!("{pack_id}.manifest.json"));
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest)
                .map_err(|err| format!("cannot serialize manifest: {err}"))?,
        )
        .map_err(|err| format!("cannot write {}: {err}", manifest_path.display()))?;
        println!(
            "  reel {:<12} {:>3} presets, {:>6.1} s -> {}",
            pack_id,
            entries.len(),
            audio.left.len() as f64 / REEL_SAMPLE_RATE as f64,
            wav_path.display()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Showcase songs
// ---------------------------------------------------------------------------
//
// Both songs embed the full pack presets in their tracks, so the .cadence
// files play anywhere without the packs installed.

const DRIVE_STEPS: u32 = 16 * STEPS_PER_BAR; // 16 bars

/// `(start_step, pitch, duration_steps, velocity)` for a composed lead note.
type LeadNote = (f64, u8, f64, f32);
/// A melody fragment: the bar it starts on, plus its notes.
type LeadPhrase = (usize, &'static [LeadNote]);

/// One chord per bar of the A-minor cycle: Am7, Fmaj7, Cmaj7, G7.
struct Chord {
    bass: u8,
    keys: [u8; 4],
    pad: [u8; 4],
}

const DRIVE_CHORDS: [Chord; 4] = [
    Chord {
        bass: 33,
        keys: [57, 60, 64, 67],
        pad: [57, 60, 64, 71],
    },
    Chord {
        bass: 29,
        keys: [53, 57, 60, 64],
        pad: [53, 57, 60, 65],
    },
    Chord {
        bass: 36,
        keys: [60, 64, 67, 71],
        pad: [55, 60, 64, 71],
    },
    Chord {
        bass: 31,
        keys: [55, 59, 62, 65],
        pad: [55, 59, 62, 67],
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Intro,
    Main,
    Break,
    Finale,
}

fn drive_section(bar: usize) -> Section {
    match bar {
        0..=1 => Section::Intro,
        2..=5 => Section::Main,
        6..=7 => Section::Break,
        8..=11 => Section::Main,
        12..=13 => Section::Break,
        _ => Section::Finale,
    }
}

fn dub_section(bar: usize) -> Section {
    match bar {
        0..=1 => Section::Intro,
        2..=5 => Section::Main,
        6..=7 => Section::Break,
        8..=11 => Section::Main,
        _ => Section::Finale,
    }
}

struct SongBuilder<'a> {
    presets: &'a HashMap<String, InstrumentPreset>,
}

impl<'a> SongBuilder<'a> {
    fn preset(&self, id: &str) -> InstrumentPreset {
        self.presets
            .get(id)
            .unwrap_or_else(|| panic!("pack preset {id} missing"))
            .clone()
    }

    /// A preset track with one pattern covering `length` steps.
    #[allow(clippy::too_many_arguments)]
    fn track(
        &self,
        editor: Instrument,
        preset_id: &str,
        name: &str,
        volume: f32,
        pan: f32,
        effects: Effects,
        length: u32,
        notes: Vec<Note>,
    ) -> Track {
        let mut track = Track::from_preset(editor, self.preset(preset_id));
        track.name = name.to_string();
        track.volume = volume;
        track.pan = pan;
        track.effects = effects;
        let mut pattern = create_pattern(editor, length);
        pattern.name = name.to_string();
        pattern.notes = notes;
        pattern.sort_notes();
        let pattern_id = pattern.id;
        track.patterns = vec![pattern];
        track.clips = vec![Clip {
            id: next_id(),
            pattern_id,
            start: 0.0,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        }];
        track
    }
}

fn song_project(name: &str, bpm: f64, swing: f64, master: f32, tracks: Vec<Track>) -> Project {
    let mut project = Project::empty();
    project.name = name.to_string();
    project.bpm = bpm;
    project.swing = swing;
    project.bars = DRIVE_STEPS / STEPS_PER_BAR;
    project.master_volume = master;
    project.tracks = tracks;
    project
        .validate()
        .unwrap_or_else(|err| panic!("showcase song {name} is invalid: {err}"));
    project
}

// -- Song 1: "Neon Circuits" — driving psychedelic/electronic, 140 BPM, A minor

fn build_drive_song(presets: &HashMap<String, InstrumentPreset>) -> Project {
    let b = SongBuilder { presets };
    let mut tracks = Vec::new();

    // -- Drums ------------------------------------------------------------
    let kick_pitch = 36u8;
    let snare_pitch = 40u8;
    let hat_pitch = 64u8;
    let open_pitch = 64u8;
    let crash_pitch = 60u8;
    let tom_pitch = 43u8;

    let mut kick = Vec::new();
    let mut snare = Vec::new();
    let mut clap = Vec::new();
    let mut hat = Vec::new();
    let mut open_hat = Vec::new();
    let mut crash = Vec::new();
    let mut toms = Vec::new();

    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let section = drive_section(bar);

        // Kick: four-on-the-floor in main/finale, sparse in intro/break.
        match section {
            Section::Intro => {
                kick.push(nt(kick_pitch, s, 1.4, 0.9));
                kick.push(nt(kick_pitch, s + 8.0, 1.4, 0.75));
            }
            Section::Break => {
                kick.push(nt(kick_pitch, s, 1.4, 0.9));
                kick.push(nt(kick_pitch, s + 10.0, 1.0, 0.6));
            }
            _ => {
                for &st in &[0.0, 4.0, 8.0, 12.0] {
                    kick.push(nt(kick_pitch, s + st, 1.2, 0.95));
                }
                kick.push(nt(kick_pitch, s + 14.0, 0.8, 0.55));
            }
        }

        // Snare backbeat on 2 and 4; clap layer on the 4.
        match section {
            Section::Main | Section::Finale => {
                snare.push(nt(snare_pitch, s + 4.0, 1.0, 0.85));
                snare.push(nt(snare_pitch, s + 12.0, 1.0, 0.9));
                clap.push(nt(60, s + 12.0, 1.0, 0.35));
                // Ghost notes keep the pocket alive.
                snare.push(nt(snare_pitch, s + 7.0, 0.4, 0.2));
                snare.push(nt(snare_pitch, s + 15.0, 0.4, 0.22));
            }
            Section::Break => {
                snare.push(nt(snare_pitch, s + 15.0, 0.5, 0.18));
            }
            Section::Intro => {}
        }

        // Hats: 8ths with accents; 16th ghosts in the main sections.
        for i in 0..8 {
            let st = i as f64 * 2.0;
            let (vel, dur) = match section {
                Section::Intro => (0.3, 0.5),
                Section::Break => (0.32, 0.5),
                _ => (if i % 2 == 0 { 0.5 } else { 0.36 }, 0.5),
            };
            hat.push(nt(hat_pitch, s + st, dur, vel));
        }
        if matches!(section, Section::Main | Section::Finale) {
            for i in 0..8 {
                hat.push(nt(hat_pitch, s + i as f64 * 2.0 + 1.0, 0.4, 0.16));
            }
            open_hat.push(nt(open_pitch, s + 14.0, 1.2, 0.4));
        }

        // Crash on section downbeats.
        if matches!(section, Section::Intro | Section::Break | Section::Finale) {
            crash.push(nt(crash_pitch, s, 3.0, 0.6));
        }

        // Drum fill into the finale and a pickup at the end of each main block.
        if bar == 13 {
            for (i, &st) in [12.0, 13.0, 14.0, 15.0].iter().enumerate() {
                toms.push(nt(tom_pitch + i as u8, s + st, 0.9, 0.5 + i as f32 * 0.12));
            }
        }
        if bar == 15 {
            for (i, &st) in [12.0, 13.0, 14.0, 15.0].iter().enumerate() {
                toms.push(nt(
                    tom_pitch + 3 - i as u8,
                    s + st,
                    0.9,
                    0.55 + i as f32 * 0.1,
                ));
            }
            snare.push(nt(snare_pitch, s + 15.5, 0.5, 0.5));
        }
    }

    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.kick-techno",
        "Techno kick",
        0.8,
        -0.1,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.08,
            delay: 0.0,
            drive: 0.1,
        },
        DRIVE_STEPS,
        kick,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.snare-tight",
        "Snare",
        0.72,
        0.05,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.12,
            delay: 0.0,
            drive: 0.05,
        },
        DRIVE_STEPS,
        snare,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.clap",
        "Clap layer",
        0.4,
        0.15,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.2,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        clap,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.hat-closed",
        "Closed hats",
        0.55,
        0.1,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.05,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        hat,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.hat-open",
        "Open hat",
        0.45,
        -0.15,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.1,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        open_hat,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.cymbal",
        "Cymbal crashes",
        0.5,
        0.0,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.25,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        crash,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.psy-tom",
        "Tom fills",
        0.6,
        0.0,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.15,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        toms,
    ));

    // -- Bass -------------------------------------------------------------
    // psy-roll: 16th-note rolling lines on the chord root in the main
    // sections, long sub notes in the intro and breakdown.
    let mut roll = Vec::new();
    let mut sub = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let root = DRIVE_CHORDS[bar % 4].bass;
        match drive_section(bar) {
            Section::Intro => {
                roll.push(nt(root, s, 7.0, 0.7));
                roll.push(nt(root + 12, s + 8.0, 7.0, 0.6));
                sub.push(nt(root - 12, s, 15.0, 0.65));
            }
            Section::Break => {
                roll.push(nt(root, s, 5.0, 0.65));
                roll.push(nt(root + 7, s + 8.0, 5.0, 0.6));
                sub.push(nt(root - 12, s, 15.0, 0.6));
            }
            _ => {
                // Rolling 16ths: root anchors with octave and fifth pops.
                let line: [(f64, u8, f32); 16] = [
                    (0.0, 0, 0.8),
                    (1.0, 0, 0.6),
                    (2.0, 0, 0.6),
                    (3.0, 12, 0.7),
                    (4.0, 0, 0.8),
                    (5.0, 0, 0.6),
                    (6.0, 7, 0.65),
                    (7.0, 0, 0.6),
                    (8.0, 0, 0.8),
                    (9.0, 0, 0.6),
                    (10.0, 12, 0.7),
                    (11.0, 0, 0.6),
                    (12.0, 7, 0.65),
                    (13.0, 0, 0.6),
                    (14.0, 10, 0.6),
                    (15.0, 12, 0.7),
                ];
                for (st, semi, vel) in line {
                    roll.push(nt(root + semi, s + st, 0.9, vel));
                }
                sub.push(nt(root - 12, s, 7.0, 0.6));
                sub.push(nt(root - 12, s + 8.0, 7.0, 0.55));
            }
        }
    }
    tracks.push(b.track(
        Instrument::Bass,
        "psy-bass.psy-roll",
        "Rolling bass",
        0.78,
        -0.05,
        Effects {
            cutoff_hz: 3200.0,
            reverb: 0.04,
            delay: 0.0,
            drive: 0.18,
        },
        DRIVE_STEPS,
        roll,
    ));
    tracks.push(b.track(
        Instrument::Bass,
        "psy-bass.sub-sine",
        "Sub layer",
        0.7,
        0.0,
        Effects {
            cutoff_hz: 1200.0,
            reverb: 0.02,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        sub,
    ));

    // -- Organ ------------------------------------------------------------
    // psy-organ stabs on the offbeats in the main sections, sustained
    // chords in the intro and breakdown.
    let mut organ = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = &DRIVE_CHORDS[bar % 4];
        match drive_section(bar) {
            Section::Intro | Section::Break => {
                for &p in &chord.keys {
                    organ.push(nt(p, s, 15.0, 0.4));
                }
            }
            _ => {
                for &st in &[2.0, 6.0, 10.0, 14.0] {
                    for (i, &p) in chord.keys.iter().enumerate() {
                        organ.push(nt(p, s + st, 1.4, 0.42 + i as f32 * 0.03));
                    }
                }
            }
        }
    }
    tracks.push(b.track(
        Instrument::Piano,
        "psy-keys.psy-organ",
        "Psy organ",
        0.5,
        0.15,
        Effects {
            cutoff_hz: 6500.0,
            reverb: 0.22,
            delay: 0.12,
            drive: 0.0,
        },
        DRIVE_STEPS,
        organ,
    ));

    // -- Brass stabs ------------------------------------------------------
    let mut stabs = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = &DRIVE_CHORDS[bar % 4];
        if matches!(drive_section(bar), Section::Main | Section::Finale) {
            // Backbeat stabs using the top three chord tones.
            for &st in &[4.0, 12.0] {
                for (i, &p) in chord.keys[1..].iter().enumerate() {
                    stabs.push(nt(p + 12, s + st, 1.2, 0.4 + i as f32 * 0.05));
                }
            }
        }
    }
    tracks.push(b.track(
        Instrument::Lead,
        "psy-brass.detuned-stab",
        "Detuned stabs",
        0.55,
        -0.2,
        Effects {
            cutoff_hz: 8000.0,
            reverb: 0.25,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        stabs,
    ));

    // -- Arpeggio ---------------------------------------------------------
    // 16th-note up-down arps through the chord tones outside the intro.
    let mut arp = Vec::new();
    for bar in 2..16usize {
        let s = (bar * 16) as f64;
        let chord = &DRIVE_CHORDS[bar % 4];
        // Arpeggio pool: chord tones plus octave colour.
        let pool = [
            chord.keys[0],
            chord.keys[1],
            chord.keys[2],
            chord.keys[3] + 12,
            chord.keys[2],
            chord.keys[1],
        ];
        if !matches!(drive_section(bar), Section::Break) {
            for i in 0..16 {
                let idx = [0, 1, 2, 3, 4, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5][i];
                let vel = if i % 4 == 0 { 0.5 } else { 0.34 };
                arp.push(nt(pool[idx], s + i as f64, 0.9, vel));
            }
        } else {
            // Breakdown: sparse high echoes of the chord.
            arp.push(nt(chord.keys[3] + 12, s, 3.0, 0.3));
            arp.push(nt(chord.keys[2] + 12, s + 8.0, 3.0, 0.26));
        }
    }
    tracks.push(b.track(
        Instrument::Pluck,
        "psy-arps.acid-pluck",
        "Acid arp",
        0.48,
        0.25,
        Effects {
            cutoff_hz: 7000.0,
            reverb: 0.18,
            delay: 0.3,
            drive: 0.0,
        },
        DRIVE_STEPS,
        arp,
    ));

    // -- Lead -------------------------------------------------------------
    // A-melody in the main blocks and the finale; rests elsewhere.
    let mut lead = Vec::new();
    let phrases: &[LeadPhrase] = &[
        // bars 2-5 (steps 32..95): call
        (
            2,
            &[
                (0.0, 76, 2.5, 0.6),
                (3.0, 74, 1.5, 0.5),
                (5.0, 72, 2.0, 0.52),
                (8.0, 71, 3.0, 0.55),
                (12.0, 69, 2.0, 0.5),
                (14.0, 67, 2.5, 0.48),
            ],
        ),
        // bars 6-7 break: airy answer, high and quiet
        (6, &[(0.0, 79, 6.0, 0.4), (8.0, 76, 6.0, 0.38)]),
        // bars 8-11: answer phrase
        (
            8,
            &[
                (0.0, 74, 2.0, 0.58),
                (3.0, 72, 1.5, 0.5),
                (5.0, 76, 2.5, 0.55),
                (8.0, 79, 3.0, 0.6),
                (12.0, 81, 2.0, 0.55),
                (14.0, 79, 2.5, 0.5),
            ],
        ),
        // bars 14-15 finale: resolve home
        (
            14,
            &[
                (0.0, 76, 3.0, 0.62),
                (4.0, 74, 1.5, 0.52),
                (6.0, 72, 2.0, 0.55),
                (8.0, 71, 4.0, 0.58),
                (13.0, 69, 3.0, 0.55),
            ],
        ),
    ];
    for &(bar_start, notes) in phrases {
        for &(st, pitch, dur, vel) in notes.iter() {
            lead.push(nt(pitch, bar_start as f64 * 16.0 + st, dur, vel));
        }
    }
    tracks.push(b.track(
        Instrument::Lead,
        "psy-leads.trance-lead",
        "Trance lead",
        0.55,
        0.1,
        Effects {
            cutoff_hz: 9000.0,
            reverb: 0.25,
            delay: 0.35,
            drive: 0.0,
        },
        DRIVE_STEPS,
        lead,
    ));

    // -- Pads -------------------------------------------------------------
    let mut pad = Vec::new();
    let mut air = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = &DRIVE_CHORDS[bar % 4];
        // Cosmic pad: sustained chords everywhere, breathing through the mix.
        let (vel, dur) = match drive_section(bar) {
            Section::Break => (0.34, 15.0),
            _ => (0.42, 15.5),
        };
        for &p in &chord.pad {
            pad.push(nt(p, s, dur, vel));
        }
        // Airy shimmer: high colour only in the intro, break and finale.
        if matches!(
            drive_section(bar),
            Section::Intro | Section::Break | Section::Finale
        ) {
            for &p in &chord.pad {
                air.push(nt(p + 12, s, 15.0, 0.22));
            }
        }
    }
    tracks.push(b.track(
        Instrument::Pad,
        "psy-space.cosmic-pad",
        "Cosmic pad",
        0.46,
        -0.1,
        Effects {
            cutoff_hz: 5000.0,
            reverb: 0.45,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        pad,
    ));
    tracks.push(b.track(
        Instrument::Pad,
        "psy-space.airy-shimmer",
        "Air shimmer",
        0.4,
        0.2,
        Effects {
            cutoff_hz: 12000.0,
            reverb: 0.5,
            delay: 0.1,
            drive: 0.0,
        },
        DRIVE_STEPS,
        air,
    ));

    song_project("Neon Circuits", 140.0, 0.08, 0.78, tracks)
}

// -- Song 2: "Midnight Dub" — dub/downtempo, 96 BPM, D minor

/// Two bars per chord: Dm, Bb, F, C — i-bVI-bIII-bVII in D minor.
struct DubChord {
    bass: u8,
    organ: [u8; 4],
    pad: [u8; 4],
}

const DUB_CHORDS: [DubChord; 4] = [
    DubChord {
        bass: 38,
        organ: [50, 53, 57, 60],
        pad: [50, 53, 57, 62],
    }, // Dm
    DubChord {
        bass: 34,
        organ: [46, 50, 53, 58],
        pad: [46, 50, 53, 58],
    }, // Bb
    DubChord {
        bass: 29,
        organ: [53, 57, 60, 65],
        pad: [53, 57, 60, 65],
    }, // F
    DubChord {
        bass: 36,
        organ: [48, 52, 55, 60],
        pad: [48, 52, 55, 60],
    }, // C
];

fn dub_chord(bar: usize) -> &'static DubChord {
    &DUB_CHORDS[(bar / 2) % 4]
}

fn build_dub_song(presets: &HashMap<String, InstrumentPreset>) -> Project {
    let b = SongBuilder { presets };
    let mut tracks = Vec::new();

    let kick_pitch = 36u8;
    let snare_pitch = 40u8;
    let hat_pitch = 64u8;
    let shaker_pitch = 60u8;
    let rim_pitch = 38u8;

    let mut kick = Vec::new();
    let mut snare = Vec::new();
    let mut rim = Vec::new();
    let mut hat = Vec::new();
    let mut shaker = Vec::new();
    let mut crash = Vec::new();

    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let section = dub_section(bar);

        // Half-time kick: deep on 1, syncopated answers, sparse in breakdown.
        match section {
            Section::Intro => {
                kick.push(nt(kick_pitch, s, 2.0, 0.9));
                kick.push(nt(kick_pitch, s + 8.0, 1.5, 0.7));
            }
            Section::Break => {
                kick.push(nt(kick_pitch, s, 2.0, 0.85));
                kick.push(nt(kick_pitch, s + 11.0, 1.2, 0.55));
            }
            _ => {
                kick.push(nt(kick_pitch, s, 2.0, 0.95));
                kick.push(nt(kick_pitch, s + 10.0, 1.4, 0.7));
                if bar % 2 == 1 {
                    kick.push(nt(kick_pitch, s + 7.0, 1.0, 0.5));
                }
            }
        }

        // Snare on the 3 (half-time backbeat); rim ghosts elsewhere.
        match section {
            Section::Intro => {}
            Section::Break => {
                snare.push(nt(snare_pitch, s + 8.0, 1.5, 0.5));
                rim.push(nt(rim_pitch, s + 4.0, 0.5, 0.25));
            }
            _ => {
                snare.push(nt(snare_pitch, s + 8.0, 1.5, 0.85));
                snare.push(nt(snare_pitch, s + 15.0, 0.5, 0.22));
                rim.push(nt(rim_pitch, s + 4.0, 0.5, 0.3));
                rim.push(nt(rim_pitch, s + 12.0, 0.5, 0.28));
            }
        }

        // Soft 8th hats and a shaker 16ths for the dub lilt.
        for i in 0..8 {
            hat.push(nt(
                hat_pitch,
                s + i as f64 * 2.0,
                0.6,
                if i % 2 == 0 { 0.34 } else { 0.24 },
            ));
        }
        for i in 0..16 {
            shaker.push(nt(
                shaker_pitch,
                s + i as f64,
                0.5,
                if i % 2 == 0 { 0.3 } else { 0.18 },
            ));
        }

        // Cymbal only on the two big section entrances.
        if bar == 0 || bar == 8 {
            crash.push(nt(60, s, 4.0, 0.55));
        }
    }

    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.kick-808",
        "808 kick",
        0.85,
        0.0,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.1,
            delay: 0.0,
            drive: 0.15,
        },
        DRIVE_STEPS,
        kick,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.snare-long",
        "Snare",
        0.68,
        0.05,
        Effects {
            cutoff_hz: 6000.0,
            reverb: 0.3,
            delay: 0.1,
            drive: 0.0,
        },
        DRIVE_STEPS,
        snare,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.rim",
        "Rim ghosts",
        0.4,
        -0.1,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.15,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        rim,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.hat-closed",
        "Hat",
        0.45,
        0.1,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.08,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        hat,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.shaker",
        "Shaker",
        0.4,
        -0.15,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.1,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        shaker,
    ));
    tracks.push(b.track(
        Instrument::Piano,
        "psy-rhythm.cymbal",
        "Cymbal",
        0.4,
        0.0,
        Effects {
            cutoff_hz: MAX_CUTOFF_HZ,
            reverb: 0.3,
            delay: 0.0,
            drive: 0.0,
        },
        DRIVE_STEPS,
        crash,
    ));

    // -- Bass -------------------------------------------------------------
    // dub: long sub notes with space between them; dub-space stabs chord
    // colour on the offbeats in the main sections.
    let mut dub = Vec::new();
    let mut dub_chords = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = dub_chord(bar);
        match dub_section(bar) {
            Section::Intro => {
                dub.push(nt(chord.bass, s, 14.0, 0.7));
            }
            Section::Break => {
                dub.push(nt(chord.bass, s, 7.0, 0.65));
                dub.push(nt(chord.bass + 7, s + 8.0, 7.0, 0.55));
            }
            _ => {
                // Root, syncopated fifth answer, octave lift.
                dub.push(nt(chord.bass, s, 6.0, 0.8));
                dub.push(nt(chord.bass + 7, s + 7.0, 3.0, 0.55));
                dub.push(nt(chord.bass, s + 11.0, 4.0, 0.6));
                dub.push(nt(chord.bass + 12, s + 14.0, 1.5, 0.5));
            }
        }
        if matches!(dub_section(bar), Section::Main | Section::Finale) {
            // Poly dub stabs on the "and" of 2 and 4.
            for &st in &[6.0, 14.0] {
                for (i, &p) in chord.organ.iter().enumerate() {
                    dub_chords.push(nt(p, s + st, 1.6, 0.36 + i as f32 * 0.03));
                }
            }
        }
    }
    tracks.push(b.track(
        Instrument::Bass,
        "psy-bass.dub",
        "Dub bass",
        0.8,
        -0.05,
        Effects {
            cutoff_hz: 1400.0,
            reverb: 0.08,
            delay: 0.15,
            drive: 0.1,
        },
        DRIVE_STEPS,
        dub,
    ));
    tracks.push(b.track(
        Instrument::Bass,
        "psy-bass.dub-space",
        "Bass chords",
        0.45,
        0.15,
        Effects {
            cutoff_hz: 2000.0,
            reverb: 0.2,
            delay: 0.3,
            drive: 0.0,
        },
        DRIVE_STEPS,
        dub_chords,
    ));

    // -- Organ ------------------------------------------------------------
    // dub-organ skank on the offbeats; sustained in the breakdown.
    let mut organ = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = dub_chord(bar);
        match dub_section(bar) {
            Section::Intro | Section::Break => {
                for &p in &chord.organ {
                    organ.push(nt(p, s, 15.0, 0.36));
                }
            }
            _ => {
                for &st in &[6.0, 14.0] {
                    for (i, &p) in chord.organ.iter().enumerate() {
                        organ.push(nt(p, s + st, 1.8, 0.4 + i as f32 * 0.03));
                    }
                }
            }
        }
    }
    tracks.push(b.track(
        Instrument::Piano,
        "psy-keys.dub-organ",
        "Dub organ",
        0.5,
        0.1,
        Effects {
            cutoff_hz: 2400.0,
            reverb: 0.4,
            delay: 0.5,
            drive: 0.0,
        },
        DRIVE_STEPS,
        organ,
    ));

    // -- Keys -------------------------------------------------------------
    // soul-keys: sparse chord stabs answering the organ.
    let mut keys = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = dub_chord(bar);
        match dub_section(bar) {
            Section::Main | Section::Finale => {
                for &p in &chord.organ {
                    keys.push(nt(p + 12, s + 10.0, 2.5, 0.3));
                }
            }
            Section::Break => {
                for &p in &chord.organ {
                    keys.push(nt(p, s + 4.0, 10.0, 0.28));
                }
            }
            Section::Intro => {}
        }
    }
    tracks.push(b.track(
        Instrument::Piano,
        "psy-keys.soul-keys",
        "Soul keys",
        0.45,
        -0.15,
        Effects {
            cutoff_hz: 4200.0,
            reverb: 0.35,
            delay: 0.4,
            drive: 0.0,
        },
        DRIVE_STEPS,
        keys,
    ));

    // -- Pad --------------------------------------------------------------
    let mut pad = Vec::new();
    for bar in 0..16usize {
        let s = (bar * 16) as f64;
        let chord = dub_chord(bar);
        for &p in &chord.pad {
            pad.push(nt(p, s, 31.0, 0.36));
        }
    }
    tracks.push(b.track(
        Instrument::Pad,
        "psy-space.dark-nebula",
        "Dark nebula",
        0.48,
        0.0,
        Effects {
            cutoff_hz: 2400.0,
            reverb: 0.5,
            delay: 0.2,
            drive: 0.0,
        },
        DRIVE_STEPS,
        pad,
    ));

    // -- Lead -------------------------------------------------------------
    // dark-lead: sparse dubwise phrases in the main blocks and finale.
    let mut lead = Vec::new();
    let phrases: &[LeadPhrase] = &[
        (
            2,
            &[
                (0.0, 62, 4.0, 0.55),
                (6.0, 65, 3.0, 0.48),
                (10.0, 67, 5.0, 0.52),
            ],
        ),
        (
            8,
            &[
                (0.0, 69, 4.0, 0.56),
                (6.0, 67, 3.0, 0.5),
                (10.0, 65, 5.0, 0.52),
            ],
        ),
        (14, &[(0.0, 62, 6.0, 0.55), (8.0, 60, 7.0, 0.5)]),
    ];
    for &(bar_start, notes) in phrases {
        for &(st, pitch, dur, vel) in notes.iter() {
            lead.push(nt(pitch, bar_start as f64 * 16.0 + st, dur, vel));
        }
    }
    tracks.push(b.track(
        Instrument::Lead,
        "psy-leads.dark-lead",
        "Dark lead",
        0.55,
        0.1,
        Effects {
            cutoff_hz: 3200.0,
            reverb: 0.4,
            delay: 0.55,
            drive: 0.0,
        },
        DRIVE_STEPS,
        lead,
    ));

    song_project("Midnight Dub", 96.0, 0.15, 0.74, tracks)
}

fn all_presets_map() -> Result<HashMap<String, InstrumentPreset>, String> {
    let mut map = HashMap::new();
    for pack in load_packs()? {
        for preset in pack.instruments {
            map.insert(preset.id.clone(), preset);
        }
    }
    Ok(map)
}

fn showcase_songs() -> Result<Vec<(&'static str, Project)>, String> {
    let presets = all_presets_map()?;
    Ok(vec![
        ("neon-circuits", build_drive_song(&presets)),
        ("midnight-dub", build_dub_song(&presets)),
    ])
}

fn song_file_name(project: &Project) -> String {
    let slug: String = project
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    format!("{slug}.cadence")
}

/// Writes the two showcase songs to `examples/songs/` — the checked-in
/// canonical copies. Only called from `--write-songs`; a plain `OUTDIR` run
/// writes its own copies under OUTDIR instead.
fn write_showcase_songs(songs: &[(&'static str, Project)]) -> Result<(), String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(SONG_DIR);
    fs::create_dir_all(&dir).map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    for (_, project) in songs {
        let path = dir.join(song_file_name(project));
        persistence::save_project(project, &path)?;
        println!("  song {:<40} -> {}", project.name, path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

const USAGE: &str = "\
bank_audit - psy soundbank QA, listening reels and showcase songs

USAGE:
    cargo run --release --locked --offline --example bank_audit -- OUTDIR
    cargo run --release --example bank_audit -- --write-songs
    cargo run --release --example bank_audit -- --help

With OUTDIR, writes audit.csv/audit.json, reels/*.wav + manifests,
songs/*.cadence and previews/*.wav into OUTDIR — and nothing else; the
checked-in examples/songs/ files are left untouched. --write-songs is the
only mode that (re)generates examples/songs/*.cadence. No user data is
touched in any mode.
";

fn run(out_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(out_dir)
        .map_err(|err| format!("cannot create {}: {err}", out_dir.display()))?;

    println!("Loading psy packs...");
    let packs = load_packs()?;
    let total: usize = packs.iter().map(|p| p.instruments.len()).sum();
    println!("  {} presets in {} packs", total, packs.len());

    println!("Rendering per-preset audit...");
    let mut entries = Vec::new();
    let mut quietest = (f32::MAX, String::new());
    let mut loudest = (0.0f32, String::new());
    for (pack_index, pack) in packs.iter().enumerate() {
        let pack_id = PSY_PACKS[pack_index].0;
        for preset in &pack.instruments {
            let (entry, metrics) = audit_preset(pack_id, preset)?;
            if metrics.peak < quietest.0 {
                quietest = (metrics.peak, preset.id.clone());
            }
            if metrics.peak > loudest.0 {
                loudest = (metrics.peak, preset.id.clone());
            }
            entries.push(entry);
        }
        println!(
            "  {:<12} {} presets audited",
            pack_id,
            pack.instruments.len()
        );
    }
    write_audit(out_dir, &entries)?;
    println!("  audit -> {}/audit.csv, audit.json", out_dir.display());
    println!(
        "  peak range {:.4} ({}) .. {:.3} ({})",
        quietest.0, quietest.1, loudest.0, loudest.1
    );

    println!("Building listening reels...");
    write_reels(out_dir, &packs)?;

    println!("Building showcase songs...");
    let songs = showcase_songs()?;

    // Everything lands under OUTDIR: audits, reels, song files, previews.
    // The checked-in examples/songs/ copies are only refreshed by
    // --write-songs, never by a plain audit run.
    let songs_dir = out_dir.join("songs");
    let previews_dir = out_dir.join("previews");
    fs::create_dir_all(&songs_dir)
        .map_err(|err| format!("cannot create {}: {err}", songs_dir.display()))?;
    fs::create_dir_all(&previews_dir)
        .map_err(|err| format!("cannot create {}: {err}", previews_dir.display()))?;
    for (_, project) in &songs {
        let file_name = song_file_name(project);
        let song_path = songs_dir.join(&file_name);
        persistence::save_project(project, &song_path)?;
        println!("  song {:<24} -> {}", project.name, song_path.display());
        let preview = previews_dir.join(file_name.replace(".cadence", ".wav"));
        export_wav(project, &preview, |_| {})?;
        println!("  preview {:<24} -> {}", project.name, preview.display());
    }

    println!("Done. Everything was written under {}", out_dir.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--help") | Some("-h") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("--write-songs") => {
            if args.len() != 1 {
                Err("--write-songs takes no other arguments.\n".to_string() + USAGE)
            } else {
                showcase_songs().and_then(|songs| write_showcase_songs(&songs))
            }
        }
        Some(flag) if flag.starts_with('-') => Err(format!("Unknown option {flag}.\n{USAGE}")),
        Some(dir) if args.len() == 1 => run(Path::new(dir)),
        None => Err(USAGE.to_string()),
        _ => Err(format!("Unexpected arguments: {}\n{USAGE}", args.join(" "))),
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
