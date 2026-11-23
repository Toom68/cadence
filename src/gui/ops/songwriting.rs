//! Songwriting helpers: turning the arranger's chord lane into playable
//! parts (block chords, basslines), named drum grooves, and the song
//! templates offered by the New Song dialog.
//!
//! Like the rest of [`super`], everything here is pure project editing —
//! no egui, audio or filesystem — so each call can be wrapped in one undo
//! step by the GUI.

use super::*;

// ----- chord lane → parts ----------------------------------------------------

/// One chord of the song's chord lane with the song steps it covers: from
/// its own step up to the next chord event, or the song end for the last.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChordSpan {
    pub spec: ChordSpec,
    pub start: f64,
    pub end: f64,
}

/// The chord lane as consecutive [`ChordSpan`]s, sorted by step. Events at
/// or past the song end are ignored; when several events share one step
/// the last (by id, the lane's sort order) wins. Empty when the lane is.
pub fn chord_lane_spans(project: &Project) -> Vec<ChordSpan> {
    let end = project.total_steps().min(MAX_SONG_STEPS);
    let mut events: Vec<&ChordEvent> = project
        .chord_events
        .iter()
        .filter(|e| e.step.is_finite() && e.step >= 0.0 && e.step < end)
        .collect();
    events.sort_by(|a, b| a.step.total_cmp(&b.step).then(a.id.cmp(&b.id)));
    let mut spans = Vec::with_capacity(events.len());
    for (i, ev) in events.iter().enumerate() {
        if events.get(i + 1).is_some_and(|n| n.step == ev.step) {
            continue;
        }
        let next = events.get(i + 1).map_or(end, |n| n.step);
        spans.push(ChordSpan {
            spec: ChordSpec::new(ev.root, ev.quality),
            start: ev.step,
            end: next,
        });
    }
    spans
}

/// Why a chord-lane part (see [`play_chords_on_track`],
/// [`bassline_from_chords`]) could not be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanePartError {
    NoTrack,
    AudioTrack,
    DrumTrack,
    NoChords,
    ChordsPastEnd,
    TooManyPatterns,
    TooManyClips,
    EventBudget,
}

impl LanePartError {
    /// A polite, user-facing explanation for the status bar.
    pub fn message(self) -> &'static str {
        match self {
            LanePartError::NoTrack => "Select an instrument track first.",
            LanePartError::AudioTrack => {
                "Audio tracks play recordings, not notes — pick an instrument track."
            }
            LanePartError::DrumTrack => {
                "Drum tracks have no chord tones — pick a melodic track (keys, pad, bass…)."
            }
            LanePartError::NoChords => {
                "The chord lane is empty — add chords on the arranger's chord strip first."
            }
            LanePartError::ChordsPastEnd => {
                "Every chord sits past the song end — lengthen the song or move the chords."
            }
            LanePartError::TooManyPatterns => "This track already holds the maximum of patterns.",
            LanePartError::TooManyClips => "This track already holds the maximum of clips.",
            LanePartError::EventBudget => "The song is at its note-event limit.",
        }
    }
}

/// What a chord-lane part wrote: the new pattern, the clip that plays it,
/// and how many notes / chords went in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanePart {
    pub pattern_id: u64,
    pub clip_id: u64,
    pub notes: usize,
    pub chords: usize,
}

/// Pattern name of the block-chord part [`play_chords_on_track`] writes.
pub const SONG_CHORDS_PATTERN: &str = "Song chords";
/// Pattern name of the part [`bassline_from_chords`] writes.
pub const BASSLINE_PATTERN: &str = "Bassline";

/// Whether `name` is `base` or one of its numbered siblings ("Bassline 2")
/// — a pattern an earlier run of the same action generated.
fn is_generated(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|n| n.parse::<u32>().is_ok())
}

/// The bar-line pieces of `start..end`: one `(from, to)` per bar touched,
/// so long chords re-strike on every bar line. Pieces shorter than the
/// minimum note length are dropped.
fn bar_segments(start: f64, end: f64) -> Vec<(f64, f64)> {
    let bar = STEPS_PER_BAR as f64;
    let mut out = Vec::new();
    let mut a = start;
    while a < end - 1e-9 {
        let b = (((a / bar) + 1e-9).floor() * bar + bar).min(end);
        if b - a >= MIN_NOTE_DURATION {
            out.push((a, b));
        }
        a = b;
    }
    out
}

/// Shared body of the chord-lane parts: writes one new pattern on
/// `track_id` spanning from the bar of the first chord to the song end
/// (`write` fills it, one call per [`ChordSpan`], positions relative to
/// the pattern origin passed as its last argument) and places one clip at
/// that bar so the part plays across the song. Clips in that range that
/// play an empty pattern, or a pattern an earlier run of the same action
/// generated (`base` name), are replaced — re-running after editing the
/// chord lane swaps the old part out — and generated patterns left without
/// clips are deleted. Atomic: on error nothing changes.
fn write_lane_part(
    project: &mut Project,
    track_id: u64,
    base: &str,
    mut write: impl FnMut(&mut Pattern, &ChordSpan, f64),
) -> Result<LanePart, LanePartError> {
    let t = track(project, track_id).ok_or(LanePartError::NoTrack)?;
    if t.audio {
        return Err(LanePartError::AudioTrack);
    }
    if t.shows_drum_grid() {
        return Err(LanePartError::DrumTrack);
    }
    if project.chord_events.is_empty() {
        return Err(LanePartError::NoChords);
    }
    let spans = chord_lane_spans(project);
    let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return Err(LanePartError::ChordsPastEnd);
    };
    let bar = STEPS_PER_BAR as f64;
    let origin = (first.start / bar).floor() * bar;
    let end = last.end;
    let length = ((end - origin).ceil() as u32).clamp(1, MAX_PATTERN_STEPS);
    let mut pat = Pattern {
        id: next_id(),
        name: String::new(),
        length,
        notes: Vec::new(),
    };
    for span in &spans {
        write(&mut pat, span, origin);
    }
    pat.sort_notes();
    let notes = pat.notes.len();

    let mut t = t.clone();
    let replaceable: BTreeSet<u64> = t
        .patterns
        .iter()
        .filter(|p| p.notes.is_empty() || is_generated(&p.name, base))
        .map(|p| p.id)
        .collect();
    let removed: Vec<u64> = t
        .clips
        .iter()
        .filter(|c| {
            replaceable.contains(&c.pattern_id)
                && c.start < end
                && c.start + clip_len(&t, c) > origin
        })
        .map(|c| c.id)
        .collect();
    let freed: usize = t
        .clips
        .iter()
        .filter(|c| removed.contains(&c.id))
        .map(|c| clip_events(&t, c))
        .fold(0, usize::saturating_add);
    if notes > event_budget(project).saturating_add(freed) {
        return Err(LanePartError::EventBudget);
    }
    t.clips.retain(|c| !removed.contains(&c.id));
    let clips = &t.clips;
    t.patterns
        .retain(|p| !is_generated(&p.name, base) || clips.iter().any(|c| c.pattern_id == p.id));
    if t.patterns.len() >= MAX_PATTERNS_PER_TRACK {
        return Err(LanePartError::TooManyPatterns);
    }
    if t.clips.len() >= MAX_CLIPS_PER_TRACK {
        return Err(LanePartError::TooManyClips);
    }
    pat.name = unique_name(base, t.patterns.iter().map(|p| p.name.clone()));
    let pattern_id = pat.id;
    let clip_id = next_id();
    t.patterns.push(pat);
    t.clips.push(Clip {
        id: clip_id,
        pattern_id,
        start: origin,
        loops: 1,
        fade_in: 0.0,
        fade_out: 0.0,
    });
    if let Some(slot) = track_mut(project, track_id) {
        *slot = t;
    }
    fit_bars_to_content(project);
    Ok(LanePart {
        pattern_id,
        clip_id,
        notes,
        chords: spans.len(),
    })
}

