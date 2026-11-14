//! Pure, testable project editing operations used by the GUI.
//!
//! Everything here mutates a [`Project`] (or part of it) and never touches
//! egui, audio or the filesystem, so undo/redo can wrap each call.

use std::collections::BTreeSet;

use crate::model::{
    create_pattern, create_track, next_id, Clip, Instrument, Note, Pattern, Project, Track,
};
use crate::packs::InstrumentPreset;

pub use crate::model::{
    MAX_BARS, MAX_BPM, MAX_CLIPS_PER_TRACK, MAX_CLIP_LOOPS, MAX_NOTES_PER_PATTERN,
    MAX_PATTERNS_PER_TRACK, MAX_PATTERN_STEPS, MAX_SONG_EVENTS, MAX_SWING, MAX_TRACKS, MIN_BARS,
    MIN_BPM, MIN_NOTE_DURATION, STEPS_PER_BAR,
};

pub const MAX_PITCH: u8 = 127;
/// Pattern lengths offered in the editor, in steps.
pub const PATTERN_LENGTHS: [u32; 8] = [8, 16, 32, 48, 64, 128, 192, 256];
/// Longest song in steps; clips must start before and fit inside this.
pub const MAX_SONG_STEPS: f64 = (MAX_BARS * STEPS_PER_BAR) as f64;

/// Display label for a track's sound: the pack preset name or the built-in name.
pub fn instrument_label(track: &Track) -> &str {
    track
        .preset
        .as_ref()
        .map_or(track.instrument.name(), |p| p.name.as_str())
}

/// Clamps a clip start so the whole clip stays inside the maximum song length.
pub fn clamp_clip_start(start: f64, len: f64) -> f64 {
    start.clamp(0.0, (MAX_SONG_STEPS - len).max(0.0))
}

pub fn track_index(project: &Project, track_id: u64) -> Option<usize> {
    project.tracks.iter().position(|t| t.id == track_id)
}

pub fn track(project: &Project, track_id: u64) -> Option<&Track> {
    project.tracks.iter().find(|t| t.id == track_id)
}

pub fn track_mut(project: &mut Project, track_id: u64) -> Option<&mut Track> {
    project.tracks.iter_mut().find(|t| t.id == track_id)
}

pub fn pattern_mut(project: &mut Project, track_id: u64, pattern_id: u64) -> Option<&mut Pattern> {
    track_mut(project, track_id)?
        .patterns
        .iter_mut()
        .find(|p| p.id == pattern_id)
}

pub fn pattern(track: &Track, pattern_id: u64) -> Option<&Pattern> {
    track.patterns.iter().find(|p| p.id == pattern_id)
}

/// Total length of a clip in steps: its pattern length times its loop count.
/// A missing pattern falls back to one bar, as before.
pub fn clip_len(track: &Track, clip: &Clip) -> f64 {
    let pattern_len =
        pattern(track, clip.pattern_id).map_or(STEPS_PER_BAR as f64, |p| p.length.max(1) as f64);
    clip.span(pattern_len)
}

/// Largest loop count that keeps this clip's flattened note events within
/// [`MAX_SONG_EVENTS`]. A missing or empty pattern counts as a single note.
pub fn max_clip_loops(track: &Track, clip: &Clip) -> u32 {
    let notes = pattern(track, clip.pattern_id)
        .map_or(0, |p| p.notes.len())
        .max(1);
    MAX_CLIP_LOOPS.min((MAX_SONG_EVENTS / notes) as u32).max(1)
}

/// End of the last clip in the song, in steps.
pub fn content_end(project: &Project) -> f64 {
    project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(move |c| c.start + clip_len(t, c)))
        .fold(0.0, f64::max)
}

/// Grows the song length so every clip fits (never shrinks).
pub fn fit_bars_to_content(project: &mut Project) {
    let needed = (content_end(project) / STEPS_PER_BAR as f64).ceil() as u32;
    if needed > project.bars {
        project.bars = needed.clamp(MIN_BARS, MAX_BARS);
    }
}

pub fn unique_name(base: &str, existing: impl Iterator<Item = String>) -> String {
    let existing: Vec<String> = existing.collect();
    if !existing.iter().any(|n| n == base) {
        return base.to_owned();
    }
    (2..)
        .map(|i| format!("{base} {i}"))
        .find(|candidate| !existing.iter().any(|n| n == candidate))
        .expect("unbounded search")
}

/// Adds a new built-in instrument track. Returns its id, or `None` at the track limit.
pub fn add_track(project: &mut Project, instrument: Instrument) -> Option<u64> {
    push_track(project, create_track(instrument))
}

/// Adds a track playing a pack preset. The full preset is embedded in the
/// track so the song still sounds the same where the pack isn't installed.
pub fn add_preset_track(project: &mut Project, preset: &InstrumentPreset) -> Option<u64> {
    let mut t = create_track(Instrument::Lead);
    t.name = preset
        .name
        .chars()
        .take(crate::model::MAX_NAME_CHARS)
        .collect();
    t.preset = Some(preset.clone());
    t.color = preset_color(&preset.id);
    push_track(project, t)
}

/// Stable accent color for a pack preset, derived from its id.
pub fn preset_color(id: &str) -> [u8; 3] {
    let hash = id
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    super::theme::TRACK_PALETTE[hash as usize % super::theme::TRACK_PALETTE.len()]
}

fn push_track(project: &mut Project, mut t: Track) -> Option<u64> {
    if project.tracks.len() >= MAX_TRACKS {
        return None;
    }
    t.name = unique_name(&t.name, project.tracks.iter().map(|t| t.name.clone()));
    if project.tracks.iter().any(|o| o.color == t.color) {
        let free = crate::model::TRACK_COLORS
            .iter()
            .chain(super::theme::TRACK_PALETTE.iter())
            .find(|c| !project.tracks.iter().any(|o| &o.color == *c));
        if let Some(c) = free {
            t.color = *c;
        }
    }
    let id = t.id;
    project.tracks.push(t);
    Some(id)
}

