//! Pure, testable project editing operations used by the GUI.
//!
//! Everything here mutates a [`Project`] (or part of it) and never touches
//! egui, audio or the filesystem, so undo/redo can wrap each call.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::model::{
    create_audio_track, create_pattern, create_track, next_id, AudioClip, Automation,
    AutomationLane, AutomationPoint, ChordEvent, ChordQuality, Clip, FontRef, Instrument, KeyMode,
    Marker, Note, Pattern, Project, SongKey, Track, MAX_NAME_CHARS,
};
use crate::packs::InstrumentPreset;

pub use crate::model::{
    ArpDirection, MAX_ARP_OCTAVES, MAX_AUDIO_CLIPS_PER_TRACK, MAX_AUTOMATION_POINTS, MAX_BARS,
    MAX_BPM, MAX_CHORD_EVENTS, MAX_CLIPS_PER_TRACK, MAX_CLIP_LOOPS, MAX_MARKERS,
    MAX_NOTES_PER_PATTERN, MAX_PATTERNS_PER_TRACK, MAX_PATTERN_STEPS, MAX_SONG_EVENTS, MAX_SWING,
    MAX_TRACKS, MIN_AUDIO_CLIP_STEPS, MIN_BARS, MIN_BPM, MIN_NOTE_DURATION, NUM_BUSES,
    STEPS_PER_BAR,
};

pub const MAX_PITCH: u8 = 127;
/// Pattern lengths offered in the editor, in steps.
pub const PATTERN_LENGTHS: [u32; 8] = [8, 16, 32, 48, 64, 128, 192, 256];
/// Longest song in steps; clips must start before and fit inside this.
pub const MAX_SONG_STEPS: f64 = (MAX_BARS * STEPS_PER_BAR) as f64;

/// Display label for a track's sound: "Audio" for imported-WAV lanes,
/// "Font · Program" for SoundFont tracks ([`FontRef::label`]), else the
/// pack preset name or the built-in name — the same precedence playback
/// uses. Borrowed for preset/built-in tracks so the per-frame callers
/// (arranger, mixer, editor) don't allocate.
pub fn instrument_label(track: &Track) -> Cow<'_, str> {
    if track.audio {
        return Cow::Borrowed("Audio");
    }
    match &track.font {
        Some(font) => Cow::Owned(font.label()),
        None => Cow::Borrowed(track.sound_name()),
    }
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

/// End of the last clip in the song, in steps — imported-WAV clips count
/// just like pattern clips so the arranger scrolls and `bars` fits them.
pub fn content_end(project: &Project) -> f64 {
    project
        .tracks
        .iter()
        .flat_map(|t| {
            t.clips
                .iter()
                .map(move |c| c.start + clip_len(t, c))
                .chain(t.audio_clips.iter().map(|a| a.start + a.span()))
        })
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
/// A kit preset becomes a Drums track (drum step grid, "Beat" patterns);
/// melodic presets stay on the piano-roll Lead track.
pub fn add_preset_track(project: &mut Project, preset: &InstrumentPreset) -> Option<u64> {
    let instrument = if preset.kit.is_some() {
        Instrument::Drums
    } else {
        Instrument::Lead
    };
    let mut t = create_track(instrument);
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
    for c in &mut t.audio_clips {
        c.id = next_id();
    }
    let id = t.id;
    project.tracks.insert(idx + 1, t);
    Some(id)
}

/// Appends an independent deep copy of `source` — fresh ids for the track,
/// every pattern, note and clip, with each clip's `pattern_id` remapped onto
/// the copy's own patterns — at the end of the track list. Unlike
/// [`duplicate_track`] the copy lands at the bottom and never sits next to
/// its source; like [`push_track`] the name is uniquified ("Bass 2") and a
/// colliding colour is re-picked. `solo` is cleared so the paste doesn't
/// suddenly solo the song; mute, volume, pan, effects, instrument, preset
/// and font all carry over. Clips whose pattern cannot be remapped are
/// dropped rather than left pointing at another track's pattern. `None` at
/// the track limit.
pub fn paste_track(project: &mut Project, source: &Track) -> Option<u64> {
    if project.tracks.len() >= MAX_TRACKS {
        return None;
    }
    let mut t = source.clone();
    t.id = next_id();
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
    t.clips
        .retain(|c| map.iter().any(|(old, _)| *old == c.pattern_id));
    for c in &mut t.clips {
        c.id = next_id();
        if let Some((_, new)) = map.iter().find(|(old, _)| *old == c.pattern_id) {
            c.pattern_id = *new;
        }
    }
    // Audio clips reference the shared store by name, not this track's
    // patterns — they copy verbatim apart from a fresh id.
    for c in &mut t.audio_clips {
        c.id = next_id();
    }
    let id = push_track(project, t)?;
    fit_bars_to_content(project);
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

/// Cleans a [`FontRef`] the same way `Project::normalize` repairs loaded
/// songs: control characters dropped, name capped at [`MAX_NAME_CHARS`] and
/// trimmed, program clamped to 0..=127. An empty result means "no font".
fn clean_font_ref(font: FontRef) -> Option<FontRef> {
    let name = font
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(crate::model::MAX_NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    Some(FontRef::new(name, font.program.min(127)))
}

/// Sets or clears (`None`) the SoundFont program a track plays.
///
/// Font references take precedence over `preset`/`instrument` at playback
/// (`TrackParams::from_track`), while the preset stays as the automatic
/// fallback when the font file is not installed — the coexistence
/// [`Track::from_font`] documents. The picker clears the font when a preset
/// or built-in sound is chosen instead, so only one "active" sound override
/// exists at a time; this op itself never touches `preset`.
///
/// Assigning a font to a track that shows the drum grid also switches it to
/// [`Instrument::Lead`]: SoundFonts only play pitched GM programs on channel
/// 0, so the melodic piano roll is the editor that fits. Clearing the font
/// does not move the track back (undo does).
///
/// Returns `false` when the track is missing or the reference is unchanged
/// (an invalid `font` cleans to `None`, i.e. a clear).
pub fn set_track_font(project: &mut Project, track_id: u64, font: Option<FontRef>) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let font = font.and_then(clean_font_ref);
    if t.font == font {
        return false;
    }
    t.font = font;
    if t.font.is_some() && t.shows_drum_grid() {
        t.instrument = Instrument::Lead;
    }
    true
}

// ----- subgroup buses ------------------------------------------------------

/// One-character route label for a [`Track::bus`] value: `"M"` for the
/// master (bus 0), `"A"`..=`"D"` for the fixed subgroups. Out-of-range
/// values (only reachable unvalidated) label as `"?"`.
pub fn bus_label(bus: u8) -> &'static str {
    match bus {
        0 => "M",
        b if (1..=NUM_BUSES as u8).contains(&b) => {
            const LABELS: [&str; NUM_BUSES] = ["A", "B", "C", "D"];
            LABELS[(b - 1) as usize]
        }
        _ => "?",
    }
}

/// Routes a track to the master (`bus = 0`) or one of the fixed subgroup
/// buses (`1..=NUM_BUSES`), clamped like `Project::normalize` does.
/// `false` when the track is missing or already on that bus.
pub fn set_track_bus(project: &mut Project, track_id: u64, bus: u8) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let bus = bus.min(NUM_BUSES as u8);
    if t.bus == bus {
        return false;
    }
    t.bus = bus;
    true
}

/// Renames bus `index` (`0..NUM_BUSES`); the name goes through the same
/// cleaning as load repair — control chars stripped, length capped, empty
/// falls back to the letter name. `false` if the index is out of range or
/// the cleaned name is unchanged.
pub fn rename_bus(project: &mut Project, index: usize, name: &str) -> bool {
    let Some(bus) = project.buses.get_mut(index) else {
        return false;
    };
    let fallback = crate::model::bus_name(index).to_owned();
    let mut cleaned = name.to_string();
    crate::model::clamp_name(&mut cleaned, &fallback);
    if bus.name == cleaned {
        return false;
    }
    bus.name = cleaned;
    true
}

/// Tracks currently routed to `bus` (0 = master). Used by the mixer to show
/// whether a subgroup is in use.
pub fn bus_members(project: &Project, bus: u8) -> usize {
    project.tracks.iter().filter(|t| t.bus == bus).count()
}

/// Places a clip, clamped inside the maximum song length. `None` if the
/// pattern is missing or the track is at its clip limit.
pub fn add_clip(project: &mut Project, track_id: u64, pattern_id: u64, start: f64) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    // Audio lanes schedule `audio_clips`, never patterns — a pattern clip
    // placed here would play silence.
    if t.audio {
        return None;
    }
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

// ----- audio clips ---------------------------------------------------------

/// Adds an audio lane — the track kind imported-WAV clips live on.
/// `None` at the track limit.
pub fn add_audio_track(project: &mut Project) -> Option<u64> {
    push_track(project, create_audio_track("Audio"))
}

/// Whether `clip_id` is an imported-WAV clip on `track` (vs a pattern clip).
pub fn is_audio_clip(track: &Track, clip_id: u64) -> bool {
    track.audio_clips.iter().any(|c| c.id == clip_id)
}

/// The imported-WAV clip `clip_id` on `track`.
pub fn audio_clip(track: &Track, clip_id: u64) -> Option<&AudioClip> {
    track.audio_clips.iter().find(|c| c.id == clip_id)
}

/// Places a staged audio file as a clip on `track_id`. `file` is the
/// store-relative name from `persistence::stage_audio_file`; `name` the
/// display label; `length` the steps one repetition spans. `None` at the
/// per-track audio-clip cap or when the track is missing.
pub fn add_audio_clip(
    project: &mut Project,
    track_id: u64,
    file: &str,
    name: &str,
    start: f64,
    length: f64,
) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    if t.audio_clips.len() >= MAX_AUDIO_CLIPS_PER_TRACK {
        return None;
    }
    let length = if length.is_finite() {
        length.clamp(MIN_AUDIO_CLIP_STEPS, MAX_SONG_STEPS)
    } else {
        STEPS_PER_BAR as f64
    };
    let mut clip = AudioClip::new(file, name, clamp_clip_start(start, length), length);
    crate::model::clamp_name(&mut clip.name, "Audio");
    let id = clip.id;
    t.audio_clips.push(clip);
    fit_bars_to_content(project);
    Some(id)
}

/// Moves an imported-WAV clip (clamped like [`move_clip`]).
pub fn move_audio_clip(project: &mut Project, track_id: u64, clip_id: u64, start: f64) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(len) = audio_clip(t, clip_id).map(|c| c.span()) else {
        return false;
    };
    if let Some(c) = t.audio_clips.iter_mut().find(|c| c.id == clip_id) {
        c.start = clamp_clip_start(start, len);
    }
    fit_bars_to_content(project);
    true
}

/// Deletes an imported-WAV clip. `false` if the track or clip is missing.
pub fn delete_audio_clip(project: &mut Project, track_id: u64, clip_id: u64) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let before = t.audio_clips.len();
    t.audio_clips.retain(|c| c.id != clip_id);
    t.audio_clips.len() != before
}

/// Sets an audio clip's loop count, clamped to `1..=MAX_CLIP_LOOPS` (audio
/// clips produce no note events, so the event budget never binds), then
/// re-clamps fades to the new span — same rules as [`set_clip_loops`].
pub fn set_audio_clip_loops(
    project: &mut Project,
    track_id: u64,
    clip_id: u64,
    loops: u32,
) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(clip) = t.audio_clips.iter_mut().find(|c| c.id == clip_id) else {
        return false;
    };
    clip.loops = loops.clamp(1, MAX_CLIP_LOOPS);
    let span = clip.span();
    clip.fade_in = clean_fade(clip.fade_in, span);
    clip.fade_out = clean_fade(clip.fade_out, span);
    fit_bars_to_content(project);
    true
}

/// Sets an audio clip's fade-in/fade-out lengths in steps, each clamped to
/// `0..=span` — same rules as [`set_clip_fades`].
pub fn set_audio_clip_fades(
    project: &mut Project,
    track_id: u64,
    clip_id: u64,
    fade_in: f64,
    fade_out: f64,
) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(span) = audio_clip(t, clip_id).map(|c| c.span()) else {
        return false;
    };
    let clean = |v: f64| clean_fade(v, span);
    if let Some(clip) = t.audio_clips.iter_mut().find(|c| c.id == clip_id) {
        clip.fade_in = clean(fade_in);
        clip.fade_out = clean(fade_out);
    }
    true
}

/// Steps one clip repetition occupies: the pattern length for pattern
/// clips, [`AudioClip::length`] for imported-WAV clips.
pub fn any_clip_rep_len(track: &Track, clip_id: u64) -> Option<f64> {
    if let Some(c) = track.clips.iter().find(|c| c.id == clip_id) {
        let plen =
            pattern(track, c.pattern_id).map_or(STEPS_PER_BAR as f64, |p| p.length.max(1) as f64);
        return Some(plen);
    }
    audio_clip(track, clip_id).map(|c| c.length)
}

/// Largest loop count the gesture layer may offer for `clip_id`: the
/// event-budget cap for pattern clips, [`MAX_CLIP_LOOPS`] for audio.
pub fn max_any_clip_loops(track: &Track, clip_id: u64) -> Option<u32> {
    if let Some(c) = track.clips.iter().find(|c| c.id == clip_id) {
        return Some(max_clip_loops(track, c));
    }
    audio_clip(track, clip_id).map(|_| MAX_CLIP_LOOPS)
}

/// Current `(start, span, loops, fade_in, fade_out)` of `clip_id` whichever
/// kind it is — the snapshot the fade/loop drags and context menu read.
pub fn any_clip_props(track: &Track, clip_id: u64) -> Option<(f64, f64, u32, f64, f64)> {
    if let Some(c) = track.clips.iter().find(|c| c.id == clip_id) {
        return Some((c.start, clip_len(track, c), c.loops, c.fade_in, c.fade_out));
    }
    audio_clip(track, clip_id).map(|c| (c.start, c.span(), c.loops, c.fade_in, c.fade_out))
}

/// Sets a clip's loop count whichever kind it is (see [`set_clip_loops`]
/// and [`set_audio_clip_loops`]).
pub fn set_any_clip_loops(project: &mut Project, track_id: u64, clip_id: u64, loops: u32) -> bool {
    match track(project, track_id) {
        Some(t) if is_audio_clip(t, clip_id) => {
            set_audio_clip_loops(project, track_id, clip_id, loops)
        }
        _ => set_clip_loops(project, track_id, clip_id, loops),
    }
}

/// Sets a clip's fades whichever kind it is.
pub fn set_any_clip_fades(
    project: &mut Project,
    track_id: u64,
    clip_id: u64,
    fade_in: f64,
    fade_out: f64,
) -> bool {
    match track(project, track_id) {
        Some(t) if is_audio_clip(t, clip_id) => {
            set_audio_clip_fades(project, track_id, clip_id, fade_in, fade_out)
        }
        _ => set_clip_fades(project, track_id, clip_id, fade_in, fade_out),
    }
}

/// Deletes a clip whichever kind it is (see [`delete_clip`] /
/// [`delete_audio_clip`]).
pub fn delete_any_clip(project: &mut Project, track_id: u64, clip_id: u64) -> bool {
    match track(project, track_id) {
        Some(t) if is_audio_clip(t, clip_id) => delete_audio_clip(project, track_id, clip_id),
        _ => delete_clip(project, track_id, clip_id),
    }
}

/// The next default name for a section marker, cycling through
/// Intro/Verse/Chorus/... and appending a number once a name repeats.
pub fn next_marker_name(project: &Project) -> String {
    const SECTIONS: [&str; 8] = [
        "Intro", "Verse", "Chorus", "Bridge", "Break", "Drop", "Solo", "Outro",
    ];
    let mut names = BTreeSet::new();
    for m in &project.markers {
        names.insert(m.name.as_str());
    }
    let mut round = 1;
    loop {
        for base in SECTIONS {
            let name = if round == 1 {
                base.to_string()
            } else {
                format!("{base} {round}")
            };
            if !names.contains(name.as_str()) {
                return name;
            }
        }
        round += 1;
    }
}

/// Adds a section marker at `step` (clamped to `0..=MAX_PATTERN_STEPS`),
/// named `name` or [`next_marker_name`] when `None`. `None` result means the
/// [`MAX_MARKERS`] cap is reached. The list is kept sorted by step then id.
pub fn add_marker(project: &mut Project, step: f64, name: Option<String>) -> Option<u64> {
    if project.markers.len() >= MAX_MARKERS {
        return None;
    }
    let mut name = name.unwrap_or_else(|| next_marker_name(project));
    crate::model::clamp_name(&mut name, "Section");
    let id = next_id();
    project.markers.push(Marker {
        id,
        name,
        step: clamp_marker_step(step),
    });
    project
        .markers
        .sort_by(|a, b| a.step.total_cmp(&b.step).then(a.id.cmp(&b.id)));
    Some(id)
}

/// Moves a marker to `step` (clamped like [`add_marker`]) and re-sorts the
/// list. `false` if no marker has that id.
pub fn move_marker(project: &mut Project, marker_id: u64, step: f64) -> bool {
    let Some(m) = project.markers.iter_mut().find(|m| m.id == marker_id) else {
        return false;
    };
    m.step = clamp_marker_step(step);
    project
        .markers
        .sort_by(|a, b| a.step.total_cmp(&b.step).then(a.id.cmp(&b.id)));
    true
}

/// Renames a marker; the name goes through the same cleaning as load repair
/// (control chars stripped, length capped, empty falls back to the old name).
/// `false` if the id is unknown.
pub fn rename_marker(project: &mut Project, marker_id: u64, name: &str) -> bool {
    let Some(m) = project.markers.iter_mut().find(|m| m.id == marker_id) else {
        return false;
    };
    let mut cleaned = name.to_string();
    crate::model::clamp_name(&mut cleaned, &m.name.clone());
    m.name = cleaned;
    true
}

/// Deletes a marker. `false` if the id is unknown.
pub fn delete_marker(project: &mut Project, marker_id: u64) -> bool {
    let before = project.markers.len();
    project.markers.retain(|m| m.id != marker_id);
    project.markers.len() != before
}

/// Clamps a marker position to `0..=MAX_PATTERN_STEPS`; non-finite becomes 0.
fn clamp_marker_step(step: f64) -> f64 {
    if step.is_finite() {
        step.clamp(0.0, MAX_PATTERN_STEPS as f64)
    } else {
        0.0
    }
}

/// Clamps an automation point's `value` to its lane's range; non-finite falls
/// back to `fallback` (the point's previous value on moves, the lane's
/// neutral value on adds) rather than writing NaN into the model.
fn clamp_auto_value(lane: AutomationLane, value: f32, fallback: f32) -> f32 {
    let (lo, hi) = Automation::value_range(lane);
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        fallback
    }
}

/// Adds a point at (`step`, `value`) on `track`'s `lane`, creating the
/// track's automation block on first use. A point already on `step` takes
/// the new value instead of stacking a second breakpoint there. The lane
/// stays sorted by step; the point's index inside it is returned, or `None`
/// when the [`MAX_AUTOMATION_POINTS`] cap is reached.
pub fn add_automation_point(
    project: &mut Project,
    track_id: u64,
    lane: AutomationLane,
    step: f64,
    value: f32,
) -> Option<usize> {
    let t = track_mut(project, track_id)?;
    let point = AutomationPoint {
        step: clamp_marker_step(step),
        value: clamp_auto_value(lane, value, Automation::neutral(lane)),
    };
    let lane = t
        .automation
        .get_or_insert_with(Automation::default)
        .lane_mut(lane);
    if let Some(i) = lane.iter().position(|p| p.step == point.step) {
        lane[i].value = point.value;
        return Some(i);
    }
    if lane.len() >= MAX_AUTOMATION_POINTS {
        return None;
    }
    let i = lane.partition_point(|p| p.step < point.step);
    lane.insert(i, point);
    Some(i)
}