/// Writes the song's chord-lane chords into `track_id` as block chords
/// (or strums, when `strum` has a positive spacing): each chord sounds for
/// its whole span — until the next chord or the song end — re-struck on
/// every bar line so decaying sounds stay audible. `voice` turns a chord
/// into concrete MIDI pitches (the chord strip's voicing in the GUI); a
/// chord it voices to nothing is skipped. See [`write_lane_part`] for how
/// the pattern and clip are placed. Refuses audio and drum tracks and an
/// empty chord lane.
pub fn play_chords_on_track(
    project: &mut Project,
    track_id: u64,
    voice: impl Fn(&ChordSpec) -> Vec<u8>,
    strum: Option<(f64, StrumDirection)>,
    velocity: f32,
) -> Result<LanePart, LanePartError> {
    let strum = strum.filter(|(spacing, _)| spacing.is_finite() && *spacing > 0.0);
    let mut voiced = 0;
    let part = write_lane_part(
        project,
        track_id,
        SONG_CHORDS_PATTERN,
        |pat, span, origin| {
            let pitches = voice(&span.spec);
            if pitches.is_empty() {
                return;
            }
            voiced += 1;
            for (a, b) in bar_segments(span.start, span.end) {
                let at = a - origin;
                let dur = b - a;
                match strum {
                    Some((spacing, direction)) => {
                        // Later tones of the roll end with the slot too.
                        let roll = spacing * pitches.len().saturating_sub(1) as f64;
                        let dur = (dur - roll).max(MIN_NOTE_DURATION);
                        stamp_strum(pat, at, &pitches, spacing, direction, dur, velocity);
                    }
                    None => {
                        stamp_chord(pat, at, &pitches, dur, velocity);
                    }
                }
            }
        },
    )?;
    Ok(LanePart {
        chords: voiced,
        ..part
    })
}

/// How [`bassline_from_chords`] plays each chord.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BassStyle {
    /// One long root per bar.
    #[default]
    Roots,
    /// Root on beat 1, fifth on beat 3.
    RootFifth,
    /// Driving eighth-note roots.
    Eighths,
    /// Eighths bouncing between the root and its octave.
    OctaveBounce,
}

impl BassStyle {
    pub const ALL: [BassStyle; 4] = [
        BassStyle::Roots,
        BassStyle::RootFifth,
        BassStyle::Eighths,
        BassStyle::OctaveBounce,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BassStyle::Roots => "Roots",
            BassStyle::RootFifth => "Root–fifth",
            BassStyle::Eighths => "Eighths",
            BassStyle::OctaveBounce => "Octave bounce",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            BassStyle::Roots => "One held root per bar — ballads, pads, big choruses",
            BassStyle::RootFifth => "Root on beat 1, fifth on beat 3 — pop, country, rock",
            BassStyle::Eighths => "Steady eighth-note roots — driving rock and punk",
            BassStyle::OctaveBounce => "Eighths jumping root ↔ octave — disco, house, synth-pop",
        }
    }

    /// Steps between note starts.
    fn slot(self) -> f64 {
        match self {
            BassStyle::Roots => 16.0,
            BassStyle::RootFifth => 8.0,
            BassStyle::Eighths | BassStyle::OctaveBounce => 2.0,
        }
    }

    /// Note length as a share of the slot.
    fn gate(self) -> f64 {
        match self {
            BassStyle::Roots => 0.94,
            BassStyle::RootFifth => 0.9,
            BassStyle::Eighths => 0.75,
            BassStyle::OctaveBounce => 0.7,
        }
    }
}

/// Lowest bass root: C2. Roots land in `BASS_LOW..BASS_LOW + 12` (C2–B2).
pub const BASS_LOW: u8 = 36;
/// Highest pitch a bassline fifth climbs to before folding down an octave.
pub const BASS_HIGH: u8 = 52;

/// The bass-register pitch (C2–B2) of pitch class `pc`.
pub fn bass_pitch(pc: u8) -> u8 {
    BASS_LOW + pc % 12
}

/// Writes a bassline following the chord lane into `track_id` in `style`:
/// roots in the C2–B2 register (fifths above, folded below past
/// [`BASS_HIGH`]). `bass` is the chord strip's slash-bass option: a key
/// degree there replaces each chord's root as the bass note (a pedal), Off
/// or Root keeps the chord roots. Every chord starts on its bass note;
/// on-beat notes get `velocity`, off-beats a little less. Placement and
/// refusals as [`play_chords_on_track`].
pub fn bassline_from_chords(
    project: &mut Project,
    track_id: u64,
    style: BassStyle,
    bass: ChordBass,
    velocity: f32,
) -> Result<LanePart, LanePartError> {
    let key = project.key;
    let slot = style.slot();
    write_lane_part(project, track_id, BASSLINE_PATTERN, |pat, span, origin| {
        let root = bass_pitch(span.spec.root);
        let low = bass_pitch(
            bass.pitch_class(key, span.spec.root)
                .unwrap_or(span.spec.root),
        );
        let fifth = if root + 7 <= BASS_HIGH {
            root + 7
        } else {
            root - 5
        };
        let mut s = span.start;
        while s < span.end - 1e-9 {
            let k = ((s / slot) + 1e-9).floor();
            let slot_end = ((k + 1.0) * slot).min(span.end);
            let room = slot_end - s;
            if room >= MIN_NOTE_DURATION {
                let odd = (k as i64) % 2 == 1;
                let pitch = match style {
                    _ if s == span.start => low,
                    BassStyle::RootFifth if odd => fifth,
                    BassStyle::OctaveBounce if odd => low + 12,
                    _ => low,
                };
                let on_beat = (s / 4.0 - (s / 4.0).round()).abs() < 1e-9;
                let vel = if on_beat { velocity } else { velocity * 0.8 };
                let dur = (slot * style.gate()).min(room);
                add_note(pat, pitch, s - origin, dur, vel);
            }
            s = slot_end;
        }
    })
}