/// Deep-copies a track (fresh ids for everything) right below the original.
pub fn duplicate_track(project: &mut Project, track_id: u64) -> Option<u64> {
    if project.tracks.len() >= MAX_TRACKS {
        return None;
    }
    let idx = track_index(project, track_id)?;
    let mut t = project.tracks[idx].clone();
    t.id = next_id();
    t.name = unique_name(
        &format!("{} copy", t.name),
        project.tracks.iter().map(|t| t.name.clone()),
    );
    t.solo = false;
    let mut map = Vec::new();
    for p in &mut t.patterns {
        let new = next_id();
        map.push((p.id, new));
        p.id = new;
        for n in &mut p.notes {
            n.id = next_id();
        }
    }
    for c in &mut t.clips {
        c.id = next_id();
        if let Some((_, new)) = map.iter().find(|(old, _)| *old == c.pattern_id) {
            c.pattern_id = *new;
        }
    }
    let id = t.id;
    project.tracks.insert(idx + 1, t);
    Some(id)
}

pub fn delete_track(project: &mut Project, track_id: u64) -> bool {
    let before = project.tracks.len();
    project.tracks.retain(|t| t.id != track_id);
    project.tracks.len() != before
}

/// Moves a track up (`delta < 0`) or down in the list.
pub fn move_track(project: &mut Project, track_id: u64, delta: i32) -> bool {
    let Some(idx) = track_index(project, track_id) else {
        return false;
    };
    let target = idx as i64 + delta as i64;
    if target < 0 || target >= project.tracks.len() as i64 {
        return false;
    }
    let t = project.tracks.remove(idx);
    project.tracks.insert(target as usize, t);
    true
}

/// Places a clip, clamped inside the maximum song length. `None` if the
/// pattern is missing or the track is at its clip limit.
pub fn add_clip(project: &mut Project, track_id: u64, pattern_id: u64, start: f64) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    let len = pattern(t, pattern_id)?.length.max(1) as f64;
    if t.clips.len() >= MAX_CLIPS_PER_TRACK {
        return None;
    }
    let id = next_id();
    t.clips.push(Clip {
        id,
        pattern_id,
        start: clamp_clip_start(start, len),
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    });
    fit_bars_to_content(project);
    Some(id)
}

/// Moves a clip (clamped so it stays inside the maximum song length).
pub fn move_clip(project: &mut Project, track_id: u64, clip_id: u64, start: f64) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(len) = t
        .clips
        .iter()
        .find(|c| c.id == clip_id)
        .map(|c| clip_len(t, c))
    else {
        return false;
    };
    if let Some(c) = t.clips.iter_mut().find(|c| c.id == clip_id) {
        c.start = clamp_clip_start(start, len);
    }
    fit_bars_to_content(project);
    true
}

pub fn delete_clip(project: &mut Project, track_id: u64, clip_id: u64) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let before = t.clips.len();
    t.clips.retain(|c| c.id != clip_id);
    t.clips.len() != before
}

/// Sets a clip's loop count, clamped to `1..=max_clip_loops`, then grows the
/// song if the longer span now reaches past the end. Fades are re-clamped to
/// the new span so a shorter clip never keeps fades wider than itself.
/// `false` if the track or clip is missing.
pub fn set_clip_loops(project: &mut Project, track_id: u64, clip_id: u64, loops: u32) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    // The cap must be computed before the clip is borrowed mutably.
    let Some(cap) = t
        .clips
        .iter()
        .find(|c| c.id == clip_id)
        .map(|c| max_clip_loops(t, c))
    else {
        return false;
    };
    if let Some(clip) = t.clips.iter_mut().find(|c| c.id == clip_id) {
        clip.loops = loops.clamp(1, cap);
        // A shorter span must not strand fades wider than the clip; growing
        // keeps each fade's absolute length.
        let plen = t
            .patterns
            .iter()
            .find(|p| p.id == clip.pattern_id)
            .map_or(STEPS_PER_BAR as f64, |p| p.length.max(1) as f64);
        let span = clip.span(plen);
        clip.fade_in = clean_fade(clip.fade_in, span);
        clip.fade_out = clean_fade(clip.fade_out, span);
    }
    fit_bars_to_content(project);
    true
}

/// Clamps a fade length into `0..=span`; non-finite input becomes 0.
fn clean_fade(v: f64, span: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, span)
    } else {
        0.0
    }
}

/// Sets a clip's fade-in/fade-out lengths in steps, each clamped to
/// `0..=clip_len` (the whole looped span); non-finite input becomes 0.
/// `false` if the track or clip is missing. Fades never change the span, so
/// the song length is untouched.
pub fn set_clip_fades(
    project: &mut Project,
    track_id: u64,
    clip_id: u64,
    fade_in: f64,
    fade_out: f64,
) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(span) = t
        .clips
        .iter()
        .find(|c| c.id == clip_id)
        .map(|c| clip_len(t, c))
    else {
        return false;
    };
    let clean = |v: f64| clean_fade(v, span);
    if let Some(clip) = t.clips.iter_mut().find(|c| c.id == clip_id) {
        clip.fade_in = clean(fade_in);
        clip.fade_out = clean(fade_out);
    }
    true
}

/// Places up to `times` copies of a clip back-to-back after it, stopping at
/// the maximum song length or clip limit. Returns the last new id, if any fit.
pub fn repeat_clip(
    project: &mut Project,
    track_id: u64,
    clip_id: u64,
    times: usize,
) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    let clip = t.clips.iter().find(|c| c.id == clip_id)?.clone();
    let len = clip_len(t, &clip);
    let mut last = None;
    for i in 1..=times {
        let start = clip.start + len * i as f64;
        if start + len > MAX_SONG_STEPS + 1e-9 || t.clips.len() >= MAX_CLIPS_PER_TRACK {
            break;
        }
        let id = next_id();
        t.clips.push(Clip {
            id,
            pattern_id: clip.pattern_id,
            start,
            loops: clip.loops,
            fade_in: clip.fade_in,
            fade_out: clip.fade_out,
        });
        last = Some(id);
    }
    fit_bars_to_content(project);
    last
}

