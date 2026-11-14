//! QA coverage for the seven `psy-*` instrument packs: 128 synthesized presets
//! across arps, bass, brass, keys, leads, rhythm and space.
//!
//! Three layers are checked here:
//!
//! 1. **Structure** — every pack parses through the real [`packs::parse_pack`]
//!    path, all 128 preset ids are unique, and no two presets share an exact
//!    synthesis definition (ids, names, descriptions and audition metadata are
//!    ignored when comparing).
//! 2. **Deployment** — [`packs::load_catalog_from`] on an empty directory (so no
//!    user pack folder is touched) must serve all 128 presets: the packs are
//!    embedded in the binary, not just present as files.
//! 3. **Signal** — every preset is rendered through [`audio::render_project`],
//!    the same `Renderer`/voice DSP used for playback and export, on a short
//!    musical audition phrase (held notes, chords, lines and percussion hits at
//!    a musically sensible register). Every render must be finite, bounded,
//!    non-silent, DC-free and must decay consistent with the preset's own
//!    envelope. These are sanity checks on the rendered signal, not a judgement
//!    of subjective sound quality.
//!
//! User data is never touched: the catalog is loaded from an explicitly empty
//! temporary directory and no environment variables are changed.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use cadence_studio::audio::{render_project, RenderedAudio};
use cadence_studio::model::{
    create_pattern, Clip, Effects, Instrument, Note, Pattern, Project, Track, MAX_CUTOFF_HZ,
    STEPS_PER_BAR,
};
use cadence_studio::packs::{self, InstrumentPack, InstrumentPreset};
use cadence_studio::persistence;
use serde_json::Value;

/// The psy bank source files, embedded at compile time so the test always
/// audits exactly what ships in the repository.
const PSY_PACKS: [(&str, &str); 7] = [
    ("psy-arps", include_str!("../packs/psy-arps/pack.json")),
    ("psy-bass", include_str!("../packs/psy-bass/pack.json")),
    ("psy-brass", include_str!("../packs/psy-brass/pack.json")),
    ("psy-keys", include_str!("../packs/psy-keys/pack.json")),
    ("psy-leads", include_str!("../packs/psy-leads/pack.json")),
    ("psy-rhythm", include_str!("../packs/psy-rhythm/pack.json")),
    ("psy-space", include_str!("../packs/psy-space/pack.json")),
];

const EXPECTED_PRESETS: usize = 128;
/// Offline render rate for the audition phrases: the same rate the existing
/// render tests use, high enough to keep the filters honest.
const AUDITION_SAMPLE_RATE: u32 = 22_050;
/// Audition tempo: a step is exactly 1/8 s, keeping step math readable.
const AUDITION_BPM: f64 = 120.0;
const STEPS_PER_SECOND: f64 = AUDITION_BPM * 4.0 / 60.0;
/// Dry mixer settings isolate the preset voice from track/master effects.
const DRY: Effects = Effects {
    cutoff_hz: MAX_CUTOFF_HZ,
    reverb: 0.0,
    delay: 0.0,
    drive: 0.0,
};

fn pack_json(pack_id: &str) -> &'static str {
    PSY_PACKS
        .iter()
        .find(|(id, _)| *id == pack_id)
        .map(|(_, json)| *json)
        .unwrap_or_else(|| panic!("unknown psy pack {pack_id}"))
}

fn parse_pack(pack_id: &str) -> InstrumentPack {
    packs::parse_pack(pack_json(pack_id))
        .unwrap_or_else(|err| panic!("pack {pack_id} must parse: {err}"))
}