// ----- drum grooves ----------------------------------------------------------

/// A drum-kit piece a groove asks for, mapped onto whatever rows the
/// track's kit has by [`drum_piece_pitch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrumPiece {
    Kick,
    Snare,
    Clap,
    ClosedHat,
    OpenHat,
    LowTom,
    MidTom,
    HighTom,
    Crash,
    Ride,
}

impl DrumPiece {
    pub fn label(self) -> &'static str {
        match self {
            DrumPiece::Kick => "kick",
            DrumPiece::Snare => "snare",
            DrumPiece::Clap => "clap",
            DrumPiece::ClosedHat => "closed hat",
            DrumPiece::OpenHat => "open hat",
            DrumPiece::LowTom => "low tom",
            DrumPiece::MidTom => "mid tom",
            DrumPiece::HighTom => "high tom",
            DrumPiece::Crash => "crash",
            DrumPiece::Ride => "ride",
        }
    }

    /// General-MIDI pitches for the piece, preferred first.
    fn pitches(self) -> &'static [u8] {
        match self {
            DrumPiece::Kick => &[36, 35],
            DrumPiece::Snare => &[38, 40],
            DrumPiece::Clap => &[39],
            DrumPiece::ClosedHat => &[42, 44],
            DrumPiece::OpenHat => &[46],
            DrumPiece::LowTom => &[45, 43, 41],
            DrumPiece::MidTom => &[47, 48],
            DrumPiece::HighTom => &[50],
            DrumPiece::Crash => &[49, 57, 55, 52],
            DrumPiece::Ride => &[51, 59, 53],
        }
    }

    /// Row-name keywords (lower case) for kits off the GM map, plus words
    /// that rule a row out ("open" is not a closed hat).
    fn keywords(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            DrumPiece::Kick => (&["kick", "bd"], &[]),
            DrumPiece::Snare => (&["snare", "sd"], &[]),
            DrumPiece::Clap => (&["clap"], &[]),
            DrumPiece::ClosedHat => (&["hat", "hh"], &["open"]),
            DrumPiece::OpenHat => (&["open"], &[]),
            DrumPiece::LowTom => (&["low tom", "floor tom"], &[]),
            DrumPiece::MidTom => (&["mid tom", "tom"], &["low", "floor", "high", "hi "]),
            DrumPiece::HighTom => (&["high tom", "hi tom"], &[]),
            DrumPiece::Crash => (&["crash", "cymbal", "splash"], &[]),
            DrumPiece::Ride => (&["ride"], &[]),
        }
    }

    /// Stand-ins when the kit lacks the piece itself.
    fn fallbacks(self) -> &'static [DrumPiece] {
        match self {
            DrumPiece::Clap => &[DrumPiece::Snare],
            DrumPiece::HighTom => &[DrumPiece::MidTom, DrumPiece::LowTom, DrumPiece::Snare],
            DrumPiece::MidTom => &[DrumPiece::LowTom, DrumPiece::HighTom, DrumPiece::Snare],
            DrumPiece::LowTom => &[DrumPiece::MidTom, DrumPiece::HighTom, DrumPiece::Snare],
            DrumPiece::Crash => &[DrumPiece::Ride],
            DrumPiece::Ride => &[DrumPiece::ClosedHat],
            _ => &[],
        }
    }
}

/// The row pitch playing `piece` in a kit with `rows` (`(pitch, name)` as
/// [`Track::drum_rows`] gives them): a GM pitch match first, then a row
/// whose name mentions the piece. `None` when the kit has neither.
pub fn drum_piece_pitch(rows: &[(u8, &str)], piece: DrumPiece) -> Option<u8> {
    if let Some(&p) = piece
        .pitches()
        .iter()
        .find(|&&p| rows.iter().any(|(rp, _)| *rp == p))
    {
        return Some(p);
    }
    let (words, not) = piece.keywords();
    rows.iter()
        .find(|(_, name)| {
            let name = format!("{} ", name.to_lowercase());
            words.iter().any(|w| name.contains(w)) && !not.iter().any(|w| name.contains(w))
        })
        .map(|(p, _)| *p)
}

/// [`drum_piece_pitch`] with the piece's stand-ins tried in order.
fn resolve_piece(rows: &[(u8, &str)], piece: DrumPiece) -> Option<u8> {
    std::iter::once(piece)
        .chain(piece.fallbacks().iter().copied())
        .find_map(|p| drum_piece_pitch(rows, p))
}

/// The named grooves of the drum editor's Fill menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Groove {
    /// The old Fill: a kick on every beat, added to what is there.
    KickEveryBeat,
    FourOnTheFloor,
    BackbeatRock,
    HalfTime,
    BoomBap,
    TrapHats,
    House,
    Dembow,
    /// A snare/tom fill over the second half of the pattern's last bar.
    FillLastBar,
}