pub fn collect_clips(project: &Project, ids: &BTreeSet<u64>) -> Vec<(u64, Clip)> {
    project
        .tracks
        .iter()
        .flat_map(|t| {
            t.clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .cloned()
                .map(|c| (t.id, c))
        })
        .collect()
}

pub fn clip_group_delta(project: &Project, originals: &[(u64, Clip)], dstep: f64) -> f64 {
    if originals.is_empty() {
        return 0.0;
    }
    let mut min_start = f64::INFINITY;
    let mut max_end = f64::NEG_INFINITY;
    for (track_id, clip) in originals {
        let Some(track) = track(project, *track_id) else {
            continue;
        };
        min_start = min_start.min(clip.start);
        max_end = max_end.max(clip.start + clip_len(track, clip));
    }
    if !min_start.is_finite() || !max_end.is_finite() {
        return 0.0;
    }
    let lo = -min_start;
    let hi = MAX_SONG_STEPS - max_end;
    // A group already wider than the song cannot satisfy both bounds;
    // `clamp` would panic on the inverted range, so leave it where it is.
    if hi < lo {
        return 0.0;
    }
    dstep.clamp(lo, hi)
}

pub fn move_clip_group(project: &mut Project, originals: &[(u64, Clip)], dstep: f64) -> bool {
    let dstep = clip_group_delta(project, originals, dstep);
    if let [(track_id, original)] = originals {
        return move_clip(project, *track_id, original.id, original.start + dstep);
    }
    let mut changed = false;
    for (track_id, original) in originals {
        if let Some(t) = track_mut(project, *track_id) {
            if let Some(c) = t.clips.iter_mut().find(|c| c.id == original.id) {
                let start = original.start + dstep;
                if (c.start - start).abs() > f64::EPSILON {
                    c.start = start;
                    changed = true;
                }
            }
        }
    }
    if changed {
        fit_bars_to_content(project);
    }
    changed
}

pub fn copy_clip_group(
    project: &mut Project,
    originals: &[(u64, Clip)],
    dstep: f64,
) -> Option<Vec<u64>> {
    if originals.is_empty() {
        return None;
    }
    let dstep = clip_group_delta(project, originals, dstep);
    let mut needed: Vec<(u64, usize)> = Vec::new();
    for (track_id, _) in originals {
        if let Some((_, n)) = needed.iter_mut().find(|(id, _)| id == track_id) {
            *n += 1;
        } else {
            needed.push((*track_id, 1));
        }
    }
    for (track_id, n) in &needed {
        let t = track(project, *track_id)?;
        if t.clips.len() + *n > MAX_CLIPS_PER_TRACK {
            return None;
        }
    }
    let mut ids = Vec::with_capacity(originals.len());
    for (track_id, original) in originals {
        let t = track_mut(project, *track_id)?;
        let id = next_id();
        let mut clip = original.clone();
        clip.id = id;
        clip.start = original.start + dstep;
        t.clips.push(clip);
        ids.push(id);
    }
    fit_bars_to_content(project);
    Some(ids)
}

pub fn duplicate_clip_group_after(
    project: &mut Project,
    originals: &[(u64, Clip)],
) -> Option<Vec<u64>> {
    if originals.is_empty() {
        return None;
    }
    let start = originals
        .iter()
        .map(|(_, c)| c.start)
        .fold(f64::INFINITY, f64::min);
    let end = originals
        .iter()
        .filter_map(|(track_id, clip)| {
            track(project, *track_id).map(|t| clip.start + clip_len(t, clip))
        })
        .fold(f64::NEG_INFINITY, f64::max);
    if !start.is_finite() || !end.is_finite() {
        return None;
    }
    copy_clip_group(project, originals, (end - start).max(1.0))
}

/// Gives a clip its own copy of its pattern so edits don't affect other clips.
pub fn make_clip_unique(project: &mut Project, track_id: u64, clip_id: u64) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    let pattern_id = t.clips.iter().find(|c| c.id == clip_id)?.pattern_id;
    let new_id = duplicate_pattern_in(t, pattern_id)?;
    t.clips.iter_mut().find(|c| c.id == clip_id)?.pattern_id = new_id;
    Some(new_id)
}

/// Creates an empty pattern. `None` at the per-track pattern limit.
pub fn new_pattern(track: &mut Track, length: u32) -> Option<u64> {
    if track.patterns.len() >= MAX_PATTERNS_PER_TRACK {
        return None;
    }
    let mut p = create_pattern(track.instrument, length.clamp(1, MAX_PATTERN_STEPS));
    p.notes.clear();
    let base = if track.instrument.is_drums() && track.preset.is_none() {
        "Beat"
    } else {
        "Pattern"
    };
    let existing: Vec<String> = track.patterns.iter().map(|p| p.name.clone()).collect();
    p.name = (track.patterns.len() + 1..)
        .map(|i| format!("{base} {i}"))
        .find(|n| !existing.contains(n))
        .expect("unbounded search");
    let id = p.id;
    track.patterns.push(p);
    Some(id)
}

pub fn duplicate_pattern_in(track: &mut Track, pattern_id: u64) -> Option<u64> {
    if track.patterns.len() >= MAX_PATTERNS_PER_TRACK {
        return None;
    }
    let idx = track.patterns.iter().position(|p| p.id == pattern_id)?;
    let mut p = track.patterns[idx].clone();
    p.id = next_id();
    p.name = unique_name(
        &format!("{} copy", p.name),
        track.patterns.iter().map(|p| p.name.clone()),
    );
    for n in &mut p.notes {
        n.id = next_id();
    }
    let id = p.id;
    track.patterns.insert(idx + 1, p);
    Some(id)
}