/// Moves the point at `index` on `track`'s `lane` to (`step`, `value`). The
/// lane re-sorts when the new step crosses a neighbour, so the index the
/// point lands at is returned — a caller dragging the point keeps following
/// it through the resort. `None` when the track, the automation block or
/// `index` does not exist.
pub fn move_automation_point(
    project: &mut Project,
    track_id: u64,
    lane: AutomationLane,
    index: usize,
    step: f64,
    value: f32,
) -> Option<usize> {
    let auto = track_mut(project, track_id)?.automation.as_mut()?;
    let points = auto.lane_mut(lane);
    let old = *points.get(index)?;
    points[index] = AutomationPoint {
        step: clamp_marker_step(step),
        value: clamp_auto_value(lane, value, old.value),
    };
    // Bubble the displaced element back into order instead of re-sorting the
    // whole lane: the drag stays attached to the same point and the returned
    // index is where it actually sits after crossing its neighbours.
    let mut i = index;
    while i > 0 && points[i].step < points[i - 1].step {
        points.swap(i, i - 1);
        i -= 1;
    }
    while i + 1 < points.len() && points[i].step > points[i + 1].step {
        points.swap(i, i + 1);
        i += 1;
    }
    Some(i)
}

/// Removes the point at `index` from `track`'s `lane`. When both lanes end
/// up empty the automation block itself drops back to `None`, keeping saved
/// songs free of `"automation": {}` husks. `false` when nothing was removed.
pub fn delete_automation_point(
    project: &mut Project,
    track_id: u64,
    lane: AutomationLane,
    index: usize,
) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(auto) = t.automation.as_mut() else {
        return false;
    };
    if index >= auto.lane(lane).len() {
        return false;
    }
    auto.lane_mut(lane).remove(index);
    if auto.is_empty() {
        t.automation = None;
    }
    true
}

/// Drops every point on `track`'s `lane` (and the automation block once both
/// lanes are empty). `false` when the lane had no points to clear.
pub fn clear_automation_lane(project: &mut Project, track_id: u64, lane: AutomationLane) -> bool {
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(auto) = t.automation.as_mut() else {
        return false;
    };
    if auto.lane(lane).is_empty() {
        return false;
    }
    auto.lane_mut(lane).clear();
    if auto.is_empty() {
        t.automation = None;
    }
    true
}

/// The chord event sitting exactly on `step`, if one is there. Chord
/// positions are bar-aligned, so an exact match is the "chord on this bar"
/// lookup the strip uses before it writes.
pub fn chord_event_at_step(project: &Project, step: f64) -> Option<u64> {
    project
        .chord_events
        .iter()
        .find(|ev| ev.step == step)
        .map(|ev| ev.id)
}

/// Adds a chord label at `step` (clamped to `0..=MAX_PATTERN_STEPS`); `root`
/// is taken mod 12 like every pitch class. `None` result means the
/// [`MAX_CHORD_EVENTS`] cap is reached. The list stays sorted by step then id.
pub fn add_chord_event(
    project: &mut Project,
    step: f64,
    root: u8,
    quality: ChordQuality,
) -> Option<u64> {
    if project.chord_events.len() >= MAX_CHORD_EVENTS {
        return None;
    }
    let id = next_id();
    project.chord_events.push(ChordEvent {
        id,
        step: clamp_marker_step(step),
        root: root % 12,
        quality,
    });
    sort_chord_events(project);
    Some(id)
}

/// Moves a chord event to `step` (clamped like [`add_chord_event`]) and
/// re-sorts the list. `false` if no event has that id.
pub fn move_chord_event(project: &mut Project, event_id: u64, step: f64) -> bool {
    let Some(ev) = project.chord_events.iter_mut().find(|e| e.id == event_id) else {
        return false;
    };
    ev.step = clamp_marker_step(step);
    sort_chord_events(project);
    true
}

/// Replaces a chord event's root and quality. `false` if the id is unknown.
pub fn set_chord_event(
    project: &mut Project,
    event_id: u64,
    root: u8,
    quality: ChordQuality,
) -> bool {
    let Some(ev) = project.chord_events.iter_mut().find(|e| e.id == event_id) else {
        return false;
    };
    ev.root = root % 12;
    ev.quality = quality;
    true
}

/// Steps a chord event's quality `delta` slots through [`ChordQuality::ALL`],
/// wrapping at both ends — the click-cycling the strip chips do. `false` if
/// the id is unknown.
pub fn cycle_chord_event_quality(project: &mut Project, event_id: u64, delta: i32) -> bool {
    let Some(ev) = project.chord_events.iter_mut().find(|e| e.id == event_id) else {
        return false;
    };
    let n = ChordQuality::ALL.len() as i32;
    let i = ChordQuality::ALL
        .iter()
        .position(|&q| q == ev.quality)
        .unwrap_or(0) as i32;
    ev.quality = ChordQuality::ALL[(i + delta).rem_euclid(n) as usize];
    true
}

/// Steps a chord event's root `delta` semitones, wrapping at the octave.
/// `false` if the id is unknown.
pub fn cycle_chord_event_root(project: &mut Project, event_id: u64, delta: i32) -> bool {
    let Some(ev) = project.chord_events.iter_mut().find(|e| e.id == event_id) else {
        return false;
    };
    ev.root = ((ev.root as i32 + delta).rem_euclid(12)) as u8;
    true
}

/// Deletes a chord event. `false` if the id is unknown.
pub fn delete_chord_event(project: &mut Project, event_id: u64) -> bool {
    let before = project.chord_events.len();
    project.chord_events.retain(|e| e.id != event_id);
    project.chord_events.len() != before
}

/// Chord symbol for a strip chip — "C", "Am7", "F#m7b5". Thin wrapper over
/// [`ChordQuality::symbol`] so call sites never re-derive the root/suffix.
/// This is the key-free (sharp) spelling; the arranger's chord strip shows
/// [`ChordEvent::label_in`], spelled for the song key.
#[allow(dead_code)]
pub fn chord_event_label(ev: &ChordEvent) -> String {
    ev.label()
}

/// The key's name spelled the way its chords are: "Bb minor" rather than
/// "A# minor", "F# major". Display only — the song stores a pitch class.
pub fn key_label(key: SongKey) -> String {
    format!("{} {}", key.spell(key.root), key.mode.name())
}

/// Note name spelled for `key` ("Bb3" in C minor, "A#3" in E major).
/// Display only.
pub fn note_name_in(pitch: u8, key: SongKey) -> String {
    format!("{}{}", key.spell(pitch % 12), i32::from(pitch) / 12 - 1)
}

/// The chord a plain click on the strip writes: the key's tonic (I in a
/// major key, i in a minor one) — the first of [`diatonic_chords`].
pub fn tonic_chord_spec(key: SongKey) -> ChordSpec {
    diatonic_chords(key)[0].spec
}

fn sort_chord_events(project: &mut Project) {
    project
        .chord_events
        .sort_by(|a, b| a.step.total_cmp(&b.step).then(a.id.cmp(&b.id)));
}

/// Positions within this many steps of a clip edge or a repetition boundary
/// count as landing on it, so splits stay on whole steps/loops.
const SPLIT_EDGE_TOL: f64 = 1e-6;

/// Whether `at_step` falls strictly inside the clip's span — the only spot
/// where [`split_clip`] produces two non-empty pieces.
pub fn can_split_clip(project: &Project, track_id: u64, clip_id: u64, at_step: f64) -> bool {
    let Some(t) = track(project, track_id) else {
        return false;
    };
    let Some(clip) = t.clips.iter().find(|c| c.id == clip_id) else {
        return false;
    };
    let Some(pat) = pattern(t, clip.pattern_id) else {
        return false;
    };
    let s = at_step - clip.start;
    let span = clip.span(pat.length.max(1) as f64);
    // The new right clip starts at `at_step`, which `Project::validate`
    // bounds like any other clip start.
    s > SPLIT_EDGE_TOL && span - s > SPLIT_EDGE_TOL && at_step < MAX_PATTERN_STEPS as f64
}

/// Whole-step length for a split-off pattern: the piece's span rounded to an
/// integer (ceil when it is not essentially whole), but never below the
/// smallest length that still covers every note start — `Project::events`
/// only plays notes with `start < length`. A ceil adds under one step of
/// silent overhang; clip spans are whole numbers of steps, so a fractional
/// cut can never land exactly on both edges.
fn split_piece_len(span: f64, max_note_start: f64) -> u32 {
    let rounded = span.round();
    let base = if (span - rounded).abs() <= SPLIT_EDGE_TOL {
        rounded
    } else {
        span.ceil()
    };
    base.max(max_note_start.floor() + 1.0).max(1.0) as u32
}