/// One groove row: a piece and a 16-step bar written as characters —
/// `X` accent, `x` normal, `o` ghost, `r` a 32nd-note roll (two soft hits
/// in one step), `.` rest.
type GrooveLine = (DrumPiece, &'static str);

impl Groove {
    /// Menu order: the full grooves, then the additive tools.
    pub const ALL: [Groove; 9] = [
        Groove::FourOnTheFloor,
        Groove::BackbeatRock,
        Groove::HalfTime,
        Groove::BoomBap,
        Groove::TrapHats,
        Groove::House,
        Groove::Dembow,
        Groove::FillLastBar,
        Groove::KickEveryBeat,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Groove::KickEveryBeat => "Kick on every beat",
            Groove::FourOnTheFloor => "Four on the floor",
            Groove::BackbeatRock => "Backbeat rock",
            Groove::HalfTime => "Half-time",
            Groove::BoomBap => "Boom bap",
            Groove::TrapHats => "Trap hats",
            Groove::House => "House",
            Groove::Dembow => "Reggaeton / dembow",
            Groove::FillLastBar => "Fill last bar",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Groove::KickEveryBeat => "Adds a kick on every beat, keeping what is there",
            Groove::FourOnTheFloor => "Kick on every beat, clap on 2 and 4, eighth hats",
            Groove::BackbeatRock => "Rock beat: kick 1 and 3, snare backbeat, eighth hats",
            Groove::HalfTime => "Laid-back snare on beat 3 only",
            Groove::BoomBap => "Dusty hip-hop: syncopated kick, hard snare on 2 and 4",
            Groove::TrapHats => "Half-time clap with 16th hats rolling into the next bar",
            Groove::House => "Four-on-the-floor kick with offbeat open hats",
            Groove::Dembow => "Kick on the beat with the dembow snare figure",
            Groove::FillLastBar => "Snare and tom fill over the end of the last bar",
        }
    }

    /// Whether stamping clears the pattern first (the full grooves) rather
    /// than adding hits.
    pub fn replaces(self) -> bool {
        !matches!(self, Groove::KickEveryBeat | Groove::FillLastBar)
    }

    fn lines(self) -> &'static [GrooveLine] {
        use DrumPiece::*;
        match self {
            Groove::KickEveryBeat => &[(Kick, "x...x...x...x...")],
            Groove::FourOnTheFloor => &[
                (Kick, "X...x...X...x..."),
                (Clap, "....x.......x..."),
                (ClosedHat, "o.x.o.x.o.x.o.x."),
            ],
            Groove::BackbeatRock => &[
                (Kick, "X.......x.x....."),
                (Snare, "....X.......X..."),
                (ClosedHat, "x.o.x.o.x.o.x.o."),
            ],
            Groove::HalfTime => &[
                (Kick, "X.........x....."),
                (Snare, "........X......."),
                (ClosedHat, "x.o.x.o.x.o.x.o."),
            ],
            Groove::BoomBap => &[
                (Kick, "X......x..x....."),
                (Snare, "....X.......X..o"),
                (ClosedHat, "x.o.x.o.x.o.x.o."),
            ],
            Groove::TrapHats => &[
                (Kick, "X......x..x....."),
                (Clap, "........X......."),
                (ClosedHat, "xoxoxoxoxoxorrrr"),
            ],
            Groove::House => &[
                (Kick, "X...x...X...x..."),
                (Clap, "....x.......x..."),
                (OpenHat, "..x...x...x...x."),
                (ClosedHat, ".o.o.o.o.o.o.o.o"),
            ],
            Groove::Dembow => &[
                (Kick, "X...x...X...x..."),
                (Snare, "...x..x....x..x."),
                (ClosedHat, "o.o.o.o.o.o.o.o."),
            ],
            Groove::FillLastBar => &[
                (Kick, "........x......."),
                (Snare, "........ooxx...."),
                (HighTom, "............xX.."),
                (MidTom, "..............x."),
                (LowTom, "...............X"),
            ],
        }
    }
}

/// What [`stamp_groove`] did: hits written and the pieces the kit could
/// not play (no row and no stand-in), for the status line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrooveOutcome {
    pub hits: usize,
    pub missing: Vec<&'static str>,
}

/// Stamps `groove` into drum pattern `p`, whose kit rows are `rows`
/// (`(pitch, name)`, see [`Track::drum_rows`]). Pieces map onto the kit by
/// [`drum_piece_pitch`] (with stand-ins: a clap falls back to the snare,
/// toms to each other then the snare, …); pieces the kit cannot play are
/// skipped and reported. One-bar figures repeat on every bar of the
/// pattern and stop at its end; a pattern shorter than a bar gets the
/// figure's start. Full grooves ([`Groove::replaces`]) clear the pattern
/// first; [`Groove::KickEveryBeat`] adds kicks (on the top row when the
/// kit has no kick); [`Groove::FillLastBar`] clears and rewrites only the
/// last half bar, aligned to the pattern end. Character velocities scale
/// with `velocity` (0.8 leaves them as written).
pub fn stamp_groove(
    p: &mut Pattern,
    rows: &[(u8, &str)],
    groove: Groove,
    velocity: f32,
) -> GrooveOutcome {
    let mut out = GrooveOutcome::default();
    let len = p.length.max(1);
    let gain = if velocity.is_finite() {
        velocity / 0.8
    } else {
        1.0
    };
    let vel = |v: f32| (v * gain).clamp(0.05, 1.0);
    // (song-free) pattern steps each figure character lands on.
    let bar = STEPS_PER_BAR;
    let placements: Vec<(u32, usize)> = match groove {
        Groove::FillLastBar => {
            let bar_start = (len - 1) / bar * bar;
            let bar_len = (len - bar_start).min(bar);
            let from = bar_start + bar_len.saturating_sub(bar / 2);
            p.notes.retain(|n| n.start < from as f64);
            (0..bar as usize)
                .filter_map(|i| {
                    let step = (bar_start + bar_len + i as u32).checked_sub(bar)?;
                    (step >= from).then_some((step, i))
                })
                .collect()
        }
        _ => {
            if groove.replaces() {
                p.notes.clear();
            }
            (0..len).map(|s| (s, (s % bar) as usize)).collect()
        }
    };
    for &(piece, figure) in groove.lines() {
        let pitch = match resolve_piece(rows, piece) {
            Some(pitch) => pitch,
            None if groove == Groove::KickEveryBeat => match rows.first() {
                Some((pitch, _)) => *pitch,
                None => continue,
            },
            None => {
                if !out.missing.contains(&piece.label()) {
                    out.missing.push(piece.label());
                }
                continue;
            }
        };
        let chars = figure.as_bytes();
        for &(step, i) in &placements {
            let (v, roll) = match chars.get(i) {
                Some(b'X') => (1.0, false),
                Some(b'x') => (0.8, false),
                Some(b'o') => (0.5, false),
                Some(b'r') => (0.55, true),
                _ => continue,
            };
            if groove == Groove::KickEveryBeat {
                if set_drum(p, pitch, step, true, vel(v)) {
                    out.hits += 1;
                }
                continue;
            }
            let hits: &[(f64, f32)] = if roll {
                &[(0.0, 0.0), (0.5, 0.15)]
            } else {
                &[(0.0, 0.0)]
            };
            let dur = if roll { 0.5 } else { 1.0 };
            for &(offset, accent) in hits {
                if p.notes.len() >= MAX_NOTES_PER_PATTERN {
                    return out;
                }
                p.notes
                    .push(Note::new(pitch, step as f64 + offset, dur, vel(v + accent)));
                out.hits += 1;
            }
        }
    }
    p.sort_notes();
    out
}

// ----- song templates --------------------------------------------------------

/// What the New Song dialog's "Start from" choice builds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SongTemplate {
    /// One empty drum track — today's blank song.
    #[default]
    Empty,
    /// Drums and a bass groove.
    Beat,
    /// Verse/chorus pop arrangement on I–V–vi–IV.
    Pop,
    /// Slow, swung, jazzy sevenths.
    LoFi,
    /// 124 BPM four-on-the-floor.
    House,
    /// Chiptune instruments from the bundled 8-bit pack.
    EightBit,
}