/// Deletes a pattern and every clip that plays it. The last pattern stays.
pub fn delete_pattern(track: &mut Track, pattern_id: u64) -> bool {
    if track.patterns.len() <= 1 || !track.patterns.iter().any(|p| p.id == pattern_id) {
        return false;
    }
    track.patterns.retain(|p| p.id != pattern_id);
    track.clips.retain(|c| c.pattern_id != pattern_id);
    true
}

/// Changes a pattern's length, dropping notes that would start past the end
/// (or have less than the minimum note length left) and trimming tails, so
/// no note ever overhangs the new end.
pub fn set_pattern_length(p: &mut Pattern, length: u32) {
    let length = length.clamp(1, MAX_PATTERN_STEPS);
    p.length = length;
    let end = length as f64;
    p.notes.retain(|n| end - n.start >= MIN_NOTE_DURATION);
    for n in &mut p.notes {
        n.duration = n.duration.min(end - n.start);
    }
}

/// Changes a pattern's length like [`set_pattern_length`], then clamps the
/// fades of every clip on the track that plays it, so shortening the pattern
/// can never leave a fade wider than its clip's looped span. Growing keeps
/// each fade's absolute length. `false` if the track or pattern is missing.
pub fn set_track_pattern_length(
    project: &mut Project,
    track_id: u64,
    pattern_id: u64,
    length: u32,
) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(pat) = t.patterns.iter_mut().find(|p| p.id == pattern_id) else {
        return false;
    };
    set_pattern_length(pat, length);
    let plen = pat.length as f64;
    for clip in t.clips.iter_mut().filter(|c| c.pattern_id == pattern_id) {
        let span = clip.span(plen);
        clip.fade_in = clean_fade(clip.fade_in, span);
        clip.fade_out = clean_fade(clip.fade_out, span);
    }
    true
}

pub fn drum_hit(p: &Pattern, pitch: u8, step: u32) -> Option<&Note> {
    p.notes
        .iter()
        .find(|n| n.pitch == pitch && n.start.floor() as u32 == step)
}

/// Turns a drum step on or off. Returns true if anything changed.
pub fn set_drum(p: &mut Pattern, pitch: u8, step: u32, on: bool, velocity: f32) -> bool {
    if step >= p.length {
        return false;
    }
    let exists = drum_hit(p, pitch, step).is_some();
    match (exists, on) {
        (false, true) if p.notes.len() < MAX_NOTES_PER_PATTERN => {
            p.notes.push(Note {
                id: next_id(),
                pitch,
                start: step as f64,
                duration: 1.0,
                velocity: velocity.clamp(0.05, 1.0),
            });
            true
        }
        (true, false) => {
            p.notes
                .retain(|n| !(n.pitch == pitch && n.start.floor() as u32 == step));
            true
        }
        _ => false,
    }
}

/// Cycles an active drum hit through accent levels (100% → 70% → 40%).
pub fn cycle_drum_velocity(p: &mut Pattern, pitch: u8, step: u32) -> bool {
    let Some(n) = p
        .notes
        .iter_mut()
        .find(|n| n.pitch == pitch && n.start.floor() as u32 == step)
    else {
        return false;
    };
    n.velocity = if n.velocity > 0.85 {
        0.7
    } else if n.velocity > 0.55 {
        0.4
    } else {
        1.0
    };
    true
}

/// Adds a note clamped inside the pattern. `None` at the per-pattern note limit.
pub fn add_note(
    p: &mut Pattern,
    pitch: u8,
    start: f64,
    duration: f64,
    velocity: f32,
) -> Option<u64> {
    if p.notes.len() >= MAX_NOTES_PER_PATTERN {
        return None;
    }
    let len = p.length as f64;
    let start = start.clamp(0.0, (len - 1.0).max(0.0));
    let room = (len - start).max(MIN_NOTE_DURATION);
    let duration = duration.max(MIN_NOTE_DURATION).min(room);
    let id = next_id();
    p.notes.push(Note {
        id,
        pitch: pitch.min(MAX_PITCH),
        start,
        duration,
        velocity: velocity.clamp(0.05, 1.0),
    });
    Some(id)
}

pub fn delete_notes(p: &mut Pattern, ids: &[u64]) -> bool {
    let before = p.notes.len();
    p.notes.retain(|n| !ids.contains(&n.id));
    before != p.notes.len()
}

/// Moves `originals` (snapshots taken at drag start) by a step/pitch delta,
/// clamping the whole group so it stays inside the pattern and MIDI range.
pub fn move_notes(p: &mut Pattern, originals: &[Note], dstep: f64, dpitch: i32) {
    if originals.is_empty() {
        return;
    }
    let len = p.length as f64;
    let min_start = originals
        .iter()
        .map(|n| n.start)
        .fold(f64::INFINITY, f64::min);
    let max_start = originals
        .iter()
        .map(|n| n.start)
        .fold(f64::NEG_INFINITY, f64::max);
    let lo = originals.iter().map(|n| n.pitch as i32).min().unwrap_or(0);
    let hi = originals.iter().map(|n| n.pitch as i32).max().unwrap_or(0);
    // Notes may move right until the last one starts on the final step (or stays put
    // if it already sits later, e.g. a fractional MIDI note).
    let dstep = dstep.clamp(-min_start, (len - 1.0 - max_start).max(0.0));
    let dpitch = dpitch.clamp(-lo, MAX_PITCH as i32 - hi);
    for o in originals {
        if let Some(n) = p.notes.iter_mut().find(|n| n.id == o.id) {
            n.start = o.start + dstep;
            n.pitch = (o.pitch as i32 + dpitch) as u8;
            n.duration = o.duration.min(len - n.start).max(MIN_NOTE_DURATION);
        }
    }
}