/// Cuts the clip at `at_step` (song steps) into two adjacent clips. The
/// left piece keeps `clip_id` and the clip's `fade_in`; the new right clip
/// — inserted immediately after it in `track.clips` — starts at the split
/// point with a fresh id and the clip's `fade_out`.
///
/// A clip is only a reference to whole pattern repetitions: it cannot
/// express "start halfway through a pattern". A cut landing on a repetition
/// boundary therefore needs no new data — the left clip just plays fewer
/// loops and the right clip the remaining ones of the same pattern. A cut
/// inside a repetition instead flattens each half's played notes into a new
/// single-play pattern (named "… split"): notes crossing the cut are
/// shortened on the left and continue from step 0 on the right, so the two
/// pieces sound exactly like the original clip.
///
/// `false` — leaving `project` untouched — when the track, clip or pattern
/// is missing, `at_step` is not strictly inside the clip's span, or the two
/// new patterns would not fit the per-track pattern, note or length limits.
pub fn split_clip(project: &mut Project, track_id: u64, clip_id: u64, at_step: f64) -> bool {
    // `Project::validate` counts every clip's notes times loops against
    // `MAX_SONG_EVENTS`; a note crossing the cut is counted in both halves,
    // so the check below needs the project's current theoretical total.
    let events_now: usize = project
        .tracks
        .iter()
        .flat_map(|t| {
            t.clips.iter().map(move |c| {
                t.patterns
                    .iter()
                    .find(|p| p.id == c.pattern_id)
                    .map_or(0, |p| p.notes.len())
                    * c.loop_count() as usize
            })
        })
        .sum();
    let Some(t) = track_mut(project, track_id) else {
        return false;
    };
    let Some(ci) = t.clips.iter().position(|c| c.id == clip_id) else {
        return false;
    };
    if t.clips.len() >= MAX_CLIPS_PER_TRACK {
        return false;
    }
    let clip = t.clips[ci].clone();
    let Some(pat_len) = t
        .patterns
        .iter()
        .find(|p| p.id == clip.pattern_id)
        .map(|p| p.length.max(1) as f64)
    else {
        return false;
    };
    let loops = clip.loop_count();
    let span = pat_len * loops as f64;
    let s = at_step - clip.start;
    // Strict interior only, and the right clip's start must stay a legal
    // clip position (`validate` rejects starts at or past the song limit).
    if !s.is_finite()
        || s <= SPLIT_EDGE_TOL
        || span - s <= SPLIT_EDGE_TOL
        || at_step >= MAX_PATTERN_STEPS as f64
    {
        return false;
    }
    // A cut (essentially) on a repetition boundary: the same pattern plays
    // `m` loops on the left and the remaining `loops - m` on the right.
    let reps = s / pat_len;
    let m = reps.round() as u32;
    if m >= 1 && m < loops && (reps - f64::from(m)).abs() * pat_len <= SPLIT_EDGE_TOL {
        let lspan = pat_len * f64::from(m);
        if clip.start + lspan >= MAX_PATTERN_STEPS as f64 {
            return false;
        }
        let left = &mut t.clips[ci];
        left.loops = m;
        left.fade_in = clean_fade(left.fade_in, lspan);
        left.fade_out = 0.0;
        t.clips.insert(
            ci + 1,
            Clip {
                id: next_id(),
                pattern_id: clip.pattern_id,
                start: clip.start + lspan,
                loops: loops - m,
                fade_in: 0.0,
                fade_out: clean_fade(clip.fade_out, span - lspan),
            },
        );
        return true;
    }
    // A cut inside a repetition: flatten the notes the clip actually plays —
    // tiled per loop and clipped to the pattern edge the way
    // `Project::events` clips them — and deal the two halves out into fresh
    // single-play patterns so each piece sounds like the original.
    let mut pat_notes = 0usize;
    let mut left_notes: Vec<(u8, f64, f64, f32)> = Vec::new();
    let mut right_notes: Vec<(u8, f64, f64, f32)> = Vec::new();
    if let Some(pat) = t.patterns.iter().find(|p| p.id == clip.pattern_id) {
        pat_notes = pat.notes.len();
        for rep in 0..loops {
            let off = rep as f64 * pat_len;
            for n in &pat.notes {
                if !(n.start >= 0.0 && n.start < pat_len) || !n.duration.is_finite() {
                    continue;
                }
                let pos = off + n.start;
                // `Project::events` clips to the pattern edge and drops
                // anything left at zero length — match that here.
                let d = n.duration.min(pat_len - n.start).max(0.0);
                if d <= 0.0 {
                    continue;
                }
                let end = pos + d;
                // Notes starting before the cut belong to the left piece,
                // shortened at the cut when they hang over it.
                if pos < s {
                    left_notes.push((n.pitch.min(127), pos, (s - pos).min(end - pos), n.velocity));
                }
                // Notes still sounding at the cut belong to the right piece,
                // shifted so the cut becomes step 0; a hanging note resumes
                // from 0 with just its remaining tail.
                if end > s {
                    let start = pos.max(s) - s;
                    right_notes.push((n.pitch.min(127), start, end - s - start, n.velocity));
                }
            }
        }
    }
    if left_notes.len() > MAX_NOTES_PER_PATTERN
        || right_notes.len() > MAX_NOTES_PER_PATTERN
        || t.patterns.len() + 2 > MAX_PATTERNS_PER_TRACK
        || events_now.saturating_sub(pat_notes * loops as usize)
            + left_notes.len()
            + right_notes.len()
            > MAX_SONG_EVENTS
    {
        return false;
    }
    let len_l = split_piece_len(s, left_notes.iter().map(|n| n.1).fold(0.0, f64::max));
    let len_r = split_piece_len(
        span - s,
        right_notes.iter().map(|n| n.1).fold(0.0, f64::max),
    );
    if len_l > MAX_PATTERN_STEPS || len_r > MAX_PATTERN_STEPS {
        return false;
    }
    // Names keep the source's prefix plus " split" (truncated so a
    // max-length source name still validates); `unique_name` disambiguates.
    let base: String = t
        .patterns
        .iter()
        .find(|p| p.id == clip.pattern_id)
        .map(|p| {
            let head: String = p.name.chars().take(MAX_NAME_CHARS - 6).collect();
            format!("{head} split")
        })
        .unwrap_or_else(|| "Split".to_string());
    // The disambiguation suffix could re-exceed the cap; clip it again.
    let name_l: String = unique_name(&base, t.patterns.iter().map(|p| p.name.clone()))
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let name_r: String = unique_name(
        &base,
        t.patterns
            .iter()
            .map(|p| p.name.clone())
            .chain(std::iter::once(name_l.clone())),
    )
    .chars()
    .take(MAX_NAME_CHARS)
    .collect();
    let build = |name: String, length: u32, notes: Vec<(u8, f64, f64, f32)>| {
        let mut pat = Pattern {
            id: next_id(),
            name,
            length,
            notes: notes
                .into_iter()
                .map(|(pitch, start, duration, velocity)| Note {
                    id: next_id(),
                    pitch,
                    start,
                    // A sliver left by the cut clamps up to the minimum note
                    // length — dropping it would lose the attack.
                    duration: duration.clamp(MIN_NOTE_DURATION, MAX_PATTERN_STEPS as f64),
                    velocity: if velocity.is_finite() {
                        velocity.clamp(0.0, 1.0)
                    } else {
                        0.0
                    },
                })
                .collect(),
        };
        pat.sort_notes();
        pat
    };
    let left_pat = build(name_l, len_l, left_notes);
    let right_pat = build(name_r, len_r, right_notes);
    let (left_pid, right_pid) = (left_pat.id, right_pat.id);
    t.patterns.push(left_pat);
    t.patterns.push(right_pat);
    {
        let left = &mut t.clips[ci];
        left.pattern_id = left_pid;
        left.loops = 1;
        left.fade_in = clean_fade(left.fade_in, len_l as f64);
        left.fade_out = 0.0;
    }
    t.clips.insert(
        ci + 1,
        Clip {
            id: next_id(),
            pattern_id: right_pid,
            start: at_step,
            loops: 1,
            fade_in: 0.0,
            fade_out: clean_fade(clip.fade_out, len_r as f64),
        },
    );
    fit_bars_to_content(project);
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

/// Places up to `times` copies of an imported-WAV clip back-to-back after
/// it — [`repeat_clip`]'s audio counterpart, capped by
/// [`MAX_AUDIO_CLIPS_PER_TRACK`].
pub fn repeat_audio_clip(
    project: &mut Project,
    track_id: u64,
    clip_id: u64,
    times: usize,
) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    let clip = audio_clip(t, clip_id)?.clone();
    let len = clip.span();
    let mut last = None;
    for i in 1..=times {
        let start = clip.start + len * i as f64;
        if start + len > MAX_SONG_STEPS + 1e-9 || t.audio_clips.len() >= MAX_AUDIO_CLIPS_PER_TRACK {
            break;
        }
        let id = next_id();
        let mut copy = clip.clone();
        copy.id = id;
        copy.start = start;
        t.audio_clips.push(copy);
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

/// Bounds-clamps a group move delta: the group's leftmost clip start must
/// stay `>= 0` and its rightmost end inside [`MAX_SONG_STEPS`].
fn bounded_group_delta(min_start: f64, max_end: f64, dstep: f64) -> f64 {
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

pub fn clip_group_delta(project: &Project, originals: &[(u64, Clip)], dstep: f64) -> f64 {
    mixed_group_delta(project, originals, &[], dstep)
}

/// The drag-delta clamp for a selection that may mix pattern clips and
/// imported-WAV clips — the same bounds as [`clip_group_delta`] over both
/// lists' combined extents.
pub fn mixed_group_delta(
    project: &Project,
    originals: &[(u64, Clip)],
    audio: &[(u64, AudioClip)],
    dstep: f64,
) -> f64 {
    if originals.is_empty() && audio.is_empty() {
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
    for (_, clip) in audio {
        min_start = min_start.min(clip.start);
        max_end = max_end.max(clip.start + clip.span());
    }
    bounded_group_delta(min_start, max_end, dstep)
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

/// Snapshots of the selected imported-WAV clips, `(track_id, clip)` like
/// [`collect_clips`].
pub fn collect_audio_clips(project: &Project, ids: &BTreeSet<u64>) -> Vec<(u64, AudioClip)> {
    project
        .tracks
        .iter()
        .flat_map(|t| {
            t.audio_clips
                .iter()
                .filter(|c| ids.contains(&c.id))
                .cloned()
                .map(|c| (t.id, c))
        })
        .collect()
}

/// Moves a group of imported-WAV clips by `dstep` (pre-clamped by the
/// caller through [`mixed_group_delta`]) — [`move_clip_group`]'s audio
/// counterpart.
pub fn move_audio_clip_group(
    project: &mut Project,
    originals: &[(u64, AudioClip)],
    dstep: f64,
) -> bool {
    if let [(track_id, original)] = originals {
        return move_audio_clip(project, *track_id, original.id, original.start + dstep);
    }
    let mut changed = false;
    for (track_id, original) in originals {
        if let Some(t) = track_mut(project, *track_id) {
            if let Some(c) = t.audio_clips.iter_mut().find(|c| c.id == original.id) {
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

/// Copies a group of imported-WAV clips by `dstep` — [`copy_clip_group`]'s
/// audio counterpart, capped at [`MAX_AUDIO_CLIPS_PER_TRACK`] per track.
/// Clip references (the staged `file` names) carry over unchanged.
pub fn copy_audio_clip_group(
    project: &mut Project,
    originals: &[(u64, AudioClip)],
    dstep: f64,
) -> Option<Vec<u64>> {
    if originals.is_empty() {
        return None;
    }
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
        if t.audio_clips.len() + *n > MAX_AUDIO_CLIPS_PER_TRACK {
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
        t.audio_clips.push(clip);
        ids.push(id);
    }
    fit_bars_to_content(project);
    Some(ids)
}

/// Copies a mixed pattern/audio clip selection by `dstep` as one operation
/// (Alt-drag, paste and duplicate all route here). Every target track's
/// clip caps are checked before anything is pushed, so a failed copy never
/// leaves half a group behind. Returns the new ids in copy order (pattern
/// clips first, then audio clips).
pub fn copy_mixed_clip_group(
    project: &mut Project,
    originals: &[(u64, Clip)],
    audio: &[(u64, AudioClip)],
    dstep: f64,
) -> Option<Vec<u64>> {
    if originals.is_empty() {
        return copy_audio_clip_group(project, audio, dstep);
    }
    if audio.is_empty() {
        return copy_clip_group(project, originals, dstep);
    }
    let dstep = mixed_group_delta(project, originals, audio, dstep);
    // Pre-check every track against both caps so the two pushes below are
    // guaranteed to succeed together.
    for track_id in originals
        .iter()
        .map(|(id, _)| *id)
        .chain(audio.iter().map(|(id, _)| *id))
    {
        let t = track(project, track_id)?;
        let want_pat = originals.iter().filter(|(id, _)| *id == track_id).count();
        let want_aud = audio.iter().filter(|(id, _)| *id == track_id).count();
        if t.clips.len() + want_pat > MAX_CLIPS_PER_TRACK
            || t.audio_clips.len() + want_aud > MAX_AUDIO_CLIPS_PER_TRACK
        {
            return None;
        }
    }
    let mut ids = Vec::with_capacity(originals.len() + audio.len());
    for (track_id, original) in originals {
        let t = track_mut(project, *track_id)?;
        let id = next_id();
        let mut clip = original.clone();
        clip.id = id;
        clip.start = original.start + dstep;
        t.clips.push(clip);
        ids.push(id);
    }
    for (track_id, original) in audio {
        let t = track_mut(project, *track_id)?;
        let id = next_id();
        let mut clip = original.clone();
        clip.id = id;
        clip.start = original.start + dstep;
        t.audio_clips.push(clip);
        ids.push(id);
    }
    fit_bars_to_content(project);
    Some(ids)
}

/// Duplicates a mixed pattern/audio selection right after itself —
/// [`duplicate_clip_group_after`]'s mixed counterpart.
pub fn duplicate_clip_groups_after(
    project: &mut Project,
    originals: &[(u64, Clip)],
    audio: &[(u64, AudioClip)],
) -> Option<Vec<u64>> {
    if audio.is_empty() {
        return duplicate_clip_group_after(project, originals);
    }
    if originals.is_empty() && audio.is_empty() {
        return None;
    }
    let start = originals
        .iter()
        .map(|(_, c)| c.start)
        .chain(audio.iter().map(|(_, c)| c.start))
        .fold(f64::INFINITY, f64::min);
    let end = originals
        .iter()
        .filter_map(|(track_id, clip)| {
            track(project, *track_id).map(|t| clip.start + clip_len(t, clip))
        })
        .chain(audio.iter().map(|(_, c)| c.start + c.span()))
        .fold(f64::NEG_INFINITY, f64::max);
    if !start.is_finite() || !end.is_finite() {
        return None;
    }
    copy_mixed_clip_group(project, originals, audio, (end - start).max(1.0))
}

/// Gives a clip its own copy of its pattern so edits don't affect other clips.
pub fn make_clip_unique(project: &mut Project, track_id: u64, clip_id: u64) -> Option<u64> {
    let t = track_mut(project, track_id)?;
    let pattern_id = t.clips.iter().find(|c| c.id == clip_id)?.pattern_id;
    let new_id = duplicate_pattern_in(t, pattern_id)?;
    t.clips.iter_mut().find(|c| c.id == clip_id)?.pattern_id = new_id;
    Some(new_id)
}

/// Creates an empty pattern. `None` on audio lanes (their clips schedule
/// `audio_clips`, so a pattern could never play) and at the per-track
/// pattern limit.
pub fn new_pattern(track: &mut Track, length: u32) -> Option<u64> {
    if track.audio || track.patterns.len() >= MAX_PATTERNS_PER_TRACK {
        return None;
    }
    let mut p = create_pattern(track.instrument, length.clamp(1, MAX_PATTERN_STEPS));
    p.notes.clear();
    let base = if track.shows_drum_grid() {
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

// ----- arpeggiator -----------------------------------------------------------

/// Accent added to the first note of every bar when [`ArpSpec::accent`] is set.
pub const ARP_ACCENT: f32 = 0.15;
/// Fraction of the rate each arpeggio note sounds for (leaves a small gap).
pub const ARP_GATE: f64 = 0.9;

/// Everything needed to render one arpeggio into a pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArpSpec {
    /// MIDI pitch of the lowest chord tone.
    pub chord_root: u8,
    pub quality: ChordQuality,
    /// How many octaves the chord tones repeat across, 1..=[`MAX_ARP_OCTAVES`].
    pub octaves: u32,
    pub direction: ArpDirection,
    /// Steps between notes (1 = sixteenths, 2 = eighths, 4 = quarters).
    pub rate_steps: u32,
    pub velocity: f32,
    /// Adds [`ARP_ACCENT`] to notes that start on a bar line (every 16 steps).
    pub accent: bool,
    /// Clear the pattern's existing notes first (otherwise append).
    pub replace: bool,
}

impl Default for ArpSpec {
    fn default() -> Self {
        ArpSpec {
            chord_root: 60,
            quality: ChordQuality::Major,
            octaves: 2,
            direction: ArpDirection::Up,
            rate_steps: 2,
            velocity: 0.8,
            accent: true,
            replace: true,
        }
    }
}

/// The ascending chord tones of `spec`: the chord's intervals voiced upward
/// from `chord_root`, repeated `octaves` times an octave apart. Tones above
/// [`MAX_PITCH`] are dropped (never folded down, which would scramble the
/// shape); duplicates are removed.
pub fn arp_tones(spec: &ArpSpec) -> Vec<u8> {
    let mut tones: Vec<u8> = (0..spec.octaves.min(MAX_ARP_OCTAVES))
        .flat_map(|o| {
            spec.quality
                .intervals()
                .iter()
                .map(move |&iv| spec.chord_root as u32 + iv as u32 + 12 * o)
        })
        .filter(|&p| p <= MAX_PITCH as u32)
        .map(|p| p as u8)
        .collect();
    tones.sort_unstable();
    tones.dedup();
    tones
}

/// xorshift64* step; deterministic so arpeggios (and tests) are repeatable.
fn arp_rand(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Uniform f64 in `[0, 1)` drawn from the xorshift stream (top 53 bits).
#[allow(dead_code)] // used by humanize_notes, which awaits its GUI caller
fn rand_f64(state: &mut u64) -> f64 {
    (arp_rand(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// Fills `p` with arpeggio notes per `spec`, one hit every `rate_steps` from
/// step 0 to the end of the pattern. Each note lasts `rate_steps * 0.9`
/// (clamped at the pattern end by [`add_note`]). Returns the number of notes
/// written; 0 (with the pattern untouched) when the spec is invalid — zero
/// octaves or rate, or no chord tone within MIDI range. Stops cleanly at
/// [`MAX_NOTES_PER_PATTERN`].
pub fn generate_arpeggio(p: &mut Pattern, spec: &ArpSpec) -> usize {
    let tones = arp_tones(spec);
    if tones.is_empty() || spec.octaves == 0 || spec.rate_steps == 0 {
        return 0;
    }
    if spec.replace {
        p.notes.clear();
    }
    let order: Vec<u8> = match spec.direction {
        ArpDirection::Up | ArpDirection::Random | ArpDirection::Block => tones.clone(),
        ArpDirection::Down => tones.iter().rev().copied().collect(),
        ArpDirection::UpDown => {
            let mut v = tones.clone();
            if tones.len() > 2 {
                v.extend(tones[1..tones.len() - 1].iter().rev());
            }
            v
        }
    };
    let rate = spec.rate_steps as f64;
    let duration = rate * ARP_GATE;
    let base_velocity = spec.velocity.clamp(0.0, 1.0);
    let mut seed = p.id.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut last_pick = usize::MAX;
    let mut written = 0;
    let mut step = 0u32;
    let mut index = 0usize;
    while step < p.length {
        let start = step as f64;
        let velocity = if spec.accent && step.is_multiple_of(STEPS_PER_BAR) {
            (base_velocity + ARP_ACCENT).min(1.0)
        } else {
            base_velocity
        };
        let hit: Vec<u8> = match spec.direction {
            ArpDirection::Block => tones.clone(),
            ArpDirection::Random => {
                let mut pick = (arp_rand(&mut seed) % tones.len() as u64) as usize;
                if pick == last_pick && tones.len() > 1 {
                    pick = (pick + 1) % tones.len();
                }
                last_pick = pick;
                vec![tones[pick]]
            }
            _ => vec![order[index % order.len()]],
        };
        for pitch in hit {
            if add_note(p, pitch, start, duration, velocity).is_none() {
                return written;
            }
            written += 1;
        }
        index += 1;
        step += spec.rate_steps;
    }
    written
}

/// Whether the per-track live arpeggiator ([`crate::model::Track::arp`]) can
/// sound on `track`. The audio engine applies it to the track's primary
/// sound whenever it is not drums (`!Sound::is_drums()`): a valid SoundFont
/// always arpeggiates (its GM program is melodic), a kit preset never does,
/// and otherwise the built-in instrument decides — exactly the precedence
/// `TrackParams::from_track` uses, mirrored here so inspector hints match
/// playback.
pub fn arp_audible(track: &Track) -> bool {
    if track
        .font
        .as_ref()
        .is_some_and(|f| !f.name.trim().is_empty())
    {
        return true;
    }
    match &track.preset {
        Some(p) => p.kit.is_none(),
        None => !track.instrument.is_drums(),
    }
}

// ----- chords & strums -------------------------------------------------------
// The chord/strum/humanize API below is `allow(dead_code)` until the editor
// wires it in — same pattern as `EditorRects` in editor.rs.

/// A chord as a pitch-class root plus a [`ChordQuality`] — C major is
/// `ChordSpec::new(0, ChordQuality::Major)`, A minor 7 is `new(9, Minor7)`.
/// The interval tables live on [`ChordQuality`], shared with the arpeggiator.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChordSpec {
    /// Root pitch class, 0..=11 (C = 0); [`ChordSpec::new`] reduces mod 12.
    pub root: u8,
    pub quality: ChordQuality,
}

#[allow(dead_code)]
impl ChordSpec {
    /// `root` may be any pitch; only its pitch class is kept.
    pub fn new(root: u8, quality: ChordQuality) -> Self {
        ChordSpec {
            root: root % 12,
            quality,
        }
    }

    /// Chord symbol like "C", "Am7" or "F#dim" — see [`ChordQuality::symbol`].
    pub fn symbol(self) -> String {
        self.quality.symbol(self.root)
    }

    /// The symbol spelled for `key` ("Ab" in C minor, "G#" in E major) —
    /// what the UI shows. Display only.
    pub fn symbol_in(self, key: SongKey) -> String {
        self.quality.symbol_in(self.root, key)
    }

    /// The chord's pitch classes as a sorted set: intervals are taken mod 12,
    /// so the 9th of an add9 lands on the same class as the 2nd.
    pub fn pitch_classes(self) -> Vec<u8> {
        let mut pcs: Vec<u8> = self
            .quality
            .intervals()
            .iter()
            .map(|&iv| ((self.root as u16 + iv as u16) % 12) as u8)
            .collect();
        pcs.sort_unstable();
        pcs.dedup();
        pcs
    }
}

/// One of the seven diatonic chords of a [`SongKey`]: the 0-based scale
/// `degree` it sits on and the [`ChordSpec`] built there. The roman numeral
/// is [`SongKey::degree_numeral`].
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiatonicChord {
    pub degree: usize,
    pub spec: ChordSpec,
}

/// The seven diatonic chords of `key`, degrees 0..=6 (I–vii). Quality comes
/// from the key's mode triads and the root from its scale degrees, so the
/// result always matches what the arpeggiator dialog offers for the key.
#[allow(dead_code)]
pub fn diatonic_chords(key: SongKey) -> [DiatonicChord; 7] {
    std::array::from_fn(|degree| DiatonicChord {
        degree,
        spec: ChordSpec::new(key.degree_root(degree), key.degree_chord_quality(degree)),
    })
}

/// Pitch-class names with ASCII flats, indexed by `pitch % 12` (C = 0).
/// Borrowed chords imply flat spelling ("A♭" is written "Ab"), so the
/// palette spells its symbols with these names instead of [`NOTE_NAMES`].
const NOTE_NAMES_FLAT: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

/// One entry of the chord strip's borrowed-chord palette: the roman-numeral
/// role it plays, the [`ChordSpec`] it stamps and the flat-spelled symbol
/// shown on its chip ("Ab" for ♭VI in C major — the same chord as "G#").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BorrowedChord {
    /// Roman-numeral role: "iv", "bVII", "V/vi" or "N6" (the Neapolitan).
    pub label: &'static str,
    /// The chord to stamp.
    pub spec: ChordSpec,
    /// Chord symbol with flat spelling, matching `label`'s role.
    pub symbol: String,
}

impl BorrowedChord {
    /// A palette entry: `label` role on pitch class `root` with `quality`;
    /// the symbol uses flat spelling.
    fn new(label: &'static str, root: u8, quality: ChordQuality) -> Self {
        let root = root % 12;
        BorrowedChord {
            label,
            spec: ChordSpec::new(root, quality),
            symbol: format!("{}{}", NOTE_NAMES_FLAT[root as usize], quality.suffix()),
        }
    }
}

/// The curated borrowed-chords / secondary-dominants palette for `key`, the
/// chord strip's "Borrowed" tab: colour chords from the parallel mode plus
/// the most common secondary dominants, each spelled flat-style per its
/// role. In a major key the borrows come from the parallel minor — the
/// minor-plagal iv, bVII, bVI and bIII, plus bII as the Neapolitan (N6) —
/// and the V/vi, V/ii and V/IV secondary dominants. In a minor key they
/// come from the parallel major (IV, N6) plus the harmonic-minor dominant
/// V and the V/III, V/iv and V/VI secondary dominants.
pub fn borrowed_chords(key: SongKey) -> Vec<BorrowedChord> {
    let r = i32::from(key.root % 12);
    let pc = |semi: i32| (r + semi).rem_euclid(12) as u8;
    match key.mode {
        KeyMode::Major => vec![
            BorrowedChord::new("iv", pc(5), ChordQuality::Minor),
            BorrowedChord::new("bVII", pc(10), ChordQuality::Major),
            BorrowedChord::new("bVI", pc(8), ChordQuality::Major),
            BorrowedChord::new("bIII", pc(3), ChordQuality::Major),
            BorrowedChord::new("N6", pc(1), ChordQuality::Major),
            BorrowedChord::new("V/vi", pc(4), ChordQuality::Dominant7),
            BorrowedChord::new("V/ii", pc(9), ChordQuality::Dominant7),
            BorrowedChord::new("V/IV", pc(0), ChordQuality::Dominant7),
        ],
        KeyMode::Minor => vec![
            BorrowedChord::new("IV", pc(5), ChordQuality::Major),
            BorrowedChord::new("N6", pc(1), ChordQuality::Major),
            BorrowedChord::new("V", pc(7), ChordQuality::Dominant7),
            BorrowedChord::new("V/III", pc(10), ChordQuality::Dominant7),
            BorrowedChord::new("V/iv", pc(0), ChordQuality::Dominant7),
            BorrowedChord::new("V/VI", pc(3), ChordQuality::Dominant7),
        ],
    }
}

/// A one-click chord progression preset: a named, genre/mood-tagged loop of
/// chords expressed as scale degrees of the song key, so the same preset
/// works in any key. Stamps one chord per bar starting at the playhead or
/// pointer (see [`stamp_progression`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressionPreset {
    /// Short chip label ("Pop", "Blues").
    pub name: &'static str,
    /// One-line description shown on hover.
    pub hint: &'static str,
    /// Scale degrees (0-based, 0 = tonic), one chord per step.
    pub degrees: &'static [u8],
    /// Quality per step; `None` keeps the key's diatonic quality on that
    /// degree. Must match `degrees` in length.
    pub qualities: &'static [Option<ChordQuality>],
}

/// The one-click progression presets shown on the chord strip's
/// Progressions tab, in display order.
pub const PROGRESSIONS: [ProgressionPreset; 7] = [
    ProgressionPreset {
        name: "Pop",
        hint: "Bright radio-pop loop: I-V-vi-IV.",
        degrees: &[0, 4, 5, 3],
        qualities: &[
            Some(ChordQuality::Major),
            Some(ChordQuality::Major),
            Some(ChordQuality::Minor),
            Some(ChordQuality::Major),
        ],
    },
    ProgressionPreset {
        name: "Rock",
        hint: "Driving three-chord rock: I-IV-V.",
        degrees: &[0, 3, 4],
        qualities: &[Some(ChordQuality::Major); 3],
    },
    ProgressionPreset {
        name: "Jazz",
        hint: "ii-V-I turnaround with sevenths.",
        degrees: &[1, 4, 0],
        qualities: &[
            Some(ChordQuality::Minor7),
            Some(ChordQuality::Dominant7),
            Some(ChordQuality::Major7),
        ],
    },
    ProgressionPreset {
        name: "Minor epic",
        hint: "Cinematic climb: i-VI-III-VII.",
        degrees: &[0, 5, 2, 6],
        qualities: &[
            Some(ChordQuality::Minor),
            Some(ChordQuality::Major),
            Some(ChordQuality::Major),
            Some(ChordQuality::Major),
        ],
    },
    ProgressionPreset {
        name: "Classic",
        hint: "The doo-wop loop: I-vi-IV-V.",
        degrees: &[0, 5, 3, 4],
        qualities: &[
            Some(ChordQuality::Major),
            Some(ChordQuality::Minor),
            Some(ChordQuality::Major),
            Some(ChordQuality::Major),
        ],
    },
    ProgressionPreset {
        name: "Blues",
        hint: "Twelve-bar blues, all dominant sevenths.",
        degrees: &[0, 0, 0, 0, 3, 3, 0, 0, 4, 3, 0, 4],
        qualities: &[Some(ChordQuality::Dominant7); 12],
    },
    ProgressionPreset {
        name: "Lo-fi",
        hint: "Dreamy major-seven loop: Imaj7-vi7-ii7-V7.",
        degrees: &[0, 5, 1, 4],
        qualities: &[
            Some(ChordQuality::Major7),
            Some(ChordQuality::Minor7),
            Some(ChordQuality::Minor7),
            Some(ChordQuality::Dominant7),
        ],
    },
];

/// Resolves a [`ProgressionPreset`] against `key`: one [`ChordSpec`] per
/// step. `override_quality` (the chord strip's Type combo) replaces every
/// step's quality when set; otherwise the preset's own quality wins, with
/// the key's diatonic quality as the fallback for `None` steps.
pub fn progression_specs(
    key: SongKey,
    preset: &ProgressionPreset,
    override_quality: Option<ChordQuality>,
) -> Vec<ChordSpec> {
    preset
        .degrees
        .iter()
        .zip(preset.qualities.iter())
        .map(|(&degree, &quality)| {
            let quality = override_quality
                .or(quality)
                .unwrap_or_else(|| key.degree_chord_quality(degree as usize));
            ChordSpec::new(key.degree_root(degree as usize), quality)
        })
        .collect()
}

/// Snaps `pitch` to the nearest MIDI pitch whose pitch class belongs to
/// `key`'s scale — its seven diatonic degrees — for the editor's
/// snap-to-scale drawing and note drags. In-scale pitches return unchanged.
/// The search widens outward semitone by semitone and checks the pitch below
/// before the one above, so an out-of-scale pitch exactly between two scale
/// tones snaps *down*. (In major/natural-minor every chromatic pitch is such
/// a tie, so the snap is always one semitone — never a leap.) Pitches at the
/// MIDI extremes snap inward: the unreachable side is skipped, so a low
/// out-of-scale pitch moves up instead of disappearing below pitch 0.
pub fn snap_pitch_to_scale(key: SongKey, pitch: u8) -> u8 {
    let pcs = key.pitch_classes();
    let pitch = i32::from(pitch.min(MAX_PITCH));
    for d in 0..=6 {
        let down = pitch - d;
        if down >= 0 && pcs[down as usize % 12] {
            return down as u8;
        }
        let up = pitch + d;
        if up <= i32::from(MAX_PITCH) && pcs[up as usize % 12] {
            return up as u8;
        }
    }
    // Unreachable for a heptatonic scale (every 12-pitch window holds all
    // seven scale tones) — return the input rather than panic.
    pitch as u8
}

/// A chord tone before voicing: a pitch class, the octave it prefers and its
/// velocity. Octaves use the arpeggiator dialog's numbering — MIDI pitch =
/// `(octave + 1) * 12 + pitch_class`, so octave 4 with pitch class 0 is C4 =
/// MIDI 60. Build lists with [`chord_notes`], resolve to pitches with
/// [`chord_midi_pitches`].
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChordNote {
    /// Pitch class 0..=11 (reduced mod 12 when voiced).
    pub pitch_class: u8,
    /// Preferred octave (see above); only a starting point — the chord is
    /// re-centred on a target pitch by [`chord_midi_pitches`].
    pub octave: i32,
    /// 0..=1, carried through for stamping.
    pub velocity: f32,
}

/// Expands `spec` into [`ChordNote`]s in `octave`, all at `velocity`, in
/// ascending interval order. Intervals voice upward: the 9th of an add9
/// prefers the octave above the root's rather than folding below it.
#[allow(dead_code)]
pub fn chord_notes(spec: &ChordSpec, octave: i32, velocity: f32) -> Vec<ChordNote> {
    spec.quality
        .intervals()
        .iter()
        .map(|&iv| {
            let semi = spec.root as i32 + iv as i32;
            ChordNote {
                pitch_class: semi.rem_euclid(12) as u8,
                octave: octave + semi.div_euclid(12),
                velocity,
            }
        })
        .collect()
}

/// Voices `tones` as concrete MIDI pitches centred on `center` (a MIDI pitch,
/// e.g. 60 = middle C). The tones' preferred-octave pitches are computed
/// first, then the whole chord is shifted by whole octaves — never re-ordered
/// — so its middle pitch lands nearest `center`. Shifting by octaves keeps the
/// voicing (inversion and spacing) intact, which is what makes it
/// voice-leading friendly: the same chord re-voices consistently around
/// whatever register the caller centres on. The result is sorted ascending,
/// deduplicated and clamped to `0..=MAX_PITCH`; empty input gives an empty vec.
#[allow(dead_code)]
pub fn chord_midi_pitches(tones: &[ChordNote], center: u8) -> Vec<u8> {
    if tones.is_empty() {
        return Vec::new();
    }
    // i64 so absurd `octave` values can't overflow before the final clamp.
    let mut raw: Vec<i64> = tones
        .iter()
        .map(|t| (t.octave as i64 + 1) * 12 + (t.pitch_class % 12) as i64)
        .collect();
    raw.sort_unstable();
    let mid = raw.len() / 2;
    let median = if raw.len() % 2 == 1 {
        raw[mid] as f64
    } else {
        (raw[mid - 1] + raw[mid]) as f64 / 2.0
    };
    let octaves = ((center as f64 - median) / 12.0).round() as i64;
    let mut out: Vec<u8> = raw
        .iter()
        .map(|&p| (p + octaves * 12).clamp(0, MAX_PITCH as i64) as u8)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One-shot helper for the common case: expands `spec` in `octave`, voices the
/// tones around `center` (see [`chord_midi_pitches`]) and returns [`NoteSnap`]s
/// — all at relative step 0 with `duration` and `velocity` — ready for
/// [`stamp_chord`], [`stamp_strum`] or [`paste_notes`]. Sorted low to high.
#[allow(dead_code)]
pub fn chord_snaps(
    spec: &ChordSpec,
    octave: i32,
    center: u8,
    duration: f64,
    velocity: f32,
) -> Vec<NoteSnap> {
    let tones = chord_notes(spec, octave, velocity);
    chord_midi_pitches(&tones, center)
        .into_iter()
        .map(|pitch| NoteSnap {
            pitch,
            start: 0.0,
            duration,
            velocity,
        })
        .collect()
}

/// Effective MIDI pitch of `t` voiced as-is — the `(octave + 1) * 12 +
/// pitch_class` mapping [`chord_midi_pitches`] uses, kept as `i64` so
/// extreme octaves can't overflow before range checks.
fn chord_note_pitch(t: &ChordNote) -> i64 {
    (t.octave as i64 + 1) * 12 + (t.pitch_class % 12) as i64
}

/// Inverts `tones` `n` times: each inversion moves the lowest sounding tone
/// up one octave — the first inversion of C–E–G is E–G–C′. Inverting more
/// times than there are tones keeps rotating: a triad inverted three times
/// is its root voicing an octave up, so [`chord_midi_pitches`] re-centres it
/// on the same sounding pitches. A rotated tone that would pass
/// [`MAX_PITCH`] clamps to the highest in-range pitch of its class (and a
/// tone already above the range clamps down), so the count is preserved and
/// the voicing's pitch classes stay a rotation of the input's. The result is
/// re-sorted low to high; `n` is capped at twelve rotations per tone, past
/// which every tone already sits at the top of MIDI range.
#[allow(dead_code)]
pub fn invert_chord(tones: &[ChordNote], n: usize) -> Vec<ChordNote> {
    let mut out = tones.to_vec();
    if out.is_empty() {
        return out;
    }
    for _ in 0..n.min(out.len().saturating_mul(12)) {
        let lo = out
            .iter()
            .enumerate()
            .min_by_key(|(_, t)| chord_note_pitch(t))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let t = &mut out[lo];
        // Highest octave that keeps this pitch class inside MIDI range.
        let max_octave = ((i64::from(MAX_PITCH) - i64::from(t.pitch_class % 12)) / 12 - 1) as i32;
        t.octave = (t.octave + 1).min(max_octave);
    }
    out.sort_by_key(chord_note_pitch);
    out
}

/// What the chord strip's slash-bass option adds under a stamped chord:
/// nothing, the chord's own root, or a scale degree of the song key — so a
/// C chord can stamp as C, C/G or C/E without changing its quality.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChordBass {
    /// No extra tone under the voicing.
    #[default]
    Off,
    /// The chord's own root — the classic bass doubling.
    Root,
    /// A 0-based scale degree of the song key: `Degree(0)` is always the
    /// tonic, `Degree(4)` the dominant, whatever the chord's own root is.
    Degree(usize),
}

#[allow(dead_code)]
impl ChordBass {
    /// The bass pitch class (0..=11) to voice under a chord on `chord_root`
    /// in `key`, or `None` when the option is off.
    pub fn pitch_class(self, key: SongKey, chord_root: u8) -> Option<u8> {
        match self {
            ChordBass::Off => None,
            ChordBass::Root => Some(chord_root % 12),
            ChordBass::Degree(degree) => Some(key.degree_root(degree)),
        }
    }
}

/// Returns `tones` plus one bass tone on pitch class `bass_pc`, voiced
/// roughly an octave below the chord's lowest tone — the pitch of that
/// class nearest `lowest - 12`, a tritone tie going low. The bass lands
/// strictly below the voicing and inside `0..=MAX_PITCH`; if nothing of
/// that class fits in range the chord is returned unchanged. Re-sorted low
/// to high, like [`invert_chord`]. The tone carries the lowest tone's
/// velocity.
#[allow(dead_code)]
pub fn with_bass(tones: &[ChordNote], bass_pc: u8) -> Vec<ChordNote> {
    let mut out = tones.to_vec();
    if out.is_empty() {
        return out;
    }
    let (lo, velocity) = out
        .iter()
        .map(|t| (chord_note_pitch(t), t.velocity))
        .min_by_key(|&(pitch, _)| pitch)
        .unwrap_or((0, 1.0));
    let bass_pc = i64::from(bass_pc % 12);
    // `below` = semitones from `target` down to the `bass_pc` pitch
    // at-or-under it; past a tritone the pitch above `target` is nearer.
    let target = lo - 12;
    let below = (target.rem_euclid(12) + 12 - bass_pc).rem_euclid(12);
    let mut bass = if below > 6 {
        target + 12 - below
    } else {
        target - below
    };
    if bass < 0 {
        bass += 12;
    }
    // The `+ 12` recovery can overshoot into the voicing when the chord
    // already sits at the bottom of MIDI range — then there is no room
    // for a bass tone at all, so the chord is left unchanged.
    if (0..=i64::from(MAX_PITCH)).contains(&bass) && bass < lo {
        out.push(ChordNote {
            pitch_class: bass.rem_euclid(12) as u8,
            octave: (bass.div_euclid(12) - 1) as i32,
            velocity,
        });
        out.sort_by_key(chord_note_pitch);
    }
    out
}

/// Direction a strummed chord is rolled.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrumDirection {
    /// Lowest pitch first, ascending.
    #[default]
    Up,
    /// Highest pitch first, descending.
    Down,
    /// Outside-in: lowest, highest, next-lowest, next-highest, …
    Alt,
}

#[allow(dead_code)]
impl StrumDirection {
    pub const ALL: [StrumDirection; 3] = [
        StrumDirection::Up,
        StrumDirection::Down,
        StrumDirection::Alt,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StrumDirection::Up => "Up",
            StrumDirection::Down => "Down",
            StrumDirection::Alt => "Alt",
        }
    }
}

/// Orders `pitches` for a strum in `direction`. Input is treated as a set:
/// sorted ascending and deduplicated first. Up = low→high, Down = high→low,
/// Alt = outside-in (low, high, next-low, next-high, …).
#[allow(dead_code)]
fn strum_order(pitches: &[u8], direction: StrumDirection) -> Vec<u8> {
    let mut sorted = pitches.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    match direction {
        StrumDirection::Up => sorted,
        StrumDirection::Down => sorted.iter().rev().copied().collect(),
        StrumDirection::Alt => {
            let mut out = Vec::with_capacity(sorted.len());
            let (mut lo, mut hi) = (0usize, sorted.len());
            while lo < hi {
                out.push(sorted[lo]);
                lo += 1;
                if lo < hi {
                    hi -= 1;
                    out.push(sorted[hi]);
                }
            }
            out
        }
    }
}

/// Plans a strummed chord as [`NoteSnap`]s: `pitches` (any order — sorted
/// internally) are rolled in `direction` order, `spacing` steps apart
/// starting at relative step 0, all `duration` steps long at `velocity`.
/// `spacing` is in sixteenth-note steps, so 0.25 is a thirty-second-note roll
/// and 1.0 a sixteenth; a `spacing` of 0 (or invalid input) produces a block
/// chord — every tone at step 0. Positions are relative to the strum's left
/// edge — feed the result to [`stamp_chord`], [`stamp_strum`] or
/// [`paste_notes`]. Empty input gives an empty vec.
#[allow(dead_code)]
pub fn strum_notes(
    pitches: &[u8],
    spacing: f64,
    direction: StrumDirection,
    duration: f64,
    velocity: f32,
) -> Vec<NoteSnap> {
    let spacing = if spacing.is_finite() {
        spacing.max(0.0)
    } else {
        0.0
    };
    strum_order(pitches, direction)
        .into_iter()
        .enumerate()
        .map(|(i, pitch)| NoteSnap {
            pitch,
            start: i as f64 * spacing,
            duration,
            velocity,
        })
        .collect()
}

/// Inserts a block chord into `p` at song step `start`: every pitch in
/// `pitches` sounds together for `duration` steps at `velocity`. `start` is
/// wrapped into the looping pattern by `rem_euclid`, exactly like
/// [`paste_notes`], and pitches are sorted and deduplicated first. Concrete
/// MIDI pitches come from [`chord_midi_pitches`]/[`chord_snaps`]; for a rolled
/// chord use [`stamp_strum`]. Returns the new note ids in pitch order;
/// writing stops at [`MAX_NOTES_PER_PATTERN`].
#[allow(dead_code)]
pub fn stamp_chord(
    p: &mut Pattern,
    start: f64,
    pitches: &[u8],
    duration: f64,
    velocity: f32,
) -> Vec<u64> {
    let mut sorted = pitches.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.is_empty() {
        return Vec::new();
    }
    let start = if start.is_finite() { start } else { 0.0 };
    let origin = start.rem_euclid(p.length as f64);
    let mut out = Vec::with_capacity(sorted.len());
    for pitch in sorted {
        match add_note(p, pitch, origin, duration, velocity) {
            Some(id) => out.push(id),
            None => break,
        }
    }
    out
}

/// Strummed variant of [`stamp_chord`]: `pitches` are rolled in `direction`
/// order `spacing` steps apart starting at `start` (wrapped into the pattern
/// like [`paste_notes`]). Equivalent to `paste_notes` on [`strum_notes`]
/// output; returns the new note ids. Tones whose offsets run past the pattern
/// end clamp inside it, exactly like [`paste_notes`].
#[allow(dead_code)]
pub fn stamp_strum(
    p: &mut Pattern,
    start: f64,
    pitches: &[u8],
    spacing: f64,
    direction: StrumDirection,
    duration: f64,
    velocity: f32,
) -> Vec<u64> {
    let start = if start.is_finite() { start } else { 0.0 };
    let snaps = strum_notes(pitches, spacing, direction, duration, velocity);
    paste_notes(p, &snaps, start)
}

/// Stamps one chord per slot across `chords` starting at `start` (wrapped
/// into the looping pattern like [`stamp_chord`]): chord `i` lands at
/// `start + i * slot_steps` — callers pass [`STEPS_PER_BAR`] for one chord
/// per bar, starting at the playhead or pointer. Each chord carries its own
/// pre-voiced ascending MIDI pitches (see [`chord_midi_pitches`]); `strum`
/// rolls each chord the way [`stamp_strum`] does (`None` = block chords).
/// A non-positive or non-finite slot falls back to one bar. Returns the new
/// note ids in write order; writing stops at [`MAX_NOTES_PER_PATTERN`] like
/// the single-chord ops.
#[allow(dead_code)]
pub fn stamp_progression(
    p: &mut Pattern,
    start: f64,
    chords: &[Vec<u8>],
    slot_steps: f64,
    duration: f64,
    velocity: f32,
    strum: Option<(f64, StrumDirection)>,
) -> Vec<u64> {
    let slot = if slot_steps.is_finite() && slot_steps > 0.0 {
        slot_steps
    } else {
        STEPS_PER_BAR as f64
    };
    let mut out = Vec::new();
    for (i, pitches) in chords.iter().enumerate() {
        let at = start + i as f64 * slot;
        let ids = match strum {
            Some((spacing, direction)) => {
                stamp_strum(p, at, pitches, spacing, direction, duration, velocity)
            }
            None => stamp_chord(p, at, pitches, duration, velocity),
        };
        out.extend(ids);
    }
    out
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

/// A pattern note captured for the clipboard: pitch, duration and velocity
/// plus a `start` measured *relative* to the copied selection's left edge,
/// so the same snapshot pastes at any playhead position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteSnap {
    pub pitch: u8,
    pub start: f64,
    pub duration: f64,
    pub velocity: f32,
}

/// Captures the notes `ids` selects in `p` as clipboard snapshots, each
/// `start` rebased by the selection's earliest start (the left edge becomes
/// 0). Empty when the selection matches no note in this pattern.
pub fn note_snaps(p: &Pattern, ids: &BTreeSet<u64>) -> Vec<NoteSnap> {
    let sel: Vec<&Note> = p.notes.iter().filter(|n| ids.contains(&n.id)).collect();
    if sel.is_empty() {
        return Vec::new();
    }
    let min_start = sel.iter().map(|n| n.start).fold(f64::INFINITY, f64::min);
    sel.iter()
        .map(|n| NoteSnap {
            pitch: n.pitch,
            start: n.start - min_start,
            duration: n.duration,
            velocity: n.velocity,
        })
        .collect()
}

/// Writes `snaps` into `p` with the selection's left edge at `step` — a song
/// step, wrapped into the looping pattern by `rem_euclid` — and returns the
/// new note ids in snapshot order. Existing notes are kept (paste merges,
/// never replaces). Every note goes through [`add_note`], so positions are
/// clamped inside the pattern and writing stops at
/// [`MAX_NOTES_PER_PATTERN`].
pub fn paste_notes(p: &mut Pattern, snaps: &[NoteSnap], step: f64) -> Vec<u64> {
    if snaps.is_empty() {
        return Vec::new();
    }
    let origin = step.rem_euclid(p.length as f64);
    let mut out = Vec::with_capacity(snaps.len());
    for snap in snaps {
        match add_note(
            p,
            snap.pitch,
            origin + snap.start,
            snap.duration,
            snap.velocity,
        ) {
            Some(id) => out.push(id),
            None => break,
        }
    }
    out
}

/// Deterministically humanizes note timing and velocity. Every selected note
/// (`ids`, or all notes when `None`) gets a velocity offset of up to
/// ±`velocity_amount` — the result clamped to 0..=1 — and a start offset of
/// up to ±`timing_amount` steps, clamped to `0..=length - MIN_NOTE_DURATION`
/// so the note always keeps at least the minimum length inside the pattern.
/// `seed` makes the result reproducible: each note's offsets derive from the
/// seed mixed with its id (via the same xorshift the arpeggiator uses), so
/// they are independent of note order and of which other notes are selected.
/// Re-running with the same seed re-applies the same offsets — pick a new
/// seed for a new take. Returns the number of notes touched; zero amounts or
/// non-finite amounts (treated as 0) are a no-op.
#[allow(dead_code)]
pub fn humanize_notes(
    p: &mut Pattern,
    ids: Option<&BTreeSet<u64>>,
    velocity_amount: f32,
    timing_amount: f64,
    seed: u64,
) -> usize {
    let va = if velocity_amount.is_finite() {
        velocity_amount.max(0.0)
    } else {
        0.0
    };
    let ta = if timing_amount.is_finite() {
        timing_amount.max(0.0)
    } else {
        0.0
    };
    if va <= 0.0 && ta <= 0.0 {
        return 0;
    }
    // Latest start that still leaves MIN_NOTE_DURATION inside the pattern —
    // the same bound set_pattern_length uses to keep notes.
    let max_start = (p.length as f64 - MIN_NOTE_DURATION).max(0.0);
    let mut touched = 0;
    for n in p.notes.iter_mut() {
        if let Some(set) = ids {
            if !set.contains(&n.id) {
                continue;
            }
        }
        // Per-note stream: seed mixed with the note id. The mix is a bijection
        // (unlike `| 1`, which would collapse seeds differing only in bit 0);
        // only the all-zero state needs a nudge since xorshift stuck at 0.
        // Both draws always happen so a note's timing offset doesn't depend
        // on whether velocity is on.
        let mut state = seed ^ n.id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        if state == 0 {
            state = 0x9E37_79B9_7F4A_7C15;
        }
        let dv = (rand_f64(&mut state) * 2.0 - 1.0) * va as f64;
        let dt = (rand_f64(&mut state) * 2.0 - 1.0) * ta;
        if va > 0.0 {
            let v = n.velocity as f64 + dv;
            n.velocity = if v.is_nan() {
                0.0
            } else {
                v.clamp(0.0, 1.0) as f32
            };
        }
        if ta > 0.0 {
            let s = if n.start.is_finite() { n.start } else { 0.0 };
            n.start = (s + dt).clamp(0.0, max_start);
        }
        touched += 1;
    }
    touched
}

/// Scales velocities by `gain` — a pattern/clip "gain" for note levels.
/// `ids` selects notes (`None` = all of them). Results clamp to 0..=1; a
/// negative or non-finite `gain` is a no-op. Returns the number of notes
/// touched.
#[allow(dead_code)]
pub fn scale_velocities(p: &mut Pattern, ids: Option<&BTreeSet<u64>>, gain: f32) -> usize {
    if !gain.is_finite() || gain < 0.0 {
        return 0;
    }
    let g = gain as f64;
    let mut touched = 0;
    for n in p.notes.iter_mut() {
        if let Some(set) = ids {
            if !set.contains(&n.id) {
                continue;
            }
        }
        let v = n.velocity as f64 * g;
        n.velocity = if v.is_nan() {
            0.0
        } else {
            v.clamp(0.0, 1.0) as f32
        };
        touched += 1;
    }
    touched
}

/// Snaps note starts to the nearest `grid` line (in steps) — the selected
/// notes when `ids` is given, else the whole pattern. Returns how many
/// notes actually moved. Starts are clamped inside the pattern like every
/// other edit; a boundary clamp can leave a start off-grid, which the
/// off-grid counter will keep reporting honestly.
pub fn quantize_notes(p: &mut Pattern, ids: Option<&BTreeSet<u64>>, grid: f64) -> usize {
    if !grid.is_finite() || grid <= 0.0 {
        return 0;
    }
    let max_start = (f64::from(p.length) - MIN_NOTE_DURATION).max(0.0);
    let mut moved = 0;
    for n in p.notes.iter_mut() {
        if let Some(set) = ids {
            if !set.contains(&n.id) {
                continue;
            }
        }
        let q = ((n.start / grid).round() * grid).clamp(0.0, max_start);
        if (q - n.start).abs() > f64::EPSILON {
            n.start = q;
            moved += 1;
        }
    }
    moved
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
    use crate::model::KeyMode;

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
    fn audio_lane_rejects_note_content() {
        let mut p = Project::empty();
        p.tracks.clear();
        let id = add_audio_track(&mut p).unwrap();
        let t = track_mut(&mut p, id).unwrap();
        assert!(t.audio);
        // Patterns and pattern clips never play on an audio lane — every
        // creation path must refuse, not just the UI.
        assert!(new_pattern(t, 16).is_none());
        assert!(t.patterns.is_empty());
        assert!(add_clip(&mut p, id, 1, 0.0).is_none());
        assert!(add_audio_clip(&mut p, id, "a.wav", "A", 0.0, 16.0).is_some());
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
    fn note_snaps_rebase_to_the_selection_edge() {
        let mut pat = create_pattern(Instrument::Piano, 32);
        pat.notes.clear();
        let a = add_note(&mut pat, 60, 4.0, 2.0, 0.9).unwrap();
        let b = add_note(&mut pat, 64, 10.5, 1.0, 0.4).unwrap();
        add_note(&mut pat, 67, 20.0, 2.0, 0.7).unwrap();
        let snaps = note_snaps(&pat, &BTreeSet::from([a, b]));
        assert_eq!(
            snaps,
            vec![
                NoteSnap {
                    pitch: 60,
                    start: 0.0,
                    duration: 2.0,
                    velocity: 0.9
                },
                NoteSnap {
                    pitch: 64,
                    start: 6.5,
                    duration: 1.0,
                    velocity: 0.4
                },
            ]
        );
        assert!(note_snaps(&pat, &BTreeSet::new()).is_empty());
        assert!(note_snaps(&pat, &BTreeSet::from([999])).is_empty());
    }

    #[test]
    fn paste_notes_wraps_into_the_pattern_and_merges() {
        let mut pat = create_pattern(Instrument::Piano, 16);
        pat.notes.clear();
        add_note(&mut pat, 40, 0.0, 1.0, 0.5).unwrap();
        let snaps = vec![
            NoteSnap {
                pitch: 60,
                start: 0.0,
                duration: 2.0,
                velocity: 0.9,
            },
            NoteSnap {
                pitch: 64,
                start: 4.0,
                duration: 1.0,
                velocity: 0.4,
            },
        ];
        // Playhead at song step 20 wraps to pattern step 4 (20 % 16).
        let ids = paste_notes(&mut pat, &snaps, 20.0);
        assert_eq!(ids.len(), 2);
        assert_eq!(pat.notes.len(), 3, "paste merges with existing notes");
        let pasted: Vec<_> = ids
            .iter()
            .map(|id| pat.notes.iter().find(|n| n.id == *id).unwrap())
            .collect();
        assert_eq!(
            pasted
                .iter()
                .map(|n| (n.pitch, n.start, n.duration, n.velocity))
                .collect::<Vec<_>>(),
            vec![(60, 4.0, 2.0, 0.9), (64, 8.0, 1.0, 0.4)]
        );
        // A second paste at the same spot stacks another copy (fresh ids).
        let again = paste_notes(&mut pat, &snaps, 20.0);
        assert_eq!(again.len(), 2);
        assert!(again.iter().all(|id| !ids.contains(id)));
        assert_eq!(pat.notes.len(), 5);
        // Negative song steps wrap the same way: -4 % 16 = 12.
        let mut pat2 = create_pattern(Instrument::Piano, 16);
        pat2.notes.clear();
        let ids = paste_notes(&mut pat2, &snaps, -4.0);
        assert_eq!(ids.len(), 2);
        assert_eq!(pat2.notes[0].start, 12.0);
        // Past the pattern end add_note clamps: rel 4 at origin 12 wants 16,
        // so it lands on the last step instead.
        assert_eq!(pat2.notes[1].start, 15.0);
        assert!(paste_notes(&mut pat2, &[], 0.0).is_empty());
    }

    #[test]
    fn paste_notes_stops_at_the_pattern_note_cap() {
        let mut pat = create_pattern(Instrument::Piano, 16);
        pat.notes.clear();
        for i in 0..MAX_NOTES_PER_PATTERN - 1 {
            pat.notes.push(Note::new(60, (i % 16) as f64, 1.0, 0.5));
        }
        let snaps = vec![
            NoteSnap {
                pitch: 60,
                start: 0.0,
                duration: 1.0,
                velocity: 0.5,
            },
            NoteSnap {
                pitch: 62,
                start: 1.0,
                duration: 1.0,
                velocity: 0.5,
            },
        ];
        let ids = paste_notes(&mut pat, &snaps, 0.0);
        assert_eq!(ids.len(), 1, "only one note fits under the cap");
        assert_eq!(pat.notes.len(), MAX_NOTES_PER_PATTERN);
        assert!(paste_notes(&mut pat, &snaps, 0.0).is_empty());
    }

    #[test]
    fn paste_track_duplicates_everything_with_fresh_ids() {
        let (mut p, id) = project_with(Instrument::Bass);
        p.tracks[0].clips.clear();
        let pid = p.tracks[0].patterns[0].id;
        set_pattern_length(&mut p.tracks[0].patterns[0], 32);
        add_note(pattern_mut(&mut p, id, pid).unwrap(), 40, 2.0, 4.0, 0.7);
        let c = add_clip(&mut p, id, pid, 48.0).unwrap();
        p.tracks[0].volume = 0.33;
        p.tracks[0].pan = -0.5;
        p.tracks[0].mute = true;
        p.tracks[0].solo = true;
        p.tracks[0].effects.reverb = 0.9;
        let source = track(&p, id).unwrap().clone();
        let name = source.name.clone();
        let before = p.tracks.len();

        let new = paste_track(&mut p, &source).unwrap();
        assert_eq!(p.tracks.len(), before + 1, "appended at the end");
        let copy = track(&p, new).unwrap();
        assert_ne!(copy.id, source.id);
        assert_eq!(
            copy.name,
            unique_name(&name, p.tracks[..before].iter().map(|t| t.name.clone()))
        );
        assert_ne!(copy.name, name, "name uniquified like a new track");
        assert_eq!(copy.instrument, source.instrument);
        assert_eq!(copy.preset, source.preset);
        assert_eq!(copy.font, source.font);
        assert_eq!(copy.volume, source.volume);
        assert_eq!(copy.pan, source.pan);
        assert_eq!(copy.mute, source.mute);
        assert_eq!(copy.effects, source.effects);
        assert!(!copy.solo, "the paste does not solo the song");
        // Fresh ids everywhere and no shared patterns with the source.
        assert_ne!(copy.patterns[0].id, source.patterns[0].id);
        assert_ne!(copy.patterns[0].notes[0].id, source.patterns[0].notes[0].id);
        assert_eq!(copy.clips.len(), 1);
        let clip = &copy.clips[0];
        assert_ne!(clip.id, c);
        assert_eq!(
            clip.pattern_id, copy.patterns[0].id,
            "clip remapped to the copy"
        );
        assert!(copy
            .clips
            .iter()
            .all(|c| copy.patterns.iter().any(|p| p.id == c.pattern_id)));
        assert!(p.validate().is_ok());

        // At the track limit the paste is refused.
        while p.tracks.len() < MAX_TRACKS {
            add_track(&mut p, Instrument::Piano);
        }
        assert!(paste_track(&mut p, &source).is_none());
    }

    #[test]
    fn paste_track_color_collision_is_repicked() {
        let (mut p, id) = project_with(Instrument::Piano);
        let source = track(&p, id).unwrap().clone();
        // A second track already holds the source colour? Fresh projects give
        // each instrument its own colour, so force one onto track two.
        add_track(&mut p, Instrument::Bass);
        let other = p.tracks[1].id;
        track_mut(&mut p, other).unwrap().color = source.color;
        let new = paste_track(&mut p, &source).unwrap();
        let copy = track(&p, new).unwrap();
        assert_ne!(
            copy.color, source.color,
            "push_track re-picks a free colour on collision"
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
    fn kit_preset_track_gets_the_drum_grid() {
        let preset: InstrumentPreset = serde_json::from_str(
            r#"{
                "id": "test.kit", "name": "Kit",
                "kit": { "pieces": [
                    { "name": "Hat", "pitch": 42,
                      "voice": { "oscillators": [{"waveform": "noise"}] } },
                    { "name": "Kick", "pitch": 60,
                      "voice": { "oscillators": [{"waveform": "sine"}] } }
                ] }
            }"#,
        )
        .unwrap();
        let mut p = Project::empty();
        let id = add_preset_track(&mut p, &preset).unwrap();
        let t = track(&p, id).unwrap();
        assert_eq!(t.instrument, Instrument::Drums);
        assert_eq!(t.preset.as_ref(), Some(&preset));
        assert!(t.shows_drum_grid());
        assert_eq!(t.drum_rows(), vec![(42, "Hat"), (60, "Kick")]);
        // Drum-grid track: "Beat" pattern naming and drum ops on piece pitches.
        assert_eq!(t.patterns[0].name, "Beat 1");
        let pid = t.patterns[0].id;
        let pat = pattern_mut(&mut p, id, pid).unwrap();
        assert!(set_drum(pat, 42, 0, true, 0.9));
        assert!(drum_hit(pat, 42, 0).is_some());
        // Pitch 60 is outside the built-in DRUMS table: kit pieces are arbitrary.
        assert!(set_drum(pat, 60, 4, true, 0.7));
        assert!(cycle_drum_velocity(pat, 60, 4));
        assert!(p.validate().is_ok());
        // A further pattern on the kit track is also a "Beat".
        let t = track_mut(&mut p, id).unwrap();
        let n = new_pattern(t, 16).unwrap();
        assert!(t
            .patterns
            .iter()
            .find(|x| x.id == n)
            .unwrap()
            .name
            .starts_with("Beat"));
    }

    #[test]
    fn set_track_font_assigns_cleans_and_clears() {
        let (mut p, id) = project_with(Instrument::Piano);
        assert!(p.tracks[0].font.is_none());

        // Assign: the reference lands on the track, trimmed and clamped.
        assert!(set_track_font(
            &mut p,
            id,
            Some(FontRef::new("  GeneralUser  ", 200))
        ));
        {
            let t = track(&p, id).unwrap();
            assert_eq!(
                t.font,
                Some(FontRef::new("GeneralUser", 127)),
                "name trimmed, program clamped"
            );
            assert_eq!(t.instrument, Instrument::Piano, "melodic editor kept");
            assert_eq!(instrument_label(t), "GeneralUser · Gunshot");
        }
        // Same reference again is a no-op.
        assert!(!set_track_font(
            &mut p,
            id,
            Some(FontRef::new("GeneralUser", 127))
        ));
        // A different program is a change.
        assert!(set_track_font(
            &mut p,
            id,
            Some(FontRef::new("GeneralUser", 40))
        ));
        assert_eq!(
            instrument_label(track(&p, id).unwrap()),
            "GeneralUser · Violin"
        );
        // Clearing works and is idempotent afterwards.
        assert!(set_track_font(&mut p, id, None));
        assert!(!set_track_font(&mut p, id, None));
        assert!(track(&p, id).unwrap().font.is_none());
        // A blank/whitespace name cleans to a clear (and is a no-op on a
        // track that has no font).
        assert!(!set_track_font(&mut p, id, Some(FontRef::new("   ", 0))));
        assert!(set_track_font(&mut p, id, Some(FontRef::new("F", 1))));
        assert!(set_track_font(&mut p, id, Some(FontRef::new("   ", 0))));
        assert!(track(&p, id).unwrap().font.is_none());
        // Missing track.
        assert!(!set_track_font(&mut p, 12345, Some(FontRef::new("X", 0))));
        assert!(p.validate().is_ok());
    }

    #[test]
    fn set_track_font_keeps_preset_as_fallback() {
        let catalog =
            crate::packs::load_catalog_from(std::path::Path::new("/nonexistent-cadence-test-dir"));
        let preset = catalog
            .instruments
            .first()
            .expect("bundled 8-bit pack")
            .clone();
        let mut p = Project::empty();
        let id = add_preset_track(&mut p, &preset).unwrap();
        assert!(set_track_font(
            &mut p,
            id,
            Some(FontRef::new("GeneralUser", 0))
        ));
        let t = track(&p, id).unwrap();
        assert!(t.font.is_some());
        assert_eq!(t.preset.as_ref(), Some(&preset), "preset kept as fallback");
        assert_eq!(t.instrument, Instrument::Lead, "melodic preset editor");
        // The label shows the font, matching playback precedence.
        assert_eq!(instrument_label(t), "GeneralUser · Acoustic Grand Piano");
        assert_eq!(t.sound_name(), "Acoustic Grand Piano");
        assert!(p.validate().is_ok());
    }

    #[test]
    fn set_track_font_moves_drum_grid_tracks_to_the_piano_roll() {
        // Project::empty()'s starter track is the built-in drum kit.
        let mut p = Project::empty();
        let id = p.tracks[0].id;
        assert!(track(&p, id).unwrap().shows_drum_grid());
        assert!(set_track_font(
            &mut p,
            id,
            Some(FontRef::new("GeneralUser", 0))
        ));
        let t = track(&p, id).unwrap();
        assert_eq!(t.instrument, Instrument::Lead);
        assert!(!t.shows_drum_grid(), "font tracks get the piano roll");
        // Clearing the font leaves the melodic editor in place (undo restores).
        assert!(set_track_font(&mut p, id, None));
        assert_eq!(track(&p, id).unwrap().instrument, Instrument::Lead);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn key_labels_spell_flat_keys_with_flats() {
        use crate::model::KeyMode::{Major, Minor};
        for (root, mode, label) in [
            (0, Major, "C major"),
            (10, Minor, "Bb minor"),
            (10, Major, "Bb major"),
            (1, Major, "Db major"),
            (1, Minor, "C# minor"),
            (6, Major, "F# major"),
            (8, Minor, "G# minor"),
            (8, Major, "Ab major"),
            (3, Minor, "D# minor"),
            (9, Minor, "A minor"),
        ] {
            assert_eq!(key_label(SongKey::new(root, mode)), label);
        }
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

    fn arp_pattern(length: u32) -> Pattern {
        let mut pat = create_pattern(Instrument::Piano, length);
        pat.id = 42;
        pat
    }

    fn arp_notes(p: &Pattern) -> Vec<(u8, f64, f64, f32)> {
        p.notes
            .iter()
            .map(|n| (n.pitch, n.start, n.duration, n.velocity))
            .collect()
    }

    fn pitches(p: &Pattern) -> Vec<u8> {
        p.notes.iter().map(|n| n.pitch).collect()
    }

    #[test]
    fn arpeggio_up_writes_exact_notes() {
        let mut pat = arp_pattern(16);
        let spec = ArpSpec {
            chord_root: 60,
            quality: ChordQuality::Major,
            octaves: 1,
            direction: ArpDirection::Up,
            rate_steps: 4,
            velocity: 0.7,
            accent: true,
            replace: true,
        };
        assert_eq!(generate_arpeggio(&mut pat, &spec), 4);
        let v = 0.7f32;
        let accented = (0.7f32 + ARP_ACCENT).min(1.0);
        assert_eq!(
            arp_notes(&pat),
            vec![
                (60, 0.0, 3.6, accented),
                (64, 4.0, 3.6, v),
                (67, 8.0, 3.6, v),
                (60, 12.0, 3.6, v),
            ]
        );
    }

    #[test]
    fn arpeggio_directions() {
        let spec = |direction| ArpSpec {
            chord_root: 60,
            quality: ChordQuality::Major,
            octaves: 2,
            direction,
            rate_steps: 1,
            accent: false,
            ..ArpSpec::default()
        };
        let mut pat = arp_pattern(8);
        generate_arpeggio(&mut pat, &spec(ArpDirection::Up));
        assert_eq!(pitches(&pat), [60, 64, 67, 72, 76, 79, 60, 64]);
        generate_arpeggio(&mut pat, &spec(ArpDirection::Down));
        assert_eq!(pitches(&pat), [79, 76, 72, 67, 64, 60, 79, 76]);
        let mut pat = arp_pattern(12);
        generate_arpeggio(&mut pat, &spec(ArpDirection::UpDown));
        assert_eq!(
            pitches(&pat),
            [60, 64, 67, 72, 76, 79, 76, 72, 67, 64, 60, 64],
            "no repeated endpoints"
        );
        let mut pat = arp_pattern(4);
        let n = generate_arpeggio(
            &mut pat,
            &ArpSpec {
                octaves: 1,
                rate_steps: 2,
                ..spec(ArpDirection::Block)
            },
        );
        assert_eq!(n, 6, "two hits of a three-note chord");
        assert_eq!(
            arp_notes(&pat)
                .iter()
                .map(|n| (n.0, n.1))
                .collect::<Vec<_>>(),
            [
                (60, 0.0),
                (64, 0.0),
                (67, 0.0),
                (60, 2.0),
                (64, 2.0),
                (67, 2.0)
            ]
        );
        assert!(pat.notes.iter().all(|n| (n.duration - 1.8).abs() < 1e-9));
    }

    #[test]
    fn arpeggio_rates_fill_the_pattern() {
        for (rate, count) in [(1, 32), (2, 16), (4, 8)] {
            let mut pat = arp_pattern(32);
            let spec = ArpSpec {
                rate_steps: rate,
                ..ArpSpec::default()
            };
            assert_eq!(generate_arpeggio(&mut pat, &spec), count, "rate {rate}");
            assert!(pat.notes.iter().all(|n| n.end() <= 32.0 + 1e-9));
        }
        // A pattern length that isn't a multiple of the rate truncates the last note.
        let mut pat = arp_pattern(6);
        let spec = ArpSpec {
            rate_steps: 4,
            ..ArpSpec::default()
        };
        assert_eq!(generate_arpeggio(&mut pat, &spec), 2);
        assert_eq!(pat.notes[1].start, 4.0);
        assert_eq!(pat.notes[1].duration, 2.0);
    }

    #[test]
    fn arpeggio_accents_bar_downbeats_only() {
        let mut pat = arp_pattern(32);
        let spec = ArpSpec {
            rate_steps: 4,
            velocity: 0.9,
            ..ArpSpec::default()
        };
        generate_arpeggio(&mut pat, &spec);
        for n in &pat.notes {
            let want = if n.start % 16.0 == 0.0 { 1.0 } else { 0.9 };
            assert_eq!(n.velocity, want, "note at {}", n.start);
        }
    }

    #[test]
    fn arpeggio_replace_vs_append() {
        let mut pat = arp_pattern(16);
        add_note(&mut pat, 40, 0.0, 1.0, 0.5).unwrap();
        let spec = ArpSpec {
            replace: false,
            rate_steps: 4,
            ..ArpSpec::default()
        };
        assert_eq!(generate_arpeggio(&mut pat, &spec), 4);
        assert_eq!(pat.notes.len(), 5);
        assert_eq!(pat.notes[0].pitch, 40, "append keeps existing notes");
        let spec = ArpSpec {
            replace: true,
            ..spec
        };
        assert_eq!(generate_arpeggio(&mut pat, &spec), 4);
        assert_eq!(pat.notes.len(), 4);
        assert!(pat.notes.iter().all(|n| n.pitch != 40));
    }

    #[test]
    fn arpeggio_respects_note_cap() {
        let mut pat = arp_pattern(MAX_PATTERN_STEPS);
        for i in 0..MAX_NOTES_PER_PATTERN - 3 {
            pat.notes.push(Note::new(30, (i % 16) as f64, 1.0, 0.5));
        }
        let spec = ArpSpec {
            replace: false,
            rate_steps: 1,
            ..ArpSpec::default()
        };
        assert_eq!(generate_arpeggio(&mut pat, &spec), 3);
        assert_eq!(pat.notes.len(), MAX_NOTES_PER_PATTERN);
    }

    #[test]
    fn arpeggio_drops_tones_above_max_pitch() {
        let spec = ArpSpec {
            chord_root: 120,
            quality: ChordQuality::Major,
            octaves: 3,
            ..ArpSpec::default()
        };
        assert_eq!(arp_tones(&spec), [120, 124, 127]);
        let spec = ArpSpec {
            chord_root: 127,
            quality: ChordQuality::Minor,
            octaves: 2,
            ..ArpSpec::default()
        };
        assert_eq!(arp_tones(&spec), [127]);
        // Octaves are capped and ninths interleave correctly.
        let spec = ArpSpec {
            chord_root: 60,
            quality: ChordQuality::Add9,
            octaves: 9,
            ..ArpSpec::default()
        };
        assert_eq!(
            arp_tones(&spec),
            [60, 64, 67, 72, 74, 76, 79, 84, 86, 88, 91, 98]
        );
    }

    #[test]
    fn arpeggio_rejects_invalid_specs_without_touching_the_pattern() {
        let mut pat = arp_pattern(16);
        add_note(&mut pat, 40, 0.0, 1.0, 0.5).unwrap();
        for spec in [
            ArpSpec {
                octaves: 0,
                ..ArpSpec::default()
            },
            ArpSpec {
                rate_steps: 0,
                ..ArpSpec::default()
            },
        ] {
            assert_eq!(generate_arpeggio(&mut pat, &spec), 0);
            assert_eq!(pat.notes.len(), 1);
        }
    }

    #[test]
    fn random_arpeggio_is_deterministic_and_in_chord() {
        let spec = ArpSpec {
            direction: ArpDirection::Random,
            rate_steps: 1,
            ..ArpSpec::default()
        };
        let mut a = arp_pattern(64);
        let mut b = arp_pattern(64);
        generate_arpeggio(&mut a, &spec);
        generate_arpeggio(&mut b, &spec);
        assert_eq!(pitches(&a), pitches(&b));
        let tones = arp_tones(&spec);
        assert!(pitches(&a).iter().all(|p| tones.contains(p)));
        assert!(
            pitches(&a).windows(2).all(|w| w[0] != w[1]),
            "no immediate repeats"
        );
        let distinct: BTreeSet<u8> = pitches(&a).into_iter().collect();
        assert!(distinct.len() >= 4, "random walk covers the chord");
    }

    #[test]
    fn arp_audible_follows_the_playing_sound() {
        // Built-ins: melodic instruments arpeggiate, the drum kit does not.
        assert!(arp_audible(&create_track(Instrument::Piano)));
        assert!(!arp_audible(&create_track(Instrument::Drums)));
        // A melodic pack preset arpeggiates even on a Drums-editor track.
        let melodic = crate::packs::template_pack("mel-pack", "Melodic")
            .unwrap()
            .instruments[0]
            .clone();
        assert!(melodic.kit.is_none());
        assert!(arp_audible(&Track::from_preset(Instrument::Drums, melodic)));
        // A kit preset never does, whatever the editor instrument is.
        let kit: InstrumentPreset = serde_json::from_str(
            r#"{
                "id": "test.kit", "name": "Kit",
                "kit": { "pieces": [
                    { "name": "Kick", "pitch": 36, "voice": {
                        "oscillators": [{"waveform":"sine"}] } }
                ] } }"#,
        )
        .unwrap();
        assert!(!arp_audible(&Track::from_preset(
            Instrument::Piano,
            kit.clone()
        )));
        // A valid SoundFont always counts as melodic, even with a kit fallback.
        let mut fonted = Track::from_font(Instrument::Drums, FontRef::new("GeneralUser", 0));
        fonted.preset = Some(kit);
        assert!(arp_audible(&fonted));
        // An empty font name leaves the (drum) fallback — no arp.
        fonted.font = Some(FontRef::new("  ", 0));
        assert!(!arp_audible(&fonted));
    }

    // ----- chords, strums & humanizing -----------------------------------------

    #[test]
    fn chord_spec_pitch_classes_and_symbol() {
        let c = ChordSpec::new(0, ChordQuality::Major);
        assert_eq!(c.pitch_classes(), [0, 4, 7]);
        assert_eq!(c.symbol(), "C");
        let am7 = ChordSpec::new(9, ChordQuality::Minor7);
        assert_eq!(am7.pitch_classes(), [0, 4, 7, 9]);
        assert_eq!(am7.symbol(), "Am7");
        let add9 = ChordSpec::new(0, ChordQuality::Add9);
        assert_eq!(
            add9.pitch_classes(),
            [0, 2, 4, 7],
            "the 9th folds onto the 2nd's pitch class"
        );
        // The root is reduced mod 12, so any MIDI pitch works as a root.
        assert_eq!(ChordSpec::new(24, ChordQuality::Major).root, 0);
    }

    #[test]
    fn diatonic_chords_match_the_song_key() {
        // C major: I C, ii Dm, iii Em, IV F, V G, vi Am, vii° Bdim.
        let c = diatonic_chords(SongKey::default());
        let expected = [
            (ChordQuality::Major, 0u8, "C"),
            (ChordQuality::Minor, 2, "Dm"),
            (ChordQuality::Minor, 4, "Em"),
            (ChordQuality::Major, 5, "F"),
            (ChordQuality::Major, 7, "G"),
            (ChordQuality::Minor, 9, "Am"),
            (ChordQuality::Diminished, 11, "Bdim"),
        ];
        for (d, &(quality, root, symbol)) in expected.iter().enumerate() {
            let chord = c[d];
            assert_eq!(chord.degree, d);
            assert_eq!(chord.spec.quality, quality);
            assert_eq!(chord.spec.root, root);
            assert_eq!(chord.spec.symbol(), symbol);
        }
        // A minor: i Am, ii° Bdim, III C, iv Dm, v Em, VI F, VII G.
        let a = diatonic_chords(SongKey::new(9, KeyMode::Minor));
        let expected = [
            (ChordQuality::Minor, 9u8, "Am"),
            (ChordQuality::Diminished, 11, "Bdim"),
            (ChordQuality::Major, 0, "C"),
            (ChordQuality::Minor, 2, "Dm"),
            (ChordQuality::Minor, 4, "Em"),
            (ChordQuality::Major, 5, "F"),
            (ChordQuality::Major, 7, "G"),
        ];
        for (d, &(quality, root, symbol)) in expected.iter().enumerate() {
            assert_eq!(a[d].spec.quality, quality, "degree {d}");
            assert_eq!(a[d].spec.root, root, "degree {d}");
            assert_eq!(a[d].spec.symbol(), symbol, "degree {d}");
        }
    }

    #[test]
    fn snap_pitch_to_scale_keeps_scale_tones() {
        // Every scale tone in every octave stays put, in both modes.
        for mode in KeyMode::ALL {
            for root in 0..12 {
                let key = SongKey::new(root, mode);
                for pitch in 0..=MAX_PITCH {
                    let snapped = snap_pitch_to_scale(key, pitch);
                    if key.pitch_classes()[pitch as usize % 12] {
                        assert_eq!(snapped, pitch, "{key:?} moved in-scale {pitch}");
                    } else {
                        assert_ne!(snapped, pitch, "{key:?} kept chromatic {pitch}");
                        assert!(
                            key.pitch_classes()[snapped as usize % 12],
                            "{key:?} snapped {pitch} to out-of-scale {snapped}"
                        );
                        // Diatonic scales step by whole/half tones only, so a
                        // chromatic pitch is always one semitone from scale.
                        assert_eq!((snapped as i32 - pitch as i32).abs(), 1);
                    }
                }
            }
        }
    }

    #[test]
    fn snap_pitch_to_scale_ties_snap_down_and_clamps_edges() {
        let c = SongKey::new(0, KeyMode::Major);
        // Every chromatic pitch in C major sits exactly between two scale
        // tones; the tie resolves to the lower neighbour.
        assert_eq!(snap_pitch_to_scale(c, 61), 60, "C#4 -> C4");
        assert_eq!(snap_pitch_to_scale(c, 63), 62, "D#4 -> D4");
        assert_eq!(snap_pitch_to_scale(c, 66), 65, "F#4 -> F4");
        assert_eq!(snap_pitch_to_scale(c, 68), 67, "G#4 -> G4");
        assert_eq!(snap_pitch_to_scale(c, 70), 69, "A#4 -> A4");
        // A minor {9,11,0,2,4,5,7}: B (11) is in scale, C (0) is too; the
        // chromatic run snaps downward the same way.
        let a = SongKey::new(9, KeyMode::Minor);
        assert_eq!(snap_pitch_to_scale(a, 58), 57, "A#3 -> A3");
        assert_eq!(snap_pitch_to_scale(a, 61), 60, "C#4 -> C4");
        // Pitch 0 can't move down: it snaps up to the nearest scale tone.
        let c_sharp_minor = SongKey::new(1, KeyMode::Minor); // pcs {1,3,4,6,8,9,11}
        assert_eq!(snap_pitch_to_scale(c_sharp_minor, 0), 1);
        // Pitch 127 (G): out of B major's scale -> down to F# (126).
        let b_major = SongKey::new(11, KeyMode::Major);
        assert!(!b_major.pitch_classes()[127 % 12]);
        assert_eq!(snap_pitch_to_scale(b_major, 127), 126);
    }

    #[test]
    fn chord_notes_and_midi_pitches_voice_around_center() {
        // C major in octave 4 prefers C4 E4 G4; centred on middle C the chord
        // keeps its shape (the shift is by whole octaves only).
        let spec = ChordSpec::new(0, ChordQuality::Major);
        let tones = chord_notes(&spec, 4, 0.8);
        assert_eq!(tones.len(), 3);
        assert!(tones.iter().all(|t| t.velocity == 0.8));
        assert_eq!(chord_midi_pitches(&tones, 60), [60, 64, 67]);
        // Centred low, the whole chord moves down an octave intact.
        assert_eq!(chord_midi_pitches(&tones, 48), [48, 52, 55]);
        // Centred at the top of the MIDI range it clamps instead of wrapping.
        assert_eq!(chord_midi_pitches(&tones, 127), [120, 124, 127]);
        // A minor 7 in octave 4 voices A3 C4 E4 G4 around middle C.
        let am7 = chord_notes(&ChordSpec::new(9, ChordQuality::Minor7), 4, 0.7);
        assert_eq!(chord_midi_pitches(&am7, 60), [57, 60, 64, 67]);
        // The add9's ninth prefers the octave above the root, not below it.
        let add9 = chord_notes(&ChordSpec::new(0, ChordQuality::Add9), 4, 1.0);
        assert_eq!(
            add9.iter()
                .map(|t| (t.pitch_class, t.octave))
                .collect::<Vec<_>>(),
            [(0, 4), (4, 4), (7, 4), (2, 5)]
        );
        assert_eq!(chord_midi_pitches(&add9, 60), [60, 64, 67, 74]);
        // Empty input gives an empty result.
        assert!(chord_midi_pitches(&[], 60).is_empty());
    }

    #[test]
    fn chord_snaps_feed_stamp_and_strum() {
        let spec = ChordSpec::new(0, ChordQuality::Major);
        let snaps = chord_snaps(&spec, 4, 60, 2.0, 0.8);
        assert_eq!(
            snaps,
            vec![
                NoteSnap {
                    pitch: 60,
                    start: 0.0,
                    duration: 2.0,
                    velocity: 0.8
                },
                NoteSnap {
                    pitch: 64,
                    start: 0.0,
                    duration: 2.0,
                    velocity: 0.8
                },
                NoteSnap {
                    pitch: 67,
                    start: 0.0,
                    duration: 2.0,
                    velocity: 0.8
                },
            ]
        );
    }

    #[test]
    fn stamp_chord_wraps_sorts_and_respects_the_note_cap() {
        let mut pat = create_pattern(Instrument::Piano, 16);
        pat.notes.clear();
        // Unsorted input with a duplicate still stamps each pitch once.
        let ids = stamp_chord(&mut pat, 20.0, &[67, 60, 64, 60], 2.0, 0.8);
        assert_eq!(ids.len(), 3);
        assert_eq!(
            pat.notes
                .iter()
                .map(|n| (n.pitch, n.start, n.duration, n.velocity))
                .collect::<Vec<_>>(),
            vec![
                (60, 4.0, 2.0, 0.8),
                (64, 4.0, 2.0, 0.8),
                (67, 4.0, 2.0, 0.8)
            ],
            "song step 20 wraps to pattern step 4"
        );
        // Empty input writes nothing; negative steps wrap the same way.
        assert!(stamp_chord(&mut pat, 0.0, &[], 1.0, 0.5).is_empty());
        let mut pat2 = create_pattern(Instrument::Piano, 16);
        pat2.notes.clear();
        let ids = stamp_chord(&mut pat2, -4.0, &[60], 1.0, 0.5);
        assert_eq!(ids.len(), 1);
        assert_eq!(pat2.notes[0].start, 12.0);
        // Writing stops at MAX_NOTES_PER_PATTERN.
        let mut pat3 = create_pattern(Instrument::Piano, 16);
        pat3.notes.clear();
        for i in 0..MAX_NOTES_PER_PATTERN - 1 {
            pat3.notes.push(Note::new(30, (i % 16) as f64, 1.0, 0.5));
        }
        let ids = stamp_chord(&mut pat3, 0.0, &[60, 62, 64], 1.0, 0.8);
        assert_eq!(ids.len(), 1, "only one chord tone fits under the cap");
        assert_eq!(pat3.notes.len(), MAX_NOTES_PER_PATTERN);
    }

    #[test]
    fn strum_notes_rolls_in_direction_order() {
        let pitches = [60u8, 64, 67, 71];
        let up = strum_notes(&pitches, 0.25, StrumDirection::Up, 0.2, 0.9);
        assert_eq!(
            up.iter().map(|s| (s.pitch, s.start)).collect::<Vec<_>>(),
            [(60, 0.0), (64, 0.25), (67, 0.5), (71, 0.75)]
        );
        let down = strum_notes(&pitches, 0.5, StrumDirection::Down, 1.0, 0.4);
        assert_eq!(
            down.iter().map(|s| (s.pitch, s.start)).collect::<Vec<_>>(),
            [(71, 0.0), (67, 0.5), (64, 1.0), (60, 1.5)]
        );
        // Alt strums outside-in: low, high, next-low, next-high.
        let alt = strum_notes(&pitches, 1.0, StrumDirection::Alt, 4.0, 0.7);
        assert_eq!(
            alt.iter().map(|s| s.pitch).collect::<Vec<_>>(),
            [60, 71, 64, 67]
        );
        assert!(alt.iter().all(|s| s.duration == 4.0 && s.velocity == 0.7));
        // Unsorted input is sorted first; spacing 0 blocks the chord.
        let blocked = strum_notes(&[67, 60], 0.0, StrumDirection::Up, 1.0, 0.5);
        assert_eq!(
            blocked
                .iter()
                .map(|s| (s.pitch, s.start))
                .collect::<Vec<_>>(),
            [(60, 0.0), (67, 0.0)]
        );
        assert!(strum_notes(&[], 0.25, StrumDirection::Up, 1.0, 0.5).is_empty());
        assert_eq!(
            StrumDirection::ALL,
            [
                StrumDirection::Up,
                StrumDirection::Down,
                StrumDirection::Alt
            ]
        );
        assert_eq!(StrumDirection::Alt.label(), "Alt");
    }

    #[test]
    fn stamp_strum_writes_rolled_notes_inside_the_pattern() {
        let mut pat = create_pattern(Instrument::Guitar, 16);
        pat.notes.clear();
        let ids = stamp_strum(
            &mut pat,
            12.0,
            &[60, 64, 67],
            1.0,
            StrumDirection::Up,
            0.9,
            0.8,
        );
        assert_eq!(ids.len(), 3);
        assert_eq!(
            pat.notes
                .iter()
                .map(|n| (n.pitch, n.start, n.duration))
                .collect::<Vec<_>>(),
            [(60, 12.0, 0.9), (64, 13.0, 0.9), (67, 14.0, 0.9)]
        );
        // A strum starting near the end wraps its origin; tones running past
        // the end clamp inside the pattern exactly like paste_notes.
        let mut pat2 = create_pattern(Instrument::Guitar, 16);
        pat2.notes.clear();
        let ids = stamp_strum(
            &mut pat2,
            15.0,
            &[60, 64],
            2.0,
            StrumDirection::Down,
            1.0,
            0.6,
        );
        assert_eq!(ids.len(), 2);
        assert_eq!(pat2.notes[0].pitch, 64);
        assert_eq!(pat2.notes[0].start, 15.0);
        assert_eq!(pat2.notes[1].pitch, 60);
        assert_eq!(
            pat2.notes[1].start, 15.0,
            "offset 17 clamps to the last step"
        );
    }

    #[test]
    fn new_chord_qualities_spell_and_voice_correctly() {
        // Interval tables (ascending semitones from the root), suffix and
        // label for the six added qualities.
        let expect: &[(ChordQuality, &[u8], &str)] = &[
            (ChordQuality::MinorAdd9, &[0, 3, 7, 14], "madd9"),
            (ChordQuality::Dominant11, &[0, 4, 7, 10, 14, 17], "11"),
            (ChordQuality::Minor11, &[0, 3, 7, 10, 14, 17], "m11"),
            (ChordQuality::Dominant13, &[0, 4, 7, 10, 14, 21], "13"),
            (ChordQuality::Dominant7Sus4, &[0, 5, 7, 10], "7sus4"),
            (ChordQuality::MinorMajor7, &[0, 3, 7, 11], "mMaj7"),
        ];
        for &(q, intervals, suffix) in expect {
            assert_eq!(q.intervals(), intervals, "{q:?} intervals");
            assert_eq!(q.suffix(), suffix, "{q:?} suffix");
            assert!(!q.label().is_empty(), "{q:?} label");
            assert!(ChordQuality::ALL.contains(&q), "{q:?} missing from ALL");
            assert_eq!(
                ChordSpec::new(0, q).symbol(),
                format!("C{suffix}"),
                "{q:?} symbol"
            );
        }
        // Exact voicings around C4 (the voicing shifts by whole octaves to
        // centre the median on middle C, so wide chords sit an octave low).
        let expect_pitches: &[(ChordQuality, &[u8])] = &[
            (ChordQuality::MinorAdd9, &[60, 63, 67, 74]),
            (ChordQuality::Dominant11, &[48, 52, 55, 58, 62, 65]),
            (ChordQuality::Minor11, &[48, 51, 55, 58, 62, 65]),
            (ChordQuality::Dominant13, &[48, 52, 55, 58, 62, 69]),
            (ChordQuality::Dominant7Sus4, &[48, 53, 55, 58]),
            (ChordQuality::MinorMajor7, &[60, 63, 67, 71]),
        ];
        for &(q, pitches) in expect_pitches {
            let tones = chord_notes(&ChordSpec::new(0, q), 4, 0.8);
            assert_eq!(chord_midi_pitches(&tones, 60), pitches, "{q:?} voicing");
        }
    }

    #[test]
    fn every_quality_voices_inside_midi_range() {
        for q in ChordQuality::ALL {
            let spec = ChordSpec::new(0, q);
            let tones = chord_notes(&spec, 4, 0.8);
            for center in [0u8, 1, 60, 100, 127] {
                let out = chord_midi_pitches(&tones, center);
                assert!(!out.is_empty(), "{q:?} voices nothing at center {center}");
                assert!(
                    out.iter().all(|&p| p <= MAX_PITCH),
                    "{q:?} exceeds MIDI range at center {center}"
                );
                assert!(
                    out.windows(2).all(|w| w[0] < w[1]),
                    "{q:?} not ascending at center {center}"
                );
                assert!(
                    out.iter().any(|&p| p % 12 == spec.root),
                    "{q:?} lost its root at center {center}"
                );
            }
            // Centred at the top of the range every chord clamps, never wraps:
            // the highest tone sits within a semitone of 127 (a chord whose
            // top interval is narrow lands a whole step below).
            let top = chord_midi_pitches(&tones, 127);
            assert!(
                *top.last().unwrap() >= 126,
                "{q:?} does not reach the top: {top:?}"
            );
        }
    }

    #[test]
    fn borrowed_chords_match_the_song_key() {
        // C major: parallel-minor borrows + the Neapolitan + three
        // secondary dominants, spelled flat per role.
        let c = borrowed_chords(SongKey::new(0, KeyMode::Major));
        let expect: &[(&str, &str)] = &[
            ("iv", "Fm"),
            ("bVII", "Bb"),
            ("bVI", "Ab"),
            ("bIII", "Eb"),
            ("N6", "Db"),
            ("V/vi", "E7"),
            ("V/ii", "A7"),
            ("V/IV", "C7"),
        ];
        assert_eq!(c.len(), expect.len());
        for (chord, &(label, symbol)) in c.iter().zip(expect) {
            assert_eq!(chord.label, label);
            assert_eq!(chord.symbol, symbol);
        }
        // A minor: parallel-major borrows, the harmonic-minor dominant and
        // secondary dominants of the natural-minor degrees.
        let a = borrowed_chords(SongKey::new(9, KeyMode::Minor));
        let expect: &[(&str, &str)] = &[
            ("IV", "D"),
            ("N6", "Bb"),
            ("V", "E7"),
            ("V/III", "G7"),
            ("V/iv", "A7"),
            ("V/VI", "C7"),
        ];
        assert_eq!(a.len(), expect.len());
        for (chord, &(label, symbol)) in a.iter().zip(expect) {
            assert_eq!(chord.label, label);
            assert_eq!(chord.symbol, symbol);
        }
        // Every key and mode produces in-range roots with matching symbols.
        for root in 0..12 {
            for mode in KeyMode::ALL {
                let pal = borrowed_chords(SongKey::new(root, mode));
                assert!(!pal.is_empty());
                for chord in &pal {
                    assert!(chord.spec.root < 12, "root {root} {mode:?}");
                    assert_eq!(
                        format!(
                            "{}{}",
                            NOTE_NAMES_FLAT[chord.spec.root as usize],
                            chord.spec.quality.suffix()
                        ),
                        chord.symbol,
                        "root {root} {mode:?} {}",
                        chord.label
                    );
                }
            }
        }
    }

    #[test]
    fn progression_presets_are_consistent_and_resolve_in_key() {
        for preset in PROGRESSIONS {
            assert!(!preset.name.is_empty(), "preset name");
            assert_eq!(
                preset.degrees.len(),
                preset.qualities.len(),
                "{} degrees/qualities",
                preset.name
            );
            assert!(
                preset.degrees.iter().all(|&d| d < 7),
                "{} degree in range",
                preset.name
            );
            let specs = progression_specs(SongKey::new(0, KeyMode::Major), &preset, None);
            assert_eq!(specs.len(), preset.degrees.len());
            assert!(specs.iter().all(|s| s.root < 12));
        }
        // Pop resolves to the classic I-V-vi-IV in C major.
        let pop = progression_specs(SongKey::new(0, KeyMode::Major), &PROGRESSIONS[0], None);
        assert_eq!(
            pop.iter().map(|s| s.symbol()).collect::<Vec<_>>(),
            ["C", "G", "Am", "F"]
        );
        // Jazz is ii7-V7-Imaj7; the blues preset is twelve bars of C7.
        let jazz = progression_specs(SongKey::new(0, KeyMode::Major), &PROGRESSIONS[2], None);
        assert_eq!(
            jazz.iter().map(|s| s.symbol()).collect::<Vec<_>>(),
            ["Dm7", "G7", "Cmaj7"]
        );
        let blues = progression_specs(SongKey::new(0, KeyMode::Major), &PROGRESSIONS[5], None);
        assert_eq!(blues.len(), 12);
        assert_eq!(blues[0].symbol(), "C7");
        assert_eq!(blues[10].symbol(), "C7");
        // The same preset works in a minor key: i-VI-III-VII in A minor.
        let epic = progression_specs(SongKey::new(9, KeyMode::Minor), &PROGRESSIONS[3], None);
        assert_eq!(
            epic.iter().map(|s| s.symbol()).collect::<Vec<_>>(),
            ["Am", "F", "C", "G"]
        );
        // An override quality replaces every step's quality.
        let all = progression_specs(
            SongKey::new(0, KeyMode::Major),
            &PROGRESSIONS[0],
            Some(ChordQuality::Minor7),
        );
        assert!(all.iter().all(|s| s.quality == ChordQuality::Minor7));
    }

    #[test]
    fn stamp_progression_writes_one_chord_per_bar_and_wraps() {
        let chords: Vec<Vec<u8>> = vec![
            vec![60, 64, 67],
            vec![57, 60, 64],
            vec![55, 59, 62],
            vec![53, 57, 60],
        ];
        let mut pat = create_pattern(Instrument::Piano, 64);
        pat.notes.clear();
        let ids = stamp_progression(&mut pat, 0.0, &chords, 16.0, 8.0, 0.8, None);
        assert_eq!(ids.len(), 12);
        assert_eq!(pat.notes.len(), 12);
        for (i, chord) in chords.iter().enumerate() {
            let at = i as f64 * 16.0;
            for &pitch in chord {
                assert!(
                    pat.notes
                        .iter()
                        .any(|n| n.pitch == pitch && n.start == at && n.duration == 8.0),
                    "bar {i} pitch {pitch}"
                );
            }
        }
        // Starting mid-song, slots wrap into the looping pattern and strums
        // roll each chord the same way the single-chord op does.
        let mut pat2 = create_pattern(Instrument::Piano, 32);
        pat2.notes.clear();
        let ids = stamp_progression(
            &mut pat2,
            16.0,
            &chords,
            16.0,
            4.0,
            0.5,
            Some((1.0, StrumDirection::Up)),
        );
        assert_eq!(ids.len(), 12);
        assert_eq!(pat2.notes.len(), 12);
        for (i, chord) in chords.iter().enumerate() {
            let at = (16.0 + i as f64 * 16.0).rem_euclid(32.0);
            for (t, &pitch) in chord.iter().enumerate() {
                assert!(
                    pat2.notes
                        .iter()
                        .any(|n| n.pitch == pitch && n.start == at + t as f64),
                    "bar {i} tone {t} pitch {pitch}"
                );
            }
        }
        // Empty input writes nothing; a non-positive slot falls back to one bar.
        let mut pat3 = create_pattern(Instrument::Piano, 32);
        pat3.notes.clear();
        assert!(stamp_progression(&mut pat3, 0.0, &[], 16.0, 1.0, 0.5, None).is_empty());
        assert!(pat3.notes.is_empty());
        let ids = stamp_progression(&mut pat3, 0.0, &[vec![60], vec![64]], -1.0, 1.0, 0.5, None);
        assert_eq!(ids.len(), 2);
        let starts: Vec<f64> = pat3.notes.iter().map(|n| n.start).collect();
        assert!(
            starts.contains(&0.0) && starts.contains(&16.0),
            "{starts:?}"
        );
    }

    #[test]
    fn new_chord_qualities_survive_a_serde_roundtrip() {
        // The wire names are the snake_case variant names.
        for (json_name, q) in [
            ("minor_add9", ChordQuality::MinorAdd9),
            ("dominant11", ChordQuality::Dominant11),
            ("minor11", ChordQuality::Minor11),
            ("dominant13", ChordQuality::Dominant13),
            ("dominant7_sus4", ChordQuality::Dominant7Sus4),
            ("minor_major7", ChordQuality::MinorMajor7),
        ] {
            let ev: ChordEvent =
                serde_json::from_str(&format!(r#"{{"root":0,"quality":"{json_name}"}}"#))
                    .unwrap_or_else(|e| panic!("{json_name}: {e}"));
            assert_eq!(ev.quality, q, "{json_name}");
        }
        // A project carrying every new quality round-trips and validates.
        let mut p = Project::empty();
        let quals = [
            ChordQuality::MinorAdd9,
            ChordQuality::Dominant11,
            ChordQuality::Minor11,
            ChordQuality::Dominant13,
            ChordQuality::Dominant7Sus4,
            ChordQuality::MinorMajor7,
        ];
        for (i, q) in quals.iter().enumerate() {
            add_chord_event(&mut p, (i * 16) as f64, (i * 2) as u8, *q).unwrap();
        }
        let json = serde_json::to_string_pretty(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chord_events, p.chord_events);
        assert!(back.validate().is_ok());
    }

    #[test]
    fn humanize_is_deterministic_and_clamps() {
        let build = || {
            let mut pat = create_pattern(Instrument::Piano, 16);
            pat.notes.clear();
            add_note(&mut pat, 60, 0.5, 1.0, 0.9).unwrap();
            add_note(&mut pat, 64, 8.0, 2.0, 0.3).unwrap();
            add_note(&mut pat, 67, 15.5, 1.0, 0.6).unwrap();
            pat
        };
        // Clones share note ids, so the per-note seeds line up.
        let snapshot = build();
        let mut a = snapshot.clone();
        let mut b = snapshot.clone();
        let ids: BTreeSet<u64> = snapshot.notes.iter().map(|n| n.id).collect();
        assert_eq!(humanize_notes(&mut a, Some(&ids), 0.3, 1.0, 42), 3);
        assert_eq!(humanize_notes(&mut b, Some(&ids), 0.3, 1.0, 42), 3);
        assert_eq!(a.notes, b.notes, "same seed + same notes = same result");

        // A different seed moves the notes differently.
        let mut d = snapshot.clone();
        humanize_notes(&mut d, None, 0.3, 1.0, 43);
        assert_ne!(a.notes, d.notes);

        // Absurd amounts still clamp: velocity 0..=1, start inside the pattern
        // with at least MIN_NOTE_DURATION of room.
        let mut c = snapshot.clone();
        let all: BTreeSet<u64> = c.notes.iter().map(|n| n.id).collect();
        assert_eq!(humanize_notes(&mut c, Some(&all), 50.0, 100.0, 7), 3);
        for n in &c.notes {
            assert!((0.0..=1.0).contains(&n.velocity), "velocity {}", n.velocity);
            assert!(
                n.start >= 0.0 && n.start <= 16.0 - MIN_NOTE_DURATION,
                "start {}",
                n.start
            );
        }

        // A subset selection leaves the other notes untouched.
        let mut e = snapshot.clone();
        let only_first = BTreeSet::from([e.notes[0].id]);
        assert_eq!(humanize_notes(&mut e, Some(&only_first), 0.5, 2.0, 9), 1);
        assert_eq!(
            e.notes[1..],
            snapshot.notes[1..],
            "unselected notes are not touched"
        );

        // Zero amounts are a no-op; unknown ids touch nothing.
        let mut f = snapshot.clone();
        assert_eq!(humanize_notes(&mut f, None, 0.0, 0.0, 1), 0);
        assert_eq!(f.notes, snapshot.notes);
        assert_eq!(
            humanize_notes(&mut f, Some(&BTreeSet::from([999])), 0.5, 0.5, 1),
            0
        );
    }

    #[test]
    fn scale_velocities_multiplies_the_selection_and_clamps() {
        let mut pat = create_pattern(Instrument::Pad, 16);
        pat.notes.clear();
        let a = add_note(&mut pat, 60, 0.0, 1.0, 0.8).unwrap();
        add_note(&mut pat, 64, 4.0, 1.0, 0.4).unwrap();
        // Whole pattern halved.
        assert_eq!(scale_velocities(&mut pat, None, 0.5), 2);
        assert!((pat.notes[0].velocity - 0.4).abs() < 1e-6);
        assert!((pat.notes[1].velocity - 0.2).abs() < 1e-6);
        // Selection only; results clamp at 1.0.
        assert_eq!(
            scale_velocities(&mut pat, Some(&BTreeSet::from([a])), 3.0),
            1
        );
        assert_eq!(pat.notes[0].velocity, 1.0);
        assert!((pat.notes[1].velocity - 0.2).abs() < 1e-6);
        // Negative or non-finite gain is a no-op.
        assert_eq!(scale_velocities(&mut pat, None, f32::NAN), 0);
        assert_eq!(scale_velocities(&mut pat, None, -1.0), 0);
        assert!((pat.notes[1].velocity - 0.2).abs() < 1e-6);
    }

    #[test]
    fn marker_ops_add_move_rename_delete_and_validate() {
        let mut p = Project::empty();
        let a = add_marker(&mut p, 64.0, None).unwrap();
        let b = add_marker(&mut p, 8.0, Some("Intro".into())).unwrap();
        let c = add_marker(&mut p, -3.0, Some("  ".into())).unwrap();
        assert_eq!(p.markers.len(), 3);
        // Sorted by step; negative step clamps to 0; the blank name fell back.
        assert_eq!(p.markers[0].id, c);
        assert_eq!(p.markers[0].step, 0.0);
        assert_eq!(p.markers[0].name, "Section");
        assert_eq!(p.markers[1].id, b);
        assert_eq!(p.markers[2].id, a);
        p.validate().unwrap();

        // Auto names cycle through the section list without repeating:
        // "Intro" is taken (twice), so the next free names come out in order.
        let d = add_marker(&mut p, 16.0, None).unwrap();
        let e = add_marker(&mut p, 24.0, None).unwrap();
        let names: Vec<&str> = p.markers.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names.iter().position(|n| *n == "Verse"), Some(2));
        assert!(names.contains(&"Chorus"));
        let _ = (d, e);

        // Move clamps into range and re-sorts.
        assert!(move_marker(&mut p, a, 999_999.0));
        assert_eq!(
            p.markers.last().unwrap().id,
            a,
            "moved marker sorts to the end"
        );
        assert_eq!(p.markers.last().unwrap().step, MAX_PATTERN_STEPS as f64);
        assert!(!move_marker(&mut p, 999_999, 0.0));

        // Rename cleans the name; unknown ids are a no-op.
        assert!(rename_marker(&mut p, b, "Verse\u{0} One"));
        let b_name = p.markers.iter().find(|m| m.id == b).unwrap().name.clone();
        assert_eq!(b_name, "Verse One");
        assert!(!rename_marker(&mut p, 999_999, "x"));
        p.validate().unwrap();

        // Delete removes exactly that marker; unknown ids report false.
        assert!(delete_marker(&mut p, b));
        assert!(!p.markers.iter().any(|m| m.id == b));
        assert!(!delete_marker(&mut p, b));
        p.validate().unwrap();

        // The cap rejects further additions but leaves the list intact.
        p.markers.clear();
        for _ in 0..MAX_MARKERS {
            assert!(add_marker(&mut p, 0.0, None).is_some());
        }
        assert!(add_marker(&mut p, 0.0, None).is_none());
        assert_eq!(p.markers.len(), MAX_MARKERS);
        p.validate().unwrap();
    }

    #[test]
    fn chord_event_ops_add_move_cycle_delete_and_label() {
        let mut p = Project::empty();
        let a = add_chord_event(&mut p, 32.0, 7, ChordQuality::Major).unwrap();
        let b = add_chord_event(&mut p, 0.0, 0, ChordQuality::Major7).unwrap();
        let c = add_chord_event(&mut p, -8.0, 21, ChordQuality::Minor7).unwrap();
        assert_eq!(p.chord_events.len(), 3);
        // Sorted by step then id; the negative step clamps to 0; the root
        // reduced mod 12 (21 -> 9 = A). `b` was added before `c`, so it
        // wins the tie on step 0.
        assert_eq!(p.chord_events[0].id, b);
        assert_eq!(p.chord_events[1].id, c);
        assert_eq!(p.chord_events[2].id, a);
        assert_eq!(p.chord_events[1].step, 0.0);
        assert_eq!(chord_event_label(&p.chord_events[0]), "Cmaj7");
        assert_eq!(chord_event_label(&p.chord_events[1]), "Am7");
        p.validate().unwrap();

        // The bar lookup only answers for an exact step.
        assert_eq!(chord_event_at_step(&p, 0.0), Some(b));
        assert_eq!(chord_event_at_step(&p, 1.0), None);

        // Labels ride the shared symbol logic: root + suffix.
        let ev = ChordEvent {
            id: 1,
            step: 0.0,
            root: 6,
            quality: ChordQuality::HalfDiminished7,
        };
        assert_eq!(chord_event_label(&ev), "F#m7b5");

        // Move clamps into range and re-sorts.
        assert!(move_chord_event(&mut p, a, 999_999.0));
        assert_eq!(p.chord_events.last().unwrap().id, a);
        assert_eq!(
            p.chord_events.last().unwrap().step,
            MAX_PATTERN_STEPS as f64
        );
        assert!(!move_chord_event(&mut p, 999_999, 0.0));

        // Click-cycling steps one quality per direction and wraps at both
        // ends of the list.
        assert!(cycle_chord_event_quality(&mut p, b, -1));
        assert_eq!(
            p.chord_events.iter().find(|e| e.id == b).unwrap().quality,
            ChordQuality::Dominant7
        );
        assert!(cycle_chord_event_quality(&mut p, b, 1));
        assert_eq!(
            p.chord_events.iter().find(|e| e.id == b).unwrap().quality,
            ChordQuality::Major7
        );
        assert!(set_chord_event(&mut p, b, 0, ChordQuality::Minor9));
        assert!(cycle_chord_event_quality(&mut p, b, 1));
        assert_eq!(
            p.chord_events.iter().find(|e| e.id == b).unwrap().quality,
            ChordQuality::MinorAdd9
        );
        assert!(cycle_chord_event_quality(&mut p, b, -1));
        assert_eq!(
            p.chord_events.iter().find(|e| e.id == b).unwrap().quality,
            ChordQuality::Minor9
        );
        // And it wraps from the end of the list too.
        assert!(set_chord_event(&mut p, b, 0, ChordQuality::MinorMajor7));
        assert!(cycle_chord_event_quality(&mut p, b, 1));
        assert_eq!(
            p.chord_events.iter().find(|e| e.id == b).unwrap().quality,
            ChordQuality::Major
        );
        // And the root wraps at the octave.
        assert!(cycle_chord_event_root(&mut p, b, 13));
        assert_eq!(p.chord_events.iter().find(|e| e.id == b).unwrap().root, 1);
        assert!(!cycle_chord_event_root(&mut p, 999_999, 1));

        // set_chord_event rewrites both fields and normalizes the root.
        assert!(set_chord_event(&mut p, a, 12 + 5, ChordQuality::Dominant9));
        let ev = p.chord_events.iter().find(|e| e.id == a).unwrap();
        assert_eq!((ev.root, ev.quality), (5, ChordQuality::Dominant9));
        assert!(!set_chord_event(&mut p, 999_999, 0, ChordQuality::Major));

        // The tonic the strip writes comes from the song key.
        p.key = SongKey::new(9, crate::model::KeyMode::Minor);
        assert_eq!(tonic_chord_spec(p.key).symbol(), "Am");
        p.key = SongKey::new(3, crate::model::KeyMode::Major);
        assert_eq!(tonic_chord_spec(p.key).symbol(), "D#");
        // …and is shown spelled for the key.
        assert_eq!(tonic_chord_spec(p.key).symbol_in(p.key), "Eb");
        p.validate().unwrap();

        // Delete removes exactly that event; unknown ids report false.
        assert!(delete_chord_event(&mut p, b));
        assert!(!p.chord_events.iter().any(|e| e.id == b));
        assert!(!delete_chord_event(&mut p, b));

        // The cap rejects further additions but leaves the list intact.
        p.chord_events.clear();
        for _ in 0..MAX_CHORD_EVENTS {
            assert!(add_chord_event(&mut p, 0.0, 0, ChordQuality::Major).is_some());
        }
        assert!(add_chord_event(&mut p, 0.0, 0, ChordQuality::Major).is_none());
        assert_eq!(p.chord_events.len(), MAX_CHORD_EVENTS);
        p.validate().unwrap();
    }

    // ----- split_clip --------------------------------------------------------

    /// The first track's (step, duration, pitch) events — a cheap before /
    /// after comparison that a split reproduces the clip's exact sound.
    fn clip_events(p: &Project) -> Vec<(f64, f64, u8)> {
        p.events(false)
            .into_iter()
            .filter(|e| e.track_index == 0)
            .map(|e| (e.step, e.duration, e.pitch))
            .collect()
    }

    #[test]
    fn split_clip_rejects_edges_and_missing_targets() {
        let (mut p, tid) = project_with(Instrument::Piano);
        let pid = p.tracks[0].patterns[0].id;
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap(); // 16-step clip
        let before = p.clone();
        for at in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -4.0,
            0.0,
            16.0,
            99.0,
        ] {
            assert!(!split_clip(&mut p, tid, clip, at), "at {at}");
            assert_eq!(p, before, "rejected split must not mutate (at {at})");
        }
        assert!(!split_clip(&mut p, 999_999, clip, 8.0));
        assert!(!split_clip(&mut p, tid, 999_999, 8.0));
        assert_eq!(p, before);
        // A clip pointing at a deleted pattern cannot split.
        p.tracks[0].patterns.clear();
        assert!(!split_clip(&mut p, tid, clip, 8.0));
    }

    #[test]
    fn split_clip_mid_pattern_flattens_and_validates() {
        let (mut p, tid) = project_with(Instrument::Piano);
        let pid = p.tracks[0].patterns[0].id;
        let pat = p.tracks[0].patterns.first_mut().unwrap();
        pat.notes.clear();
        add_note(pat, 60, 0.0, 2.0, 0.9).unwrap();
        add_note(pat, 64, 6.0, 4.0, 0.8).unwrap(); // crosses the cut at 8
        add_note(pat, 67, 12.0, 2.0, 0.7).unwrap();
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap();

        assert!(can_split_clip(&p, tid, clip, 8.0));
        assert!(split_clip(&mut p, tid, clip, 8.0));
        p.validate().unwrap();
        let t = &p.tracks[0];
        assert_eq!(t.clips.len(), 3, "starter clip + the two pieces");
        assert_eq!(t.patterns.len(), 3, "two fresh split patterns");
        let li = t.clips.iter().position(|c| c.id == clip).unwrap();
        let (l, r) = (&t.clips[li], &t.clips[li + 1]);
        assert_eq!(l.id, clip, "left keeps the original id");
        assert_ne!(r.id, clip, "right gets a fresh id");
        assert_ne!(l.pattern_id, pid);
        assert_ne!(r.pattern_id, pid);
        assert_eq!(l.start, 0.0);
        assert_eq!(r.start, 8.0, "right starts exactly at the split");
        assert_eq!(l.loops, 1);
        assert_eq!(r.loops, 1);
        let lp = pattern(t, l.pattern_id).unwrap();
        let rp = pattern(t, r.pattern_id).unwrap();
        assert_eq!(lp.length, 8, "left piece ends exactly at the split");
        assert_eq!(rp.length, 8);
        // Left: the first note whole, the crossing note shortened to the cut.
        assert_eq!(
            lp.notes
                .iter()
                .map(|n| (n.pitch, n.start, n.duration))
                .collect::<Vec<_>>(),
            vec![(60, 0.0, 2.0), (64, 6.0, 2.0)]
        );
        // Right: the crossing note's tail re-attacks at step 0, the last
        // note shifted back by the cut.
        assert_eq!(
            rp.notes
                .iter()
                .map(|n| (n.pitch, n.start, n.duration))
                .collect::<Vec<_>>(),
            vec![(64, 0.0, 2.0), (67, 4.0, 2.0)]
        );
        assert!(lp.name.contains("split") && rp.name.contains("split"));
        assert_ne!(lp.id, rp.id);
    }

    #[test]
    fn split_clip_preserves_the_event_stream_without_crossings() {
        let (mut p, tid) = project_with(Instrument::Pad);
        let pid = p.tracks[0].patterns[0].id;
        let pat = p.tracks[0].patterns.first_mut().unwrap();
        pat.notes.clear();
        add_note(pat, 60, 2.0, 1.0, 0.8).unwrap();
        add_note(pat, 64, 6.0, 2.0, 0.8).unwrap();
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap();
        assert!(set_clip_loops(&mut p, tid, clip, 3)); // 48 steps
        let before = clip_events(&p);
        // Step 20 lands inside the second repetition: two new patterns hold
        // the flattened halves, and every event stays where it sounded.
        assert!(split_clip(&mut p, tid, clip, 20.0));
        p.validate().unwrap();
        assert_eq!(clip_events(&p), before);
        let t = &p.tracks[0];
        let li = t.clips.iter().position(|c| c.id == clip).unwrap();
        let (l, r) = (&t.clips[li], &t.clips[li + 1]);
        assert_eq!(r.start, 20.0);
        assert_eq!(pattern(t, l.pattern_id).unwrap().length, 20);
        assert_eq!(pattern(t, r.pattern_id).unwrap().length, 28);
    }

    #[test]
    fn split_clip_on_a_loop_boundary_shares_the_pattern() {
        let (mut p, tid) = project_with(Instrument::Guitar);
        let pid = p.tracks[0].patterns[0].id;
        add_note(p.tracks[0].patterns.first_mut().unwrap(), 62, 4.0, 2.0, 0.8).unwrap();
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap();
        assert!(set_clip_loops(&mut p, tid, clip, 4)); // 64 steps
        set_clip_fades(&mut p, tid, clip, 2.0, 6.0);
        let patterns_before = p.tracks[0].patterns.len();
        let events_before = clip_events(&p);

        assert!(split_clip(&mut p, tid, clip, 32.0));
        p.validate().unwrap();
        let t = &p.tracks[0];
        assert_eq!(t.clips.len(), 3);
        assert_eq!(
            t.patterns.len(),
            patterns_before,
            "a boundary cut needs no new pattern"
        );
        let li = t.clips.iter().position(|c| c.id == clip).unwrap();
        let (l, r) = (&t.clips[li], &t.clips[li + 1]);
        assert_eq!(l.id, clip);
        assert_eq!(l.pattern_id, pid);
        assert_eq!(r.pattern_id, pid, "right shares the original pattern");
        assert_eq!(l.loops, 2);
        assert_eq!(r.loops, 2);
        assert_eq!(r.start, 32.0);
        assert_eq!(l.fade_in, 2.0, "left keeps the clip's fade-in");
        assert_eq!(l.fade_out, 0.0);
        assert_eq!(r.fade_in, 0.0);
        assert_eq!(r.fade_out, 6.0, "right keeps the clip's fade-out");
        assert_eq!(clip_events(&p), events_before);
    }

    #[test]
    fn split_clip_fades_clamp_to_their_new_spans() {
        let (mut p, tid) = project_with(Instrument::Bass);
        let pid = p.tracks[0].patterns[0].id;
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap(); // 16 steps
        set_clip_fades(&mut p, tid, clip, 12.0, 12.0);
        assert!(split_clip(&mut p, tid, clip, 4.0));
        p.validate().unwrap();
        let t = &p.tracks[0];
        let li = t.clips.iter().position(|c| c.id == clip).unwrap();
        let (l, r) = (&t.clips[li], &t.clips[li + 1]);
        assert_eq!(l.fade_in, 4.0, "fade-in clamps to the left span");
        assert_eq!(l.fade_out, 0.0);
        assert_eq!(r.fade_in, 0.0);
        assert_eq!(r.fade_out, 12.0, "fade-out keeps its length on the right");
    }

    #[test]
    fn split_clip_fractional_cut_keeps_every_note_playing() {
        let (mut p, tid) = project_with(Instrument::Pluck);
        let pid = p.tracks[0].patterns[0].id;
        let pat = p.tracks[0].patterns.first_mut().unwrap();
        pat.notes.clear();
        add_note(pat, 60, 4.3, 0.5, 0.8).unwrap(); // starts just before the cut
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap();
        assert!(split_clip(&mut p, tid, clip, 4.5));
        p.validate().unwrap();
        // The note at 4.3 must survive into the left piece, so the left
        // pattern length rounds up to cover it (clip spans are whole steps).
        let t = &p.tracks[0];
        let li = t.clips.iter().position(|c| c.id == clip).unwrap();
        let lp = pattern(t, t.clips[li].pattern_id).unwrap();
        assert_eq!(lp.length, 5);
        assert_eq!(lp.notes[0].start, 4.3);
        assert_eq!(t.clips[li + 1].start, 4.5);
    }

    #[test]
    fn split_clip_honours_track_limits() {
        let (mut p, tid) = project_with(Instrument::Lead);
        let pid = p.tracks[0].patterns[0].id;
        let clip = add_clip(&mut p, tid, pid, 0.0).unwrap();
        assert!(set_clip_loops(&mut p, tid, clip, 2));
        // No room for the two flattened patterns: a mid-repetition cut
        // fails but a loop-boundary cut still works — it adds no patterns.
        while p.tracks[0].patterns.len() < MAX_PATTERNS_PER_TRACK - 1 {
            let mut extra = p.tracks[0].patterns[0].clone();
            extra.id = next_id();
            p.tracks[0].patterns.push(extra);
        }
        assert_eq!(p.tracks[0].patterns.len(), MAX_PATTERNS_PER_TRACK - 1);
        p.validate().unwrap();
        assert!(!split_clip(&mut p, tid, clip, 8.0));
        assert!(split_clip(&mut p, tid, clip, 16.0));
        p.validate().unwrap();
    }

    #[test]
    fn split_clip_right_clip_sits_directly_after_the_left() {
        let (mut p, tid) = project_with(Instrument::Piano);
        let pid = p.tracks[0].patterns[0].id;
        let first = add_clip(&mut p, tid, pid, 16.0).unwrap();
        let second = add_clip(&mut p, tid, pid, 48.0).unwrap();
        assert!(split_clip(&mut p, tid, first, 24.0));
        let ids: Vec<u64> = p.tracks[0].clips.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 4, "starter clip + first + right + second");
        assert_eq!(ids[1], first);
        assert_eq!(ids[3], second, "the right piece inserts after the left");
        assert!(ids[2] != first && ids[2] != second);
    }

    // ----- chord inversions & slash bass -------------------------------------

    #[test]
    fn invert_chord_rotates_the_lowest_tone_up() {
        let spec = ChordSpec::new(0, ChordQuality::Major); // C major
        let tones = chord_notes(&spec, 4, 0.8);
        assert_eq!(chord_midi_pitches(&tones, 60), vec![60, 64, 67]);

        // C–E–G → E–G–C′: the inverted voicing re-centres on the same home.
        let first = invert_chord(&tones, 1);
        assert_eq!(first.len(), tones.len(), "inversion keeps every tone");
        assert_eq!(chord_midi_pitches(&first, 60), vec![52, 55, 60]);
        // → G–C′–E′ second inversion.
        let second = invert_chord(&tones, 2);
        assert_eq!(chord_midi_pitches(&second, 60), vec![55, 60, 64]);
        // A third rotation on a triad is the root voicing an octave up —
        // re-centring lands on the same sounding pitches.
        let third = invert_chord(&tones, 3);
        assert_eq!(chord_midi_pitches(&third, 60), vec![60, 64, 67]);

        // Zero rotations and empty input are identities.
        assert_eq!(invert_chord(&tones, 0), tones);
        assert!(invert_chord(&[], 2).is_empty());
    }

    #[test]
    fn invert_chord_preserves_pitch_classes_as_a_rotation() {
        // Inverting a seventh chord three times puts its top tone on the
        // bottom: A–C–E–G → G–A–C′–E′.
        let spec = ChordSpec::new(9, ChordQuality::Minor7);
        let tones = chord_notes(&spec, 3, 0.8); // A3 C4 E4 G4 = 57 60 64 67
        let third = invert_chord(&tones, 3);
        assert_eq!(third.len(), tones.len());
        // Every output pitch class is one of the input's — a pure rotation.
        let orig_pcs: Vec<u8> = tones.iter().map(|t| t.pitch_class).collect();
        assert!(third.iter().all(|t| orig_pcs.contains(&t.pitch_class)));
        // Re-centred on the same home, the third inversion sounds G A C E.
        assert_eq!(chord_midi_pitches(&third, 60), vec![55, 57, 60, 64]);
    }

    #[test]
    fn invert_chord_clamps_to_midi_range_preserving_count() {
        // F10 = 137 is already above the range: rotating can't raise it, so
        // it clamps down to the highest in-range F (F9 = 125) — the count
        // and the pitch class survive.
        let tones = vec![
            ChordNote {
                pitch_class: 5,
                octave: 10,
                velocity: 0.8,
            },
            ChordNote {
                pitch_class: 7,
                octave: 10,
                velocity: 0.8,
            },
        ];
        let inv = invert_chord(&tones, 1);
        assert_eq!(inv.len(), 2, "clamping never drops a tone");
        let pitches = chord_midi_pitches(&inv, 127);
        assert!(pitches.iter().all(|&p| p <= MAX_PITCH));
        // At the very top, C9 = 120 has no C above it in range: the
        // rotation is a no-op rather than a dropped or folded tone.
        let top = vec![ChordNote {
            pitch_class: 0,
            octave: 9,
            velocity: 0.8,
        }];
        let inv = invert_chord(&top, 3);
        assert_eq!(inv.len(), 1);
        assert_eq!(chord_note_pitch(&inv[0]), 120);
    }

    #[test]
    fn with_bass_voices_a_tone_below_the_chord() {
        let spec = ChordSpec::new(0, ChordQuality::Major); // C–E–G
        let tones = chord_notes(&spec, 4, 0.8);
        // Root doubling lands exactly an octave under the lowest tone.
        let bass = with_bass(&tones, 0);
        assert_eq!(bass.len(), tones.len() + 1);
        assert_eq!(chord_midi_pitches(&bass, 60), vec![48, 60, 64, 67]);
        // A slash degree picks the class's pitch nearest an octave below
        // the lowest tone: G under C4 → G2 = 43 (nearer to 48 than G3 = 55).
        let bass = with_bass(&tones, 7);
        assert_eq!(chord_midi_pitches(&bass, 60), vec![43, 60, 64, 67]);
        // Under a first-inversion voicing the bass tracks the new lowest
        // tone: E4's C sits at C3 = 48, still below the whole voicing.
        let inv = invert_chord(&tones, 1);
        let bass = with_bass(&inv, 0);
        assert_eq!(chord_midi_pitches(&bass, 60), vec![48, 64, 67, 72]);
        // Velocity comes from the voicing; empty input stays empty.
        assert!((bass[0].velocity - 0.8).abs() < 1e-6);
        assert!(with_bass(&[], 0).is_empty());
    }

    #[test]
    fn with_bass_stays_below_the_voicing_or_bows_out() {
        // A chord already at the bottom of MIDI range has no room for a
        // bass tone — the chord comes back unchanged rather than doubling.
        let low = vec![ChordNote {
            pitch_class: 0,
            octave: -1,
            velocity: 0.8,
        }]; // C(-1) = 0 — nothing fits below it
        assert_eq!(with_bass(&low, 0).len(), 1);
        // Whatever fits always lands strictly under the lowest tone and
        // inside range.
        let spec = ChordSpec::new(2, ChordQuality::Minor);
        let tones = chord_notes(&spec, 2, 0.8); // D minor ~ D3
        let bass = with_bass(&tones, 9); // A
        let lo = tones.iter().map(chord_note_pitch).min().unwrap();
        assert!(chord_note_pitch(&bass[0]) < lo);
        assert!(chord_note_pitch(&bass[0]) >= 0);
    }

    #[test]
    fn chord_bass_pitch_class_resolves_root_and_degrees() {
        let cmaj = SongKey::new(0, KeyMode::Major);
        assert_eq!(ChordBass::Off.pitch_class(cmaj, 0), None);
        // The chord root, whatever it is — A minor's bass is A.
        assert_eq!(ChordBass::Root.pitch_class(cmaj, 9), Some(9));
        // Scale degrees come from the key: tonic C, dominant G in C major.
        assert_eq!(ChordBass::Degree(0).pitch_class(cmaj, 0), Some(0));
        assert_eq!(ChordBass::Degree(4).pitch_class(cmaj, 0), Some(7));
        // The same degree follows the key — degree 4 of A minor is E.
        let amin = SongKey::new(9, KeyMode::Minor);
        assert_eq!(ChordBass::Degree(4).pitch_class(amin, 0), Some(4));
    }

    #[test]
    fn automation_points_add_move_delete_and_clear() {
        use crate::model::{automation_value, AutomationLane};
        let (mut p, id) = project_with(Instrument::Lead);
        assert!(p.tracks[0].automation.is_none());
        let vol = AutomationLane::Volume;
        let pan = AutomationLane::Pan;

        // First add creates the automation block; points stay sorted by step.
        assert_eq!(add_automation_point(&mut p, id, vol, 8.0, 1.0), Some(0));
        assert_eq!(add_automation_point(&mut p, id, vol, 0.0, 0.5), Some(0));
        assert_eq!(add_automation_point(&mut p, id, vol, 4.0, 0.75), Some(1));
        let lane = &p.tracks[0].automation.as_ref().unwrap().volume;
        assert_eq!(
            lane.iter().map(|pt| pt.step).collect::<Vec<_>>(),
            [0.0, 4.0, 8.0]
        );
        // The lane interpolates the way the renderer reads it.
        let auto = p.tracks[0].automation.as_ref().unwrap();
        assert_eq!(automation_value(&auto.volume, 6.0), Some(0.875));

        // Same-step add writes over the existing point rather than stacking.
        assert_eq!(add_automation_point(&mut p, id, vol, 4.0, 0.2), Some(1));
        assert_eq!(p.tracks[0].automation.as_ref().unwrap().volume.len(), 3);
        assert_eq!(
            p.tracks[0].automation.as_ref().unwrap().volume[1],
            AutomationPoint {
                step: 4.0,
                value: 0.2
            }
        );

        // Values clamp to the lane's range; steps clamp to the song span.
        assert_eq!(add_automation_point(&mut p, id, vol, 9.0, 99.0), Some(3));
        assert_eq!(
            p.tracks[0].automation.as_ref().unwrap().volume[3].value,
            crate::model::MAX_AUTOMATION_GAIN
        );
        // A step that clamps onto an existing point overwrites it too.
        assert_eq!(add_automation_point(&mut p, id, vol, -5.0, 0.4), Some(0));
        assert_eq!(p.tracks[0].automation.as_ref().unwrap().volume.len(), 4);
        assert_eq!(
            p.tracks[0]
                .automation
                .as_ref()
                .unwrap()
                .volume
                .first()
                .unwrap()
                .step,
            0.0
        );

        // Moving a point past a neighbour re-sorts and returns its new index.
        assert_eq!(
            move_automation_point(&mut p, id, vol, 0, 12.0, 0.9),
            Some(3)
        );
        assert_eq!(
            move_automation_point(&mut p, id, vol, 3, -8.0, 0.1),
            Some(0)
        );
        let lane = &p.tracks[0].automation.as_ref().unwrap().volume;
        assert_eq!(
            lane[0],
            AutomationPoint {
                step: 0.0,
                value: 0.1
            }
        );
        assert!(lane.windows(2).all(|w| w[0].step <= w[1].step));

        // Deleting the last points of both lanes collapses the block to None.
        while delete_automation_point(&mut p, id, vol, 0) {}
        assert!(p.tracks[0].automation.is_none());
        // A pan lane keeps the block alive until it, too, is empty.
        assert!(add_automation_point(&mut p, id, pan, 0.0, -0.5).is_some());
        assert!(add_automation_point(&mut p, id, vol, 0.0, 1.0).is_some());
        assert!(clear_automation_lane(&mut p, id, vol));
        let auto = p.tracks[0].automation.as_ref().unwrap();
        assert!(auto.volume.is_empty() && auto.pan.len() == 1);
        assert!(!clear_automation_lane(&mut p, id, vol));
        assert!(clear_automation_lane(&mut p, id, pan));
        assert!(p.tracks[0].automation.is_none());

        // Bad targets are reported, not panics.
        assert!(!delete_automation_point(&mut p, id, vol, 0));
        assert_eq!(move_automation_point(&mut p, id, vol, 0, 0.0, 0.0), None);
        assert_eq!(add_automation_point(&mut p, u64::MAX, vol, 0.0, 0.0), None);
    }

    #[test]
    fn automation_point_cap_is_enforced() {
        use crate::model::AutomationLane;
        let (mut p, id) = project_with(Instrument::Piano);
        for i in 0..crate::model::MAX_AUTOMATION_POINTS {
            assert!(
                add_automation_point(&mut p, id, AutomationLane::Volume, i as f64, 0.5).is_some()
            );
        }
        // A new step past the cap is refused; overwriting an existing point isn't.
        assert_eq!(
            add_automation_point(
                &mut p,
                id,
                AutomationLane::Volume,
                crate::model::MAX_AUTOMATION_POINTS as f64 + 4.0,
                0.5
            ),
            None
        );
        assert_eq!(
            add_automation_point(&mut p, id, AutomationLane::Volume, 0.0, 0.7),
            Some(0)
        );
        p.validate().unwrap();
    }
}