impl SongTemplate {
    pub const ALL: [SongTemplate; 6] = [
        SongTemplate::Empty,
        SongTemplate::Beat,
        SongTemplate::Pop,
        SongTemplate::LoFi,
        SongTemplate::House,
        SongTemplate::EightBit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SongTemplate::Empty => "Empty",
            SongTemplate::Beat => "Beat",
            SongTemplate::Pop => "Pop song",
            SongTemplate::LoFi => "Lo-fi",
            SongTemplate::House => "House",
            SongTemplate::EightBit => "8-bit",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SongTemplate::Empty => "A blank song with one empty drum track.",
            SongTemplate::Beat => "Boom-bap drums and a bass groove — 8 bars at 92 BPM.",
            SongTemplate::Pop => {
                "Verse and chorus, I–V–vi–IV chords, drums, bass, keys and pad — 16 bars."
            }
            SongTemplate::LoFi => "Swung 78 BPM beat with jazzy seventh chords on keys.",
            SongTemplate::House => "124 BPM four-on-the-floor, octave bass and a pad.",
            SongTemplate::EightBit => "Chiptune lead, bass and drums from the 8-bit pack.",
        }
    }
}

/// Chord-lane progression (scale degree, quality) a template uses in `key`.
fn template_progression(template: SongTemplate, key: SongKey) -> Vec<ChordSpec> {
    use ChordQuality::*;
    let minor = key.mode == KeyMode::Minor;
    let steps: &[(usize, Option<ChordQuality>)] = match (template, minor) {
        (SongTemplate::Pop, false) => &[(0, None), (4, None), (5, None), (3, None)],
        (SongTemplate::Pop, true) => &[(0, None), (5, None), (2, None), (6, None)],
        (SongTemplate::LoFi, false) => &[
            (0, Some(Major7)),
            (5, Some(Minor7)),
            (1, Some(Minor7)),
            (4, Some(Dominant7)),
        ],
        (SongTemplate::LoFi, true) => &[
            (0, Some(Minor7)),
            (3, Some(Minor7)),
            (6, Some(Dominant7)),
            (2, Some(Major7)),
        ],
        (SongTemplate::House, false) => &[(5, Some(Minor7)), (3, None), (0, None), (4, None)],
        (SongTemplate::House, true) => &[(0, Some(Minor7)), (5, None), (2, None), (6, None)],
        (SongTemplate::EightBit, false) => &[(0, None), (5, None), (3, None), (4, None)],
        (SongTemplate::EightBit, true) => &[(0, None), (5, None), (6, None), (4, None)],
        _ => &[],
    };
    steps
        .iter()
        .map(|&(degree, quality)| {
            ChordSpec::new(
                key.degree_root(degree),
                quality.unwrap_or_else(|| key.degree_chord_quality(degree)),
            )
        })
        .collect()
}

/// A chord voiced around `center` in the plain root-position voicing
/// templates use (no inversion, no slash bass).
pub fn plain_voicing(spec: &ChordSpec, center: u8) -> Vec<u8> {
    let octave = i32::from(center) / 12 - 1;
    chord_midi_pitches(&chord_notes(spec, octave, 0.8), center)
}

/// A preset from the bundled 8-bit pack, for the 8-bit template.
fn eight_bit_preset(id: &str) -> Option<InstrumentPreset> {
    crate::packs::parse_pack(include_str!("../../../packs/8-bit/pack.json"))
        .ok()?
        .instruments
        .into_iter()
        .find(|p| p.id == id)
}

/// Adds a drum track playing `groove` as a one-bar loop for `bars` bars.
fn template_drums(project: &mut Project, groove: Groove, bars: u32) -> Option<u64> {
    let id = add_track(project, Instrument::Drums)?;
    let t = track_mut(project, id)?;
    let rows: Vec<(u8, String)> = t
        .drum_rows()
        .into_iter()
        .map(|(p, n)| (p, n.to_string()))
        .collect();
    let rows: Vec<(u8, &str)> = rows.iter().map(|(p, n)| (*p, n.as_str())).collect();
    let pat = t.patterns.first_mut()?;
    stamp_groove(pat, &rows, groove, 0.8);
    t.clips.first_mut()?.loops = bars.clamp(1, MAX_CLIP_LOOPS);
    Some(id)
}

/// Clears a new track's starter clip so a chord-lane part can take over.
fn drop_starter_clip(project: &mut Project, track_id: u64) {
    if let Some(t) = track_mut(project, track_id) {
        t.clips.clear();
    }
}