/// Changes the length of `originals` by `ddur` steps, at least `min` steps
/// and never past the pattern end.
pub fn resize_notes(p: &mut Pattern, originals: &[Note], ddur: f64, min: f64) {
    let len = p.length as f64;
    for o in originals {
        if let Some(n) = p.notes.iter_mut().find(|n| n.id == o.id) {
            n.duration = (o.duration + ddur)
                .max(min)
                .min(len - n.start)
                .max(MIN_NOTE_DURATION);
        }
    }
}

/// Copies notes to immediately after the selection's end. Returns new ids.
pub fn duplicate_notes(p: &mut Pattern, ids: &[u64]) -> Vec<u64> {
    let sel: Vec<Note> = p
        .notes
        .iter()
        .filter(|n| ids.contains(&n.id))
        .cloned()
        .collect();
    if sel.is_empty() {
        return Vec::new();
    }
    let start = sel.iter().map(|n| n.start).fold(f64::INFINITY, f64::min);
    let end = sel.iter().map(|n| n.start + n.duration).fold(0.0, f64::max);
    let offset = (end - start).max(1.0).ceil();
    let len = p.length as f64;
    let mut out = Vec::new();
    for n in sel {
        let s = n.start + offset;
        if len - s >= MIN_NOTE_DURATION && p.notes.len() < MAX_NOTES_PER_PATTERN {
            let id = next_id();
            p.notes.push(Note {
                id,
                start: s,
                duration: n.duration.min(len - s),
                ..n
            });
            out.push(id);
        }
    }
    out
}

/// Song position helpers: 16 steps per bar, 4 per beat.
pub fn format_position(step: f64) -> String {
    let s = step.max(0.0).floor() as u64;
    format!("{:>3}.{}.{}", s / 16 + 1, (s / 4) % 4 + 1, s % 4 + 1)
}

pub fn steps_to_seconds(steps: f64, bpm: f64) -> f64 {
    steps * 15.0 / bpm.max(1.0)
}

pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let m = (s / 60.0).floor();
    format!("{}:{:04.1}", m as u64, s - m * 60.0)
}