/// Every psy preset as `(pack_id, preset)` in pack order.
fn all_presets() -> Vec<(&'static str, InstrumentPreset)> {
    PSY_PACKS
        .iter()
        .flat_map(|(pack_id, json)| {
            packs::parse_pack(json)
                .unwrap_or_else(|err| panic!("pack {pack_id} must parse: {err}"))
                .instruments
                .into_iter()
                .map(move |preset| (*pack_id, preset))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Audition metadata (`audition_note` / `audition_duration`)
// ---------------------------------------------------------------------------
//
// These optional JSON fields steer library preview centering only. They are read
// straight from the pack documents so this test works whether or not the
// `InstrumentPreset` struct exposes them yet; absent fields simply fall back to
// per-pack defaults.

/// `(pitch, seconds)` audition hint for `preset_id`, or `None` when the pack
/// does not carry the metadata.
fn audition_meta(pack_id: &str, preset_id: &str) -> Option<(u8, f64)> {
    let value: Value = serde_json::from_str(pack_json(pack_id)).ok()?;
    let instruments = value.get("instruments")?.as_array()?;
    let preset = instruments
        .iter()
        .find(|p| p.get("id").and_then(Value::as_str) == Some(preset_id))?;
    let note = preset.get("audition_note")?.as_u64()?;
    let duration = preset
        .get("audition_duration")
        .and_then(Value::as_f64)
        .unwrap_or(0.8);
    Some((note.min(127) as u8, duration.clamp(0.1, 8.0)))
}

// ---------------------------------------------------------------------------
// Audition phrase construction
// ---------------------------------------------------------------------------

/// How a preset is exercised: percussion hits, a sustained pad chord, a short
/// melodic line for mono voices, or chord stabs for poly voices.
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

/// Editor instrument only decides which editor opens the track; the preset
/// itself supplies the sound. Pick the closest built-in for a sensible default.
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

/// A sensible audition register when the pack carries no `audition_note`:
/// drum voices map onto their kit-like pitches, everything else uses a
/// mid-range default per family.
fn fallback_pitch(pack_id: &str, preset: &InstrumentPreset) -> u8 {
    if pack_id == "psy-rhythm" {
        let id = preset.id.as_str();
        return if id.contains("kick-deep") {
            24 // C1: maximum sub weight
        } else if id.contains("kick") {
            36 // C2
        } else if id.contains("snare") {
            40 // E2, where the body sits
        } else if id.contains("rim") {
            38 // D2
        } else if id.contains("tom-low") || id.contains("floor-tom") {
            33 // A1
        } else if id.contains("tom-high") || id.contains("conga") {
            50 // D3
        } else if id.contains("bongo") {
            57 // A3
        } else if id.contains("tick") || id.contains("hat") {
            72 // hats/ticks are unpitched; high keeps them crisp
        } else {
            60
        };
    }
    match pack_id {
        "psy-bass" => 33,               // A1, mid of the recommended C1-C2 band
        "psy-space" => 46,              // Bb2, low pad register
        "psy-leads" | "psy-arps" => 72, // C5, lead/pluck register
        _ => 60,                        // keys/brass middle C
    }
}

fn audition_pitch(pack_id: &str, preset: &InstrumentPreset) -> u8 {
    audition_meta(pack_id, &preset.id)
        .map(|(pitch, _)| pitch)
        .unwrap_or_else(|| fallback_pitch(pack_id, preset))
}

fn n(pitch: u8, start: f64, duration: f64, velocity: f32) -> Note {
    Note::new(pitch, start, duration, velocity)
}

/// Builds the audition pattern for one preset. Returns the pattern and the
/// number of steps of musical content (before the rendered tail).
fn audition_pattern(
    pack_id: &str,
    preset: &InstrumentPreset,
    pitch: u8,
    meta_seconds: f64,
) -> Pattern {
    let env = &preset.envelope;
    let role = role_of(pack_id, preset);
    let steps = |secs: f64| secs * STEPS_PER_SECOND;
    let mut notes: Vec<Note> = Vec::new();

    let length = match role {
        Role::Percussion => {
            // One full-length hit plus two shorter ghosts: exercises the decay
            // while the note is held and the release after it.
            let gate = (f64::from(env.decay) * 1.2).clamp(0.15, 1.6);
            notes.push(n(pitch, 0.0, steps(gate), 0.95));
            let second = steps(gate) + 2.0;
            notes.push(n(pitch, second, steps(0.15), 0.6));
            notes.push(n(pitch + 12, second + 4.0, steps(0.15), 0.45));
            second + 6.0
        }
        Role::Pad => {
            // Hold long enough for even the slowest attack to open, then let
            // the (often very long) release ring into the tail.
            let hold = (f64::from(env.attack) * 1.2 + 0.5).clamp(1.2, 4.0);
            let tones: &[u8] = if preset.monophonic {
                &[0]
            } else {
                &[0, 4, 7, 12]
            };
            for (i, &semi) in tones.iter().enumerate() {
                notes.push(n(
                    pitch.saturating_add(semi),
                    0.0,
                    steps(hold),
                    0.5 + i as f32 * 0.05,
                ));
            }
            steps(hold) + 2.0
        }
        Role::Mono => {
            // A four-note motif an octave-ish wide: pitch tracking, mono voice
            // behaviour and the preset's own gate length.
            let gate = meta_seconds.clamp(0.25, 0.9);
            let motif: [u8; 4] = [0, 7, 12, 9];
            for (i, &semi) in motif.iter().enumerate() {
                let start = i as f64 * 4.0;
                notes.push(n(
                    pitch.saturating_add(semi),
                    start,
                    steps(gate),
                    0.55 + i as f32 * 0.1,
                ));
            }
            12.0 + steps(gate) + 2.0
        }
        Role::Poly => {
            // A short stab then a sustained chord: covers both articulations.
            let stab = steps(meta_seconds.clamp(0.15, 0.5));
            let hold = (f64::from(env.attack) * 1.2 + 0.4).clamp(0.8, 2.2);
            let tones: [u8; 4] = [0, 4, 7, 12];
            for (i, &semi) in tones.iter().enumerate() {
                let vel = 0.5 + i as f32 * 0.04;
                notes.push(n(pitch.saturating_add(semi), 0.0, stab, vel));
                notes.push(n(pitch.saturating_add(semi), 8.0, steps(hold), vel));
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

/// Seconds of rendered tail to ask for after the pattern ends, scaled to the
/// preset's own release so long pads get room to demonstrate their decay
/// without making every preset pay for a full-length tail.
fn tail_seconds(preset: &InstrumentPreset) -> f64 {
    let env = &preset.envelope;
    (f64::from(env.release) * 1.2 + f64::from(env.decay) * 0.6 + 0.8).clamp(1.4, 7.0)
}

/// A one-track project playing `preset` on the audition phrase.
fn audition_project(pack_id: &str, preset: &InstrumentPreset) -> Project {
    let (meta_pitch, meta_secs) =
        audition_meta(pack_id, &preset.id).unwrap_or((fallback_pitch(pack_id, preset), 0.8));
    let pitch = meta_pitch;
    let pattern = audition_pattern(pack_id, preset, pitch, meta_secs);
    let mut track = Track::from_preset(editor_for(pack_id), preset.clone());
    track.name = preset.name.chars().take(60).collect();
    track.volume = 0.75;
    track.pan = 0.0;
    track.effects = DRY;
    let pattern_id = pattern.id;
    track.patterns = vec![pattern];
    track.clips = vec![Clip {
        id: cadence_studio::model::next_id(),
        pattern_id,
        start: 0.0,
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    }];

    let mut project = Project::empty();
    project.name = format!("QA {}", preset.id);
    project.bpm = AUDITION_BPM;
    project.swing = 0.0;
    project.bars = track.patterns[0].length.div_ceil(STEPS_PER_BAR).max(1);
    project.master_volume = 0.85;
    project.tracks = vec![track];
    project
        .validate()
        .unwrap_or_else(|err| panic!("audition project for {} is invalid: {err}", preset.id));
    project
}

// ---------------------------------------------------------------------------
// Rendered-audio analysis
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct AudioStats {
    /// Loudest absolute sample across both channels.
    peak: f32,
    /// Mean sample value (DC estimate).
    dc: f64,
    /// RMS of the loudest 0.2 s window: the "hot" level of the phrase.
    hot_rms: f64,
    /// RMS near the end of the render, excluding the final 80 ms safety fade.
    tail_rms: f64,
    /// Seconds from the last note-off to the end of the audio.
    tail_span: f64,
    /// Fraction of samples pinned at |x| >= 0.98 (pathological clipping).
    clipped_fraction: f64,
}

fn window_rms(samples: &[f32], start: usize, end: usize) -> f64 {
    let end = end.min(samples.len());
    if end <= start {
        return 0.0;
    }
    let sum: f64 = samples[start..end]
        .iter()
        .map(|s| (*s as f64) * (*s as f64))
        .sum();
    (sum / (end - start) as f64).sqrt()
}

fn analyze(audio: &RenderedAudio, last_note_off_secs: f64) -> AudioStats {
    let sr = audio.sample_rate as usize;
    let frames = audio.left.len();
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    let mut clipped = 0usize;
    for i in 0..frames {
        let m = audio.left[i].abs().max(audio.right[i].abs());
        peak = peak.max(m);
        sum += 0.5 * (audio.left[i] as f64 + audio.right[i] as f64);
        if m >= 0.98 {
            clipped += 1;
        }
    }
    let seconds = frames as f64 / sr as f64;

    // Loudest 0.2 s window, scanned at 10 ms hops.
    let win = (0.2 * sr as f64) as usize;
    let hop = (0.01 * sr as f64) as usize;
    let mut hot_rms = 0.0f64;
    let mut start = 0usize;
    while start + win <= frames {
        hot_rms = hot_rms.max(window_rms(&audio.left, start, start + win));
        start += hop;
    }

    // Tail window: last 0.4 s, stopping 80 ms early to dodge the render's
    // final safety fade.
    let tail_end = frames.saturating_sub((0.08 * sr as f64) as usize);
    let tail_start = tail_end.saturating_sub((0.4 * sr as f64) as usize);
    let tail_rms = 0.5
        * (window_rms(&audio.left, tail_start, tail_end)
            + window_rms(&audio.right, tail_start, tail_end));

    AudioStats {
        peak,
        dc: sum / frames.max(1) as f64,
        hot_rms,
        tail_rms,
        tail_span: (seconds - last_note_off_secs).max(0.0),
        clipped_fraction: clipped as f64 / frames.max(1) as f64,
    }
}

/// The latest note-off time of the audition pattern, in seconds.
fn last_note_off(pattern: &Pattern) -> f64 {
    pattern
        .notes
        .iter()
        .map(|note| note.end())
        .fold(0.0, f64::max)
        / STEPS_PER_SECOND
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn psy_packs_parse_with_128_unique_valid_presets() {
    let presets = all_presets();
    assert_eq!(
        presets.len(),
        EXPECTED_PRESETS,
        "the psy bank must contain exactly {EXPECTED_PRESETS} presets"
    );

    let mut ids = HashSet::new();
    for (pack_id, preset) in &presets {
        assert!(
            ids.insert(preset.id.as_str()),
            "duplicate preset id {}",
            preset.id
        );
        assert!(
            preset.id.starts_with(&format!("{pack_id}.")),
            "preset {} is not namespaced under {pack_id}",
            preset.id
        );
        packs::validate_preset(preset)
            .unwrap_or_else(|err| panic!("preset {} failed validation: {err}", preset.id));
    }

    // No two presets may share an exact synthesis definition. Identity and
    // descriptive/preview metadata (id, name, description, audition_note,
    // audition_duration) are stripped before comparing.
    let mut signatures = HashSet::new();
    for (pack_id, preset) in &presets {
        let mut value = serde_json::to_value(preset)
            .unwrap_or_else(|err| panic!("preset {} does not serialize: {err}", preset.id));
        if let Value::Object(map) = &mut value {
            for key in [
                "id",
                "name",
                "description",
                "audition_note",
                "audition_duration",
            ] {
                map.remove(key);
            }
        }
        let signature = serde_json::to_string(&value).expect("signature serializes");
        assert!(
            signatures.insert(signature),
            "presets in the psy bank must not share an exact synthesis definition \
             (first duplicate found in pack {pack_id})"
        );
    }
    assert_eq!(signatures.len(), EXPECTED_PRESETS);
}

#[test]
fn catalog_serves_all_psy_presets_from_an_empty_data_dir() {
    // An empty temp dir means only embedded packs are served: this proves the
    // bank ships inside the binary and never depends on installed files.
    let dir = tempfile::tempdir().expect("temp dir");
    let catalog = packs::load_catalog_from(dir.path());
    assert!(
        catalog.errors.is_empty(),
        "catalog reported problems: {:?}",
        catalog.errors
    );

    let mut by_pack: BTreeMap<&str, usize> = BTreeMap::new();
    for preset in &catalog.instruments {
        if let Some(pack_id) = preset.id.split('.').next() {
            if pack_id.starts_with("psy-") {
                *by_pack.entry(pack_id).or_default() += 1;
            }
        }
    }
    for (pack_id, _) in PSY_PACKS {
        let expected = parse_pack(pack_id).instruments.len();
        assert_eq!(
            by_pack.get(pack_id).copied().unwrap_or(0),
            expected,
            "catalog must serve all {expected} presets of {pack_id}"
        );
    }

    let served: HashSet<&str> = catalog
        .instruments
        .iter()
        .map(|preset| preset.id.as_str())
        .collect();
    for (_, preset) in all_presets() {
        assert!(
            served.contains(preset.id.as_str()),
            "catalog is missing preset {}",
            preset.id
        );
    }
}

#[test]
fn audition_metadata_is_in_range_when_present() {
    for (pack_id, _) in PSY_PACKS {
        let value: Value = serde_json::from_str(pack_json(pack_id)).expect("pack JSON parses");
        let instruments = value["instruments"].as_array().expect("instruments");
        for preset in instruments {
            let id = preset["id"].as_str().unwrap_or("?");
            if let Some(note) = preset.get("audition_note") {
                let note = note
                    .as_u64()
                    .unwrap_or_else(|| panic!("{id}: audition_note must be an integer"));
                assert!(note <= 127, "{id}: audition_note {note} above 127");
            }
            if let Some(duration) = preset.get("audition_duration") {
                let duration = duration
                    .as_f64()
                    .unwrap_or_else(|| panic!("{id}: audition_duration must be a number"));
                assert!(
                    (0.1..=8.0).contains(&duration),
                    "{id}: audition_duration {duration} outside 0.1..=8"
                );
            }
        }
    }
}

#[test]
fn every_preset_renders_finite_bounded_decaying_audio() {
    let mut failures: Vec<String> = Vec::new();
    let mut quietest = (f32::MAX, String::new());
    let mut loudest = (0.0f32, String::new());

    for (pack_id, preset) in all_presets() {
        let pattern = audition_pattern(
            pack_id,
            &preset,
            audition_pitch(pack_id, &preset),
            audition_meta(pack_id, &preset.id)
                .map(|(_, secs)| secs)
                .unwrap_or(0.8),
        );
        let project = audition_project(pack_id, &preset);
        let tail = tail_seconds(&preset);
        let max_seconds = project.duration_seconds() + tail;
        let audio = match render_project(&project, AUDITION_SAMPLE_RATE, max_seconds) {
            Ok(audio) => audio,
            Err(err) => {
                failures.push(format!("{}: render failed: {err}", preset.id));
                continue;
            }
        };
        let stats = analyze(&audio, last_note_off(&pattern));
        let mut problems: Vec<String> = Vec::new();

        // Finite and bounded: the limiter ceiling is -1 dBFS, so nothing may
        // exceed full scale and essentially nothing may pin near it.
        let all_finite = audio
            .left
            .iter()
            .chain(audio.right.iter())
            .all(|s| s.is_finite());
        if !all_finite {
            problems.push("non-finite samples".into());
        }
        if stats.peak > 1.0 {
            problems.push(format!("peak {:.3} exceeds full scale", stats.peak));
        }
        if stats.clipped_fraction > 0.0 {
            problems.push(format!(
                "{:.2}% of samples pinned near full scale",
                stats.clipped_fraction * 100.0
            ));
        }

        // Non-silent: thresholds are deliberately low so intentional quiet
        // pads, ticks and dark drones still pass; they only catch presets that
        // produce no usable signal at all.
        if stats.peak < 0.004 {
            problems.push(format!("peak {:.5} is effectively silent", stats.peak));
        }
        if stats.hot_rms < 0.0006 {
            problems.push(format!(
                "hot rms {:.6} is effectively silent",
                stats.hot_rms
            ));
        }

        // DC: the master DC blocker should leave the mean near zero; a stuck
        // or constant-offset voice shows up here.
        if stats.dc.abs() > 0.03 {
            problems.push(format!("dc offset {:.4} is too large", stats.dc));
        }

        // Decay: the tail must fall consistent with the preset's own envelope.
        // `tau` is the slowest exponential stage (decay tau = decay/4,
        // release tau = release/5 in envelope time constants); allow generous
        // headroom so LFO/filter motion and short rendered tails cannot fail a
        // healthy preset, while a voice that ignores note-off still fails.
        let env = &preset.envelope;
        let tau = (f64::from(env.decay) / 4.0)
            .max(f64::from(env.release) / 5.0)
            .max(0.01);
        let expected = (-stats.tail_span / tau).exp();
        let allowed = (expected * 3.0).max(0.05);
        if stats.tail_rms > stats.hot_rms * allowed + 0.0004 {
            problems.push(format!(
                "tail rms {:.5} does not decay with the envelope \
                 (hot {:.5}, allowed ratio {:.3}, tail {:.2}s, release {:.2}s)",
                stats.tail_rms, stats.hot_rms, allowed, stats.tail_span, env.release
            ));
        }

        if stats.peak < quietest.0 {
            quietest = (stats.peak, preset.id.clone());
        }
        if stats.peak > loudest.0 {
            loudest = (stats.peak, preset.id.clone());
        }
        if !problems.is_empty() {
            failures.push(format!("{}: {}", preset.id, problems.join("; ")));
        }
    }

    eprintln!(
        "psy soundbank audit: {} presets rendered; quietest peak {:.4} ({}), \
         loudest peak {:.3} ({})",
        EXPECTED_PRESETS, quietest.0, quietest.1, loudest.0, loudest.1
    );
    assert!(
        failures.is_empty(),
        "{} preset(s) failed the render checks:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn pack_order_and_categories_are_stable() {
    // The seven packs and their sizes are the bank's public shape; docs and
    // the audit tooling rely on these names.
    let expected: [(&str, usize); 7] = [
        ("psy-arps", 16),
        ("psy-bass", 24),
        ("psy-brass", 16),
        ("psy-keys", 16),
        ("psy-leads", 16),
        ("psy-rhythm", 24),
        ("psy-space", 16),
    ];
    for (pack_id, count) in expected {
        let pack = parse_pack(pack_id);
        assert_eq!(pack.instruments.len(), count, "{pack_id} preset count");
        assert!(pack.id == pack_id);
        // Preset order inside a pack is the listening order; ids must stay
        // unique and names non-empty (parse_pack already enforces both).
        let mut seen = BTreeSet::new();
        for preset in &pack.instruments {
            assert!(seen.insert(preset.id.as_str()));
            assert!(!preset.name.trim().is_empty());
        }
    }
}

/// The checked-in showcase songs must keep percussion in the piano roll:
/// `psy-rhythm` presets are standalone pitched voices (played at MIDI notes
/// like C1/C2/E2), so their tracks must not use the built-in eight-row drum
/// editor, and every showcase track must embed its preset so the songs stay
/// portable. The files are read at runtime — not `include_str!` — so this
/// test checks whatever `bank_audit --write-songs` last generated.
#[test]
fn showcase_songs_keep_percussion_in_the_piano_roll() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/songs");
    for file_name in ["neon-circuits.cadence", "midnight-dub.cadence"] {
        let path = dir.join(file_name);
        let project = persistence::load_project(&path)
            .unwrap_or_else(|err| panic!("{file_name} must load: {err}"));
        project
            .validate()
            .unwrap_or_else(|err| panic!("{file_name} is invalid: {err}"));

        let mut percussion = 0usize;
        for track in &project.tracks {
            let preset = track.preset.as_ref().unwrap_or_else(|| {
                panic!("{file_name}: every showcase track must embed its preset")
            });
            if !preset.id.starts_with("psy-rhythm.") {
                continue;
            }
            percussion += 1;
            assert_ne!(
                track.instrument,
                Instrument::Drums,
                "{file_name}: {} must use a non-drum editor (piano roll), \
                 not the built-in drum kit",
                preset.id
            );
        }
        assert!(
            percussion > 0,
            "{file_name} must contain psy-rhythm preset tracks"
        );
    }
}