/// Builds the song `template` describes in `key`, from the same ops the
/// editor uses: tracks with built-in (or bundled-pack) instruments, starter
/// patterns, chord lane and markers. Always passes [`Project::validate`].
pub fn template_project(template: SongTemplate, key: SongKey) -> Project {
    let mut p = Project::empty();
    p.key = SongKey::new(key.root, key.mode);
    if template == SongTemplate::Empty {
        return p;
    }
    p.tracks.clear();
    let (bpm, swing, bars) = match template {
        SongTemplate::Beat => (92.0, 0.12, 8),
        SongTemplate::Pop => (108.0, 0.0, 16),
        SongTemplate::LoFi => (78.0, 0.3, 8),
        SongTemplate::House => (124.0, 0.0, 8),
        SongTemplate::EightBit => (140.0, 0.0, 8),
        SongTemplate::Empty => (110.0, 0.0, 8),
    };
    p.bpm = bpm;
    p.swing = swing;
    p.bars = bars;
    let bar = STEPS_PER_BAR as f64;
    let progression = template_progression(template, p.key);
    for b in 0..bars {
        if let Some(spec) = progression.get(b as usize % progression.len().max(1)) {
            add_chord_event(&mut p, f64::from(b) * bar, spec.root, spec.quality);
        }
    }
    let chords_on = |p: &mut Project, id: u64, center: u8, strum: Option<(f64, StrumDirection)>| {
        drop_starter_clip(p, id);
        let _ = play_chords_on_track(p, id, |s| plain_voicing(s, center), strum, 0.7);
    };
    let bass_on = |p: &mut Project, id: u64, style: BassStyle| {
        drop_starter_clip(p, id);
        let _ = bassline_from_chords(p, id, style, ChordBass::Off, 0.85);
    };
    match template {
        SongTemplate::Empty => {}
        SongTemplate::Beat => {
            template_drums(&mut p, Groove::BoomBap, bars);
            if let Some(id) = add_track(&mut p, Instrument::Bass) {
                let root = bass_pitch(p.key.root);
                let fifth = root + 7;
                if let Some(t) = track_mut(&mut p, id) {
                    if let Some(pat) = t.patterns.first_mut() {
                        for &(pitch, start, dur) in &[
                            (root, 0.0, 3.0),
                            (root, 7.0, 1.5),
                            (fifth, 10.0, 2.0),
                            (root + 12, 14.0, 1.0),
                        ] {
                            add_note(pat, pitch, start, dur, 0.85);
                        }
                    }
                    if let Some(c) = t.clips.first_mut() {
                        c.loops = bars;
                    }
                }
            }
        }
        SongTemplate::Pop => {
            add_marker(&mut p, 0.0, Some("Verse".into()));
            add_marker(&mut p, 8.0 * bar, Some("Chorus".into()));
            if let Some(id) = template_drums(&mut p, Groove::BackbeatRock, 7) {
                if let Some(t) = track_mut(&mut p, id) {
                    let rows: Vec<(u8, String)> = t
                        .drum_rows()
                        .into_iter()
                        .map(|(p, n)| (p, n.to_string()))
                        .collect();
                    let rows: Vec<(u8, &str)> =
                        rows.iter().map(|(p, n)| (*p, n.as_str())).collect();
                    // Bar 8: the verse beat with a fill into the chorus.
                    if let Some(fill) = new_pattern(t, 16) {
                        if let Some(pat) = t.pattern_mut(fill) {
                            stamp_groove(pat, &rows, Groove::BackbeatRock, 0.8);
                            stamp_groove(pat, &rows, Groove::FillLastBar, 0.8);
                            pat.name = "Fill".into();
                        }
                        t.clips.push(Clip {
                            id: next_id(),
                            pattern_id: fill,
                            start: 7.0 * bar,
                            loops: 1,
                            fade_in: 0.0,
                            fade_out: 0.0,
                        });
                    }
                    // Chorus: four on the floor with a crash on the downbeat.
                    if let Some(chorus) = new_pattern(t, 16) {
                        if let Some(pat) = t.pattern_mut(chorus) {
                            stamp_groove(pat, &rows, Groove::FourOnTheFloor, 0.8);
                            if let Some(crash) = resolve_piece(&rows, DrumPiece::Crash) {
                                set_drum(pat, crash, 0, true, 0.8);
                            }
                            pat.name = "Chorus".into();
                        }
                        t.clips.push(Clip {
                            id: next_id(),
                            pattern_id: chorus,
                            start: 8.0 * bar,
                            loops: 8,
                            fade_in: 0.0,
                            fade_out: 0.0,
                        });
                    }
                    if let Some(pat) = t.patterns.first_mut() {
                        pat.name = "Verse".into();
                    }
                }
            }
            if let Some(id) = add_track(&mut p, Instrument::Bass) {
                bass_on(&mut p, id, BassStyle::RootFifth);
            }
            if let Some(id) = add_track(&mut p, Instrument::Piano) {
                chords_on(&mut p, id, 62, None);
            }
            if let Some(id) = add_track(&mut p, Instrument::Pad) {
                if let Some(t) = track_mut(&mut p, id) {
                    t.volume = 0.38;
                }
                chords_on(&mut p, id, 67, None);
            }
        }
        SongTemplate::LoFi => {
            template_drums(&mut p, Groove::BoomBap, bars);
            if let Some(id) = add_track(&mut p, Instrument::Bass) {
                bass_on(&mut p, id, BassStyle::Roots);
            }
            if let Some(id) = add_track(&mut p, Instrument::Piano) {
                chords_on(&mut p, id, 62, Some((0.25, StrumDirection::Up)));
            }
        }
        SongTemplate::House => {
            template_drums(&mut p, Groove::House, bars);
            if let Some(id) = add_track(&mut p, Instrument::Bass) {
                bass_on(&mut p, id, BassStyle::OctaveBounce);
            }
            if let Some(id) = add_track(&mut p, Instrument::Pad) {
                chords_on(&mut p, id, 64, None);
            }
        }
        SongTemplate::EightBit => {
            template_drums(&mut p, Groove::BackbeatRock, bars);
            let bass = match eight_bit_preset("eight-bit.triangle-bass") {
                Some(preset) => add_preset_track(&mut p, &preset),
                None => add_track(&mut p, Instrument::Bass),
            };
            if let Some(id) = bass {
                bass_on(&mut p, id, BassStyle::OctaveBounce);
            }
            let keys = match eight_bit_preset("eight-bit.hollow-keys") {
                Some(preset) => add_preset_track(&mut p, &preset),
                None => add_track(&mut p, Instrument::Pluck),
            };
            if let Some(id) = keys {
                chords_on(&mut p, id, 64, None);
            }
        }
    }
    fit_bars_to_content(&mut p);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song_with_chords(chords: &[(f64, u8, ChordQuality)]) -> Project {
        let mut p = Project::empty();
        p.bars = 4;
        for &(step, root, q) in chords {
            add_chord_event(&mut p, step, root, q).unwrap();
        }
        p
    }

    fn voice(spec: &ChordSpec) -> Vec<u8> {
        plain_voicing(spec, 60)
    }

    #[test]
    fn chord_lane_spans_run_to_the_next_chord_and_the_song_end() {
        let mut p = song_with_chords(&[
            (16.0, 7, ChordQuality::Major),
            (0.0, 0, ChordQuality::Major),
            (48.0, 9, ChordQuality::Minor),
            (80.0, 5, ChordQuality::Major), // past the 4-bar end
        ]);
        let spans = chord_lane_spans(&p);
        assert_eq!(
            spans.iter().map(|s| (s.start, s.end)).collect::<Vec<_>>(),
            vec![(0.0, 16.0), (16.0, 48.0), (48.0, 64.0)]
        );
        assert_eq!(spans[2].spec, ChordSpec::new(9, ChordQuality::Minor));
        // Two events on one step: the later one wins.
        add_chord_event(&mut p, 0.0, 2, ChordQuality::Minor).unwrap();
        let spans = chord_lane_spans(&p);
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].spec, ChordSpec::new(2, ChordQuality::Minor));
    }

    #[test]
    fn play_chords_refuses_drums_audio_and_an_empty_lane() {
        let mut p = song_with_chords(&[]);
        let drums = p.tracks[0].id;
        let piano = add_track(&mut p, Instrument::Piano).unwrap();
        let audio = add_audio_track(&mut p).unwrap();
        let before = p.clone();
        assert_eq!(
            play_chords_on_track(&mut p, piano, voice, None, 0.8),
            Err(LanePartError::NoChords)
        );
        add_chord_event(&mut p, 0.0, 0, ChordQuality::Major);
        let before_chords = p.clone();
        assert_eq!(
            play_chords_on_track(&mut p, drums, voice, None, 0.8),
            Err(LanePartError::DrumTrack)
        );
        assert_eq!(
            play_chords_on_track(&mut p, audio, voice, None, 0.8),
            Err(LanePartError::AudioTrack)
        );
        assert_eq!(
            play_chords_on_track(&mut p, 12345, voice, None, 0.8),
            Err(LanePartError::NoTrack)
        );
        assert_eq!(p, before_chords, "refusals change nothing");
        assert_ne!(p, before);
        p.chord_events[0].step = 64.0;
        assert_eq!(
            play_chords_on_track(&mut p, piano, voice, None, 0.8),
            Err(LanePartError::ChordsPastEnd)
        );
    }

    #[test]
    fn play_chords_writes_every_chord_for_its_span_and_plays_across_the_song() {
        let mut p = song_with_chords(&[
            (16.0, 0, ChordQuality::Major),
            (32.0, 7, ChordQuality::Major),
        ]);
        let piano = add_track(&mut p, Instrument::Piano).unwrap();
        let part = play_chords_on_track(&mut p, piano, voice, None, 0.7).unwrap();
        assert_eq!(part.chords, 2);
        let t = track(&p, piano).unwrap();
        // One clip at the first chord's bar; the empty starter clip in bar 1
        // lies outside the chords and is left alone.
        assert_eq!(t.clips.len(), 2);
        let clip = t.clips.iter().find(|c| c.id == part.clip_id).unwrap();
        assert_eq!((clip.start, clip.pattern_id), (16.0, part.pattern_id));
        let pat = t.pattern(part.pattern_id).unwrap();
        assert_eq!(pat.name, SONG_CHORDS_PATTERN);
        assert_eq!(pat.length, 48, "bar 2 to the 4-bar song end");
        assert_eq!(t.clip_end(clip), p.total_steps());
        // C for one bar, then G held to the end — re-struck on each bar.
        let at = |s: f64| -> Vec<u8> {
            let mut v: Vec<u8> = pat
                .notes
                .iter()
                .filter(|n| n.start == s)
                .map(|n| n.pitch)
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(at(0.0), voice(&ChordSpec::new(0, ChordQuality::Major)));
        assert_eq!(at(16.0), voice(&ChordSpec::new(7, ChordQuality::Major)));
        assert_eq!(at(32.0), at(16.0));
        assert!(pat.notes.iter().all(|n| n.duration == 16.0));
        assert_eq!(part.notes, pat.notes.len());
        p.validate().unwrap();
    }

    #[test]
    fn play_chords_again_replaces_the_previous_part_but_keeps_user_clips() {
        let mut p = song_with_chords(&[(0.0, 0, ChordQuality::Major)]);
        let keys = add_track(&mut p, Instrument::Piano).unwrap();
        // A user pattern with notes stays where it is.
        let user_pid = p.tracks[1].patterns[0].id;
        add_note(
            pattern_mut(&mut p, keys, user_pid).unwrap(),
            72,
            0.0,
            1.0,
            0.8,
        );
        let first = play_chords_on_track(&mut p, keys, voice, None, 0.8).unwrap();
        p.chord_events[0].root = 9;
        let second = play_chords_on_track(&mut p, keys, voice, None, 0.8).unwrap();
        let t = track(&p, keys).unwrap();
        assert!(t.pattern(first.pattern_id).is_none(), "old part removed");
        assert!(t.pattern(second.pattern_id).is_some());
        assert_eq!(
            t.pattern(second.pattern_id).unwrap().name,
            SONG_CHORDS_PATTERN
        );
        assert!(t.clips.iter().any(|c| c.pattern_id == user_pid));
        assert_eq!(t.clips.len(), 2);
    }

    #[test]
    fn play_chords_strums_roll_inside_their_slot() {
        let mut p = song_with_chords(&[(0.0, 0, ChordQuality::Major)]);
        p.bars = 1;
        let keys = add_track(&mut p, Instrument::Guitar).unwrap();
        let part = play_chords_on_track(&mut p, keys, voice, Some((0.5, StrumDirection::Up)), 0.8)
            .unwrap();
        let pat = track(&p, keys).unwrap().pattern(part.pattern_id).unwrap();
        let starts: Vec<f64> = pat.notes.iter().map(|n| n.start).collect();
        assert_eq!(starts, vec![0.0, 0.5, 1.0]);
        assert!(pat.notes.iter().all(|n| n.end() <= 16.0 + 1e-9));
        assert_eq!(
            pat.notes.last().unwrap().end(),
            16.0,
            "the roll ends with the bar"
        );
    }

    #[test]
    fn bassline_styles_follow_the_chords_in_the_bass_register() {
        let chords = [
            (0.0, 0, ChordQuality::Major),
            (16.0, 11, ChordQuality::Minor),
        ];
        for style in BassStyle::ALL {
            let mut p = song_with_chords(&chords);
            p.bars = 2;
            let bass = add_track(&mut p, Instrument::Bass).unwrap();
            let part = bassline_from_chords(&mut p, bass, style, ChordBass::Off, 0.8).unwrap();
            let pat = track(&p, bass).unwrap().pattern(part.pattern_id).unwrap();
            assert_eq!(pat.name, BASSLINE_PATTERN);
            assert!(
                pat.notes.iter().all(|n| (31..=59).contains(&n.pitch)),
                "{style:?}"
            );
            // Every chord starts on its root.
            let first = |s: f64| pat.notes.iter().find(|n| n.start == s).unwrap().pitch;
            assert_eq!(first(0.0), 36, "{style:?} C2");
            assert_eq!(first(16.0), 47, "{style:?} B2");
            let count = pat.notes.len();
            match style {
                BassStyle::Roots => assert_eq!(count, 2),
                BassStyle::RootFifth => {
                    assert_eq!(count, 4);
                    assert_eq!(first(8.0), 43, "G2 is C's fifth");
                    assert_eq!(first(24.0), 42, "B's fifth folds down to F#2");
                }
                BassStyle::Eighths => {
                    assert_eq!(count, 16);
                    assert!(pat.notes[..8].iter().all(|n| n.pitch == 36));
                }
                BassStyle::OctaveBounce => {
                    assert_eq!(count, 16);
                    assert_eq!(first(2.0), 48);
                    assert_eq!(first(4.0), 36);
                }
            }
            assert!(pat.notes.iter().all(|n| n.end() <= 32.0));
            p.validate().unwrap();
        }
    }

    #[test]
    fn bassline_uses_the_slash_bass_degree() {
        let mut p = song_with_chords(&[(0.0, 5, ChordQuality::Major)]);
        p.bars = 1;
        let bass = add_track(&mut p, Instrument::Bass).unwrap();
        // F/C in C major: the tonic pedal under the F chord.
        let part = bassline_from_chords(&mut p, bass, BassStyle::Roots, ChordBass::Degree(0), 0.8)
            .unwrap();
        let pat = track(&p, bass).unwrap().pattern(part.pattern_id).unwrap();
        assert_eq!(pat.notes[0].pitch, 36);
    }

    fn builtin_rows() -> Vec<(u8, &'static str)> {
        crate::model::DRUMS.to_vec()
    }

    fn hits(p: &Pattern, pitch: u8) -> Vec<f64> {
        let mut v: Vec<f64> = p
            .notes
            .iter()
            .filter(|n| n.pitch == pitch)
            .map(|n| n.start)
            .collect();
        v.sort_by(f64::total_cmp);
        v
    }

    #[test]
    fn drum_pieces_map_by_pitch_then_name() {
        let rows = builtin_rows();
        assert_eq!(drum_piece_pitch(&rows, DrumPiece::Kick), Some(36));
        assert_eq!(drum_piece_pitch(&rows, DrumPiece::OpenHat), Some(46));
        assert_eq!(drum_piece_pitch(&rows, DrumPiece::HighTom), None);
        assert_eq!(resolve_piece(&rows, DrumPiece::HighTom), Some(45));
        let odd = [
            (60, "Big Kick"),
            (61, "Closed HH"),
            (62, "Open Hat"),
            (63, "Floor Tom"),
        ];
        assert_eq!(drum_piece_pitch(&odd, DrumPiece::Kick), Some(60));
        assert_eq!(drum_piece_pitch(&odd, DrumPiece::ClosedHat), Some(61));
        assert_eq!(drum_piece_pitch(&odd, DrumPiece::OpenHat), Some(62));
        assert_eq!(drum_piece_pitch(&odd, DrumPiece::LowTom), Some(63));
        assert_eq!(drum_piece_pitch(&odd, DrumPiece::MidTom), None);
        assert_eq!(drum_piece_pitch(&odd, DrumPiece::Snare), None);
    }

    #[test]
    fn grooves_repeat_per_bar_and_replace_the_pattern() {
        let rows = builtin_rows();
        let mut pat = create_pattern(Instrument::Drums, 32);
        pat.notes.push(Note::new(51, 3.0, 1.0, 0.8));
        let out = stamp_groove(&mut pat, &rows, Groove::FourOnTheFloor, 0.8);
        assert!(out.missing.is_empty());
        assert_eq!(out.hits, pat.notes.len());
        assert_eq!(
            hits(&pat, 36),
            vec![0.0, 4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 28.0]
        );
        assert_eq!(hits(&pat, 39), vec![4.0, 12.0, 20.0, 28.0]);
        assert!(hits(&pat, 51).is_empty(), "replaced the old hits");
        // Every groove stays inside the pattern.
        for g in Groove::ALL {
            let mut pat = create_pattern(Instrument::Drums, 8);
            stamp_groove(&mut pat, &rows, g, 0.8);
            assert!(
                pat.notes.iter().all(|n| n.start < 8.0 && n.velocity <= 1.0),
                "{g:?}"
            );
            assert!(!pat.notes.is_empty(), "{g:?}");
        }
    }

    #[test]
    fn grooves_skip_missing_pieces_and_report_them() {
        let rows = [(36, "Kick"), (38, "Snare")];
        let mut pat = create_pattern(Instrument::Drums, 16);
        let out = stamp_groove(&mut pat, &rows, Groove::House, 0.8);
        assert_eq!(out.missing, vec!["open hat", "closed hat"]);
        // The clap falls back to the snare.
        assert_eq!(hits(&pat, 38), vec![4.0, 12.0]);
        assert_eq!(hits(&pat, 36).len(), 4);
    }

    #[test]
    fn trap_hats_roll_in_32nds() {
        let mut pat = create_pattern(Instrument::Drums, 16);
        stamp_groove(&mut pat, &builtin_rows(), Groove::TrapHats, 0.8);
        let hats = hits(&pat, 42);
        assert_eq!(hats.len(), 12 + 8);
        assert!(hats.contains(&12.5) && hats.contains(&15.5));
    }

    #[test]
    fn kick_every_beat_adds_and_fill_rewrites_only_the_last_half_bar() {
        let rows = builtin_rows();
        let mut pat = create_pattern(Instrument::Drums, 32);
        pat.notes.push(Note::new(42, 2.0, 1.0, 0.8));
        pat.notes.push(Note::new(42, 28.0, 1.0, 0.8));
        stamp_groove(&mut pat, &rows, Groove::KickEveryBeat, 0.8);
        assert_eq!(hits(&pat, 36).len(), 8);
        assert_eq!(hits(&pat, 42), vec![2.0, 28.0], "kicks are added");
        stamp_groove(&mut pat, &rows, Groove::FillLastBar, 0.8);
        assert_eq!(hits(&pat, 42), vec![2.0], "the fill clears its half bar");
        assert_eq!(hits(&pat, 38), vec![24.0, 25.0, 26.0, 27.0]);
        // Built-in kit: every tom is the one low tom.
        assert_eq!(hits(&pat, 45), vec![28.0, 29.0, 30.0, 31.0]);
        assert!(pat.notes.iter().all(|n| n.start < 32.0));
        // A kit without a kick puts "Kick on every beat" on its top row.
        let mut pat = create_pattern(Instrument::Drums, 16);
        stamp_groove(&mut pat, &[(70, "Cowbell")], Groove::KickEveryBeat, 0.8);
        assert_eq!(hits(&pat, 70), vec![0.0, 4.0, 8.0, 12.0]);
    }

    #[test]
    fn every_template_builds_a_valid_song_in_every_key() {
        for template in SongTemplate::ALL {
            for key in [
                SongKey::new(0, KeyMode::Major),
                SongKey::new(9, KeyMode::Minor),
            ] {
                let p = template_project(template, key);
                p.validate()
                    .unwrap_or_else(|e| panic!("{template:?} in {key:?}: {e}"));
                assert_eq!(p.key, key);
                assert!(p.tracks.iter().all(|t| t.font.is_none()));
                if template != SongTemplate::Empty {
                    assert!(p.tracks.len() >= 2, "{template:?}");
                    let notes: usize = p
                        .tracks
                        .iter()
                        .flat_map(|t| &t.patterns)
                        .map(|pat| pat.notes.len())
                        .sum();
                    assert!(notes > 0, "{template:?} has starter patterns");
                    // Everything plays inside the song.
                    assert!(content_end(&p) <= p.total_steps(), "{template:?}");
                }
            }
        }
        assert_eq!(
            template_project(SongTemplate::Empty, SongKey::default())
                .tracks
                .len(),
            1
        );
    }

    #[test]
    fn template_details() {
        let pop = template_project(SongTemplate::Pop, SongKey::new(7, KeyMode::Major));
        let names: Vec<&str> = pop.markers.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Verse", "Chorus"]);
        let roots: Vec<u8> = pop.chord_events.iter().take(4).map(|e| e.root).collect();
        assert_eq!(roots, vec![7, 2, 4, 0], "I–V–vi–IV in G");
        assert_eq!(pop.chord_events.len(), 16);
        assert_eq!(pop.tracks.len(), 4);
        let house = template_project(SongTemplate::House, SongKey::default());
        assert_eq!(house.bpm, 124.0);
        let drums = &house.tracks[0];
        assert_eq!(hits(&drums.patterns[0], 36), vec![0.0, 4.0, 8.0, 12.0]);
        let lofi = template_project(SongTemplate::LoFi, SongKey::default());
        assert!(lofi.bpm < 90.0 && lofi.swing > 0.0);
        assert!(lofi
            .chord_events
            .iter()
            .all(|e| e.quality.intervals().len() == 4));
        let chip = template_project(SongTemplate::EightBit, SongKey::default());
        assert!(
            chip.tracks.iter().filter(|t| t.preset.is_some()).count() >= 2,
            "uses the bundled 8-bit pack"
        );
    }
}