pub fn length_label(steps: u32) -> String {
    if steps.is_multiple_of(16) {
        let bars = steps / 16;
        format!("{bars} bar{}", if bars == 1 { "" } else { "s" })
    } else {
        format!("{steps} steps")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_with(inst: Instrument) -> (Project, u64) {
        let mut p = Project::empty();
        p.tracks.clear();
        let id = add_track(&mut p, inst).unwrap();
        (p, id)
    }

    #[test]
    fn track_limit_and_unique_names() {
        let (mut p, _) = project_with(Instrument::Piano);
        let b = add_track(&mut p, Instrument::Piano).unwrap();
        assert_ne!(track(&p, b).unwrap().name, p.tracks[0].name);
        while add_track(&mut p, Instrument::Bass).is_some() {}
        assert_eq!(p.tracks.len(), MAX_TRACKS);
        assert!(duplicate_track(&mut p, b).is_none());
    }

    #[test]
    fn duplicate_track_remaps_patterns_and_ids() {
        let (mut p, id) = project_with(Instrument::Lead);
        let pid = p.tracks[0].patterns[0].id;
        add_note(pattern_mut(&mut p, id, pid).unwrap(), 60, 0.0, 2.0, 0.8).unwrap();
        let dup = duplicate_track(&mut p, id).unwrap();
        let (a, b) = (&p.tracks[0], &p.tracks[1]);
        assert_eq!(b.id, dup);
        assert_ne!(a.patterns[0].id, b.patterns[0].id);
        assert_ne!(a.patterns[0].notes[0].id, b.patterns[0].notes[0].id);
        assert!(b
            .clips
            .iter()
            .all(|c| b.patterns.iter().any(|p| p.id == c.pattern_id)));
        assert!(move_track(&mut p, dup, -1));
        assert_eq!(p.tracks[0].id, dup);
        assert!(!move_track(&mut p, dup, -1));
        assert!(delete_track(&mut p, dup));
        assert_eq!(p.tracks.len(), 1);
    }

    #[test]
    fn clips_add_move_repeat_and_grow_song() {
        let (mut p, id) = project_with(Instrument::Drums);
        p.bars = 4;
        p.tracks[0].clips.clear();
        p.tracks[0].patterns[0].length = 16;
        let pid = p.tracks[0].patterns[0].id;
        let c = add_clip(&mut p, id, pid, 48.0).unwrap();
        assert_eq!(p.bars, 4);
        assert!(move_clip(&mut p, id, c, 96.0));
        assert_eq!(p.bars, 7);
        repeat_clip(&mut p, id, c, 3).unwrap();
        let starts: Vec<f64> = p.tracks[0].clips.iter().map(|c| c.start).collect();
        assert!(starts.contains(&112.0) && starts.contains(&144.0));
        assert_eq!(p.bars, 10);
        assert!(move_clip(&mut p, id, c, -10.0));
        assert_eq!(
            p.tracks[0].clips.iter().find(|x| x.id == c).unwrap().start,
            0.0
        );
        assert!(delete_clip(&mut p, id, c));
        assert!(!delete_clip(&mut p, id, c));
        assert!(add_clip(&mut p, id, 12345, 0.0).is_none());
    }

    #[test]
    fn unique_clip_gets_its_own_pattern() {
        let (mut p, id) = project_with(Instrument::Pad);
        let old = p.tracks[0].patterns[0].id;
        let patterns = p.tracks[0].patterns.len();
        let c = add_clip(&mut p, id, old, 0.0).unwrap();
        let new = make_clip_unique(&mut p, id, c).unwrap();
        assert_ne!(old, new);
        assert_eq!(p.tracks[0].patterns.len(), patterns + 1);
        assert_eq!(
            p.tracks[0]
                .clips
                .iter()
                .find(|x| x.id == c)
                .unwrap()
                .pattern_id,
            new
        );
    }

    #[test]
    fn patterns_create_duplicate_delete_and_length() {
        let (mut p, _) = project_with(Instrument::Bass);
        let t = &mut p.tracks[0];
        t.patterns.truncate(1);
        let first = t.patterns[0].id;
        let n = new_pattern(t, 32).unwrap();
        assert_eq!(t.patterns.len(), 2);
        assert_ne!(t.patterns[0].name, t.patterns[1].name);
        let pat = t.patterns.iter_mut().find(|p| p.id == n).unwrap();
        add_note(pat, 40, 20.0, 8.0, 0.7);
        add_note(pat, 41, 10.0, 12.0, 0.7);
        set_pattern_length(pat, 16);
        assert_eq!(pat.notes.len(), 1);
        assert_eq!(pat.notes[0].duration, 6.0);
        let d = duplicate_pattern_in(t, n).unwrap();
        assert!(t
            .patterns
            .iter()
            .find(|p| p.id == d)
            .unwrap()
            .name
            .contains("copy"));
        t.clips.push(Clip {
            id: next_id(),
            pattern_id: d,
            start: 0.0,
            loops: 1,
            fade_in: 0.0,
            fade_out: 0.0,
        });
        assert!(delete_pattern(t, d));
        assert!(t.clips.iter().all(|c| c.pattern_id != d));
        assert!(delete_pattern(t, n));
        assert!(!delete_pattern(t, first), "last pattern is kept");
    }

    #[test]
    fn drum_steps_toggle_and_accent() {
        let mut pat = create_pattern(Instrument::Drums, 16);
        pat.notes.clear();
        assert!(set_drum(&mut pat, 36, 4, true, 1.0));
        assert!(!set_drum(&mut pat, 36, 4, true, 0.8));
        assert!(!set_drum(&mut pat, 36, 16, true, 0.8), "outside pattern");
        for v in [0.7, 0.4, 1.0] {
            assert!(cycle_drum_velocity(&mut pat, 36, 4));
            assert_eq!(
                drum_hit(&pat, 36, 4).unwrap().velocity,
                v,
                "accent cycle step"
            );
        }
        assert!(!cycle_drum_velocity(&mut pat, 38, 4), "nothing to cycle");
        assert!(set_drum(&mut pat, 36, 4, false, 0.8));
        assert!(pat.notes.is_empty());
    }

    #[test]
    fn note_move_resize_clamps_group() {
        let mut pat = create_pattern(Instrument::Piano, 16);
        pat.notes.clear();
        let a = add_note(&mut pat, 60, 2.0, 2.0, 0.8).unwrap();
        let b = add_note(&mut pat, 126, 6.0, 4.0, 0.8).unwrap();
        let orig = pat.notes.clone();
        move_notes(&mut pat, &orig, -5.0, 5);
        let get = |p: &Pattern, id| p.notes.iter().find(|n| n.id == id).unwrap().clone();
        assert_eq!(get(&pat, a).start, 0.0);
        assert_eq!(get(&pat, b).start, 4.0);
        assert_eq!(get(&pat, b).pitch, 127);
        assert_eq!(get(&pat, a).pitch, 61);
        let orig = pat.notes.clone();
        resize_notes(&mut pat, &orig, 20.0, 1.0);
        assert_eq!(get(&pat, b).duration, 12.0);
        resize_notes(&mut pat, &orig, -20.0, 1.0);
        assert_eq!(get(&pat, a).duration, 1.0);
        let dups = duplicate_notes(&mut pat, &[a]);
        assert_eq!(dups.len(), 1);
        assert!(delete_notes(&mut pat, &[a, b]));
        assert_eq!(pat.notes.len(), 1);
    }

    #[test]
    fn add_note_clamps_to_pattern() {
        let mut pat = create_pattern(Instrument::Lead, 16);
        pat.notes.clear();
        let id = add_note(&mut pat, 200, 15.0, 4.0, 2.0).unwrap();
        let n = &pat.notes[0];
        assert_eq!(
            (n.id, n.pitch, n.start, n.duration, n.velocity),
            (id, 127, 15.0, 1.0, 1.0)
        );
    }

    #[test]
    fn shortening_pattern_never_leaves_overhanging_fractional_notes() {
        // In-place on a real melodic track so the clip's pattern_id stays valid.
        let (mut p, _) = project_with(Instrument::Piano);
        {
            let pat = &mut p.tracks[0].patterns[0];
            pat.notes.clear();
            set_pattern_length(pat, 32);
            pat.notes.push(Note {
                id: next_id(),
                pitch: 60,
                start: 15.97,
                duration: 2.0,
                velocity: 0.5,
            });
            pat.notes.push(Note {
                id: next_id(),
                pitch: 62,
                start: 15.5,
                duration: 2.0,
                velocity: 0.5,
            });
            set_pattern_length(pat, 16);
            assert_eq!(
                pat.notes.len(),
                1,
                "a note with < minimum length left is dropped"
            );
            assert!(pat
                .notes
                .iter()
                .all(|n| n.start + n.duration <= 16.0 + 1e-9 && n.duration >= MIN_NOTE_DURATION));
        }
        assert!(p.validate().is_ok());
    }

    #[test]
    fn clips_never_leave_the_maximum_song_and_project_stays_valid() {
        let (mut p, id) = project_with(Instrument::Bass);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id;
        crate::gui::ops::set_pattern_length(&mut p.tracks[0].patterns[0], 64);
        let c = add_clip(&mut p, id, pid, MAX_SONG_STEPS + 500.0).unwrap();
        let start = |p: &Project| p.tracks[0].clips.iter().find(|x| x.id == c).unwrap().start;
        assert_eq!(start(&p), MAX_SONG_STEPS - 64.0);
        assert_eq!(p.bars, MAX_BARS);
        assert!(
            repeat_clip(&mut p, id, c, 4).is_none(),
            "no room after the last bar"
        );
        assert!(move_clip(&mut p, id, c, MAX_SONG_STEPS - 100.0));
        let extra = repeat_clip(&mut p, id, c, 4);
        assert!(
            extra.is_none(),
            "a 64-step copy at 1948 would overhang 2048"
        );
        assert!(move_clip(&mut p, id, c, MAX_SONG_STEPS - 192.0));
        assert!(repeat_clip(&mut p, id, c, 10).is_some());
        assert!(p.tracks[0]
            .clips
            .iter()
            .all(|x| x.start + 64.0 <= MAX_SONG_STEPS));
        assert!(p.validate().is_ok());
    }

    #[test]
    fn preset_tracks_embed_a_copy_of_the_preset() {
        let catalog =
            crate::packs::load_catalog_from(std::path::Path::new("/nonexistent-cadence-test-dir"));
        let preset = catalog
            .instruments
            .first()
            .expect("bundled 8-bit pack")
            .clone();
        let mut p = Project::empty();
        let id = add_preset_track(&mut p, &preset).unwrap();
        let t = track(&p, id).unwrap();
        assert_eq!(t.preset.as_ref(), Some(&preset));
        assert_eq!(instrument_label(t), preset.name);
        assert!(!t.instrument.is_drums());
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert!(back.validate().is_ok());
    }

    #[test]
    fn formatting_helpers() {
        assert_eq!(format_position(0.0), "  1.1.1");
        assert_eq!(format_position(21.5), "  2.2.2");
        assert_eq!(steps_to_seconds(16.0, 120.0), 2.0);
        assert_eq!(format_time(75.25), "1:15.2");
        assert_eq!(length_label(32), "2 bars");
        assert_eq!(length_label(16), "1 bar");
        assert_eq!(length_label(12), "12 steps");
    }

    #[test]
    fn set_clip_loops_clamps_and_grows_the_song() {
        let (mut p, id) = project_with(Instrument::Piano);
        p.bars = 1;
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id; // one-bar pattern, no notes
        let c = add_clip(&mut p, id, pid, 0.0).unwrap();
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!((clip.loops, clip.fade_in, clip.fade_out), (1, 0.0, 0.0));
            assert_eq!(clip_len(t, clip), 16.0);
            assert_eq!(max_clip_loops(t, clip), MAX_CLIP_LOOPS);
        }
        assert!(set_clip_loops(&mut p, id, c, 4));
        assert_eq!(p.bars, 4, "4 loops of a 1-bar pattern need 4 bars");
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!(clip.loops, 4);
            assert_eq!(clip_len(t, clip), 64.0, "clip_len is the looped span");
        }
        assert!(set_clip_loops(&mut p, id, c, 0), "0 clamps up to 1");
        {
            let t = track(&p, id).unwrap();
            assert_eq!(t.clips.iter().find(|x| x.id == c).unwrap().loops, 1);
        }
        assert!(set_clip_loops(&mut p, id, c, 999));
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!(clip.loops, MAX_CLIP_LOOPS);
            assert_eq!(clip_len(t, clip), 16.0 * MAX_CLIP_LOOPS as f64);
        }
        assert!(!set_clip_loops(&mut p, 12345, c, 2));
        assert!(!set_clip_loops(&mut p, id, 12345, 2));
        // A dense pattern lowers the cap so flattened events stay in budget:
        // 7000 notes allow floor(200000/7000) = 28 loops, under MAX_CLIP_LOOPS.
        {
            let t = track_mut(&mut p, id).unwrap();
            let pat = t.patterns.iter_mut().find(|p| p.id == pid).unwrap();
            pat.notes = (0..7000)
                .map(|i| Note {
                    id: next_id(),
                    pitch: 60,
                    start: (i % 16) as f64,
                    duration: 1.0,
                    velocity: 0.8,
                })
                .collect();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            let cap = max_clip_loops(t, clip);
            assert_eq!(cap, (MAX_SONG_EVENTS / 7000) as u32);
            assert!(cap < MAX_CLIP_LOOPS);
            assert!(set_clip_loops(&mut p, id, c, MAX_CLIP_LOOPS));
            let t = track(&p, id).unwrap();
            assert_eq!(t.clips.iter().find(|x| x.id == c).unwrap().loops, cap);
        }
        assert!(p.validate().is_ok(), "7000 notes x 28 loops fit the budget");
    }

    #[test]
    fn shortening_clip_loops_clamps_fades_to_the_new_span() {
        // 16-step pattern, loops=4 -> span 64: fades of 32/24 steps validate.
        // Dropping to loops=1 quarters the span to 16, so both fades must be
        // pulled inside it or the project stops validating.
        let (mut p, id) = project_with(Instrument::Piano);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id;
        let c = add_clip(&mut p, id, pid, 0.0).unwrap();
        assert!(set_clip_loops(&mut p, id, c, 4));
        assert!(set_clip_fades(&mut p, id, c, 32.0, 24.0));
        assert!(p.validate().is_ok());

        assert!(set_clip_loops(&mut p, id, c, 1));
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!(clip.span(16.0), 16.0);
            assert_eq!((clip.fade_in, clip.fade_out), (16.0, 16.0));
        }
        assert!(
            p.validate().is_ok(),
            "fades wider than the shrunken span must be clamped"
        );
        // The validated project must survive the save/load JSON roundtrip.
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        back.validate().unwrap();
    }

    #[test]
    fn growing_clip_loops_preserves_absolute_fade_lengths() {
        let (mut p, id) = project_with(Instrument::Bass);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id;
        let c = add_clip(&mut p, id, pid, 0.0).unwrap();
        assert!(set_clip_fades(&mut p, id, c, 6.0, 4.0));
        assert!(set_clip_loops(&mut p, id, c, 4));
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!((clip.fade_in, clip.fade_out), (6.0, 4.0));
        }
        assert!(p.validate().is_ok());
    }

    #[test]
    fn shortening_a_shared_pattern_clamps_every_referencing_clip() {
        // Two clips share one 16-step pattern with different loop counts, so
        // shortening the pattern shrinks their spans by different amounts.
        let (mut p, id) = project_with(Instrument::Piano);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id;
        let a = add_clip(&mut p, id, pid, 0.0).unwrap();
        let b = add_clip(&mut p, id, pid, 128.0).unwrap();
        assert!(set_clip_loops(&mut p, id, a, 4)); // span 64
        assert!(set_clip_loops(&mut p, id, b, 2)); // span 32
        assert!(set_clip_fades(&mut p, id, a, 48.0, 40.0));
        assert!(set_clip_fades(&mut p, id, b, 20.0, 12.0));
        assert!(p.validate().is_ok());

        // 16 -> 8 steps: A's span is 32, B's is 16. Fades past each span clamp.
        assert!(set_track_pattern_length(&mut p, id, pid, 8));
        {
            let t = track(&p, id).unwrap();
            let clip = |cid| t.clips.iter().find(|x| x.id == cid).unwrap();
            assert_eq!(clip(a).span(8.0), 32.0);
            assert_eq!((clip(a).fade_in, clip(a).fade_out), (32.0, 32.0));
            assert_eq!(clip(b).span(8.0), 16.0);
            assert_eq!((clip(b).fade_in, clip(b).fade_out), (16.0, 12.0));
        }
        assert!(p.validate().is_ok());
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        back.validate().unwrap();

        // Growing back to 16 keeps each fade's (already clamped) absolute
        // length; it does not restore the pre-shrink values.
        assert!(set_track_pattern_length(&mut p, id, pid, 16));
        {
            let t = track(&p, id).unwrap();
            let clip = |cid| t.clips.iter().find(|x| x.id == cid).unwrap();
            assert_eq!((clip(a).fade_in, clip(a).fade_out), (32.0, 32.0));
            assert_eq!((clip(b).fade_in, clip(b).fade_out), (16.0, 12.0));
        }
        assert!(p.validate().is_ok());

        assert!(!set_track_pattern_length(&mut p, 12345, pid, 8));
        assert!(!set_track_pattern_length(&mut p, id, 12345, 8));
    }

    #[test]
    fn clip_group_moves_freeze_when_the_group_spans_past_the_song() {
        let (mut p, id) = project_with(Instrument::Bass);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id;
        set_pattern_length(&mut p.tracks[0].patterns[0], 256);
        let big = add_clip(&mut p, id, pid, 0.0).unwrap();
        p.tracks[0]
            .clips
            .iter_mut()
            .find(|c| c.id == big)
            .unwrap()
            .loops = 32; // 256 x 32 = 8192 steps, wider than MAX_SONG_STEPS
        let small = add_clip(&mut p, id, pid, 64.0).unwrap();
        let originals: Vec<(u64, Clip)> =
            p.tracks[0].clips.iter().map(|c| (id, c.clone())).collect();
        assert_eq!(clip_group_delta(&p, &originals, 8.0), 0.0);
        assert!(!move_clip_group(&mut p, &originals, 8.0));
        {
            let t = track(&p, id).unwrap();
            let start = |cid: u64| t.clips.iter().find(|c| c.id == cid).unwrap().start;
            assert_eq!((start(big), start(small)), (0.0, 64.0));
        }
        // The single-clip fast path shares the same guard and must not panic.
        let single = vec![(id, originals[0].1.clone())];
        assert_eq!(clip_group_delta(&p, &single, 8.0), 0.0);
        let _ = move_clip_group(&mut p, &single, 8.0);
        assert_eq!(
            track(&p, id)
                .unwrap()
                .clips
                .iter()
                .find(|c| c.id == big)
                .unwrap()
                .start,
            0.0
        );
    }

    #[test]
    fn set_clip_fades_clamp_to_span_and_survive_repeat() {
        let (mut p, id) = project_with(Instrument::Bass);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id; // one-bar pattern
        let c = add_clip(&mut p, id, pid, 0.0).unwrap();
        assert!(set_clip_loops(&mut p, id, c, 2)); // span = 32 steps
        assert!(set_clip_fades(&mut p, id, c, 8.0, 4.0));
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!((clip.fade_in, clip.fade_out), (8.0, 4.0));
        }
        // Each fade clamps to the whole looped span; bad input becomes 0.
        assert!(set_clip_fades(&mut p, id, c, 9999.0, -2.0));
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!((clip.fade_in, clip.fade_out), (32.0, 0.0));
        }
        assert!(set_clip_fades(&mut p, id, c, f64::NAN, f64::INFINITY));
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == c).unwrap();
            assert_eq!((clip.fade_in, clip.fade_out), (0.0, 0.0));
        }
        assert!(!set_clip_fades(&mut p, 12345, c, 1.0, 1.0));
        assert!(!set_clip_fades(&mut p, id, 12345, 1.0, 1.0));
        // Repeats copy loops and fades and start one whole span later.
        assert!(set_clip_fades(&mut p, id, c, 3.0, 5.0));
        let copy = repeat_clip(&mut p, id, c, 1).unwrap();
        {
            let t = track(&p, id).unwrap();
            let clip = t.clips.iter().find(|x| x.id == copy).unwrap();
            assert_eq!(clip.loops, 2);
            assert_eq!((clip.fade_in, clip.fade_out), (3.0, 5.0));
            assert_eq!(clip.start, 32.0);
        }
    }
}
