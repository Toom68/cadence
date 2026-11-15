//! Per-track SoundFont synthesizers driven by the [`Renderer`](super::renderer::Renderer).
//!
//! A [`FontSynth`] wraps one `rustysynth` [`Synthesizer`] playing a single GM
//! program. Synths are keyed by *track id* (not strip position) so they survive
//! track reordering on song load exactly like voices do — except no remap is
//! needed because the renderer resolves strip → track id → synth at render
//! time. The editor preview strip gets a dedicated synth slot.
//!
//! Real-time notes: `rustysynth` preallocates its voice pool inside
//! `Synthesizer::new`, so `note_on`/`note_off`/`render` do not allocate.
//! Synthesizers themselves are created lazily on the audio thread the first
//! time a font track plays (a bounded, one-time cost per track/font, like a
//! `Voice::set_sample_rate` reallocation); everything else — note gates,
//! program changes, mixing — allocates nothing.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;

use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

use super::renderer::{MAX_BLOCK, PREVIEW_STRIP, STRIPS};
use super::song::TrackParams;
use super::voice::FontKey;
use crate::font::FontLibrary;

/// MIDI channel every font synth uses. Channel 9 is GM percussion; melodic
/// programs live on channel 0.
const FONT_CHANNEL: i32 = 0;
/// rustysynth master volume, matched to `InstrumentPreset`'s default gain so
/// sampled instruments sit at roughly preset loudness before the fader.
const FONT_MASTER_VOLUME: f32 = 0.9;
/// rustysynth polyphony per synth instance.
const FONT_POLYPHONY: usize = 64;
/// Initial capacity of the held-note list (notes are `Copy`, gate-scheduled).
const NOTE_CAPACITY: usize = 32;

/// One held font note awaiting its gate end. Mirrors `Voice::gate`: the count
/// decrements per rendered sample and `note_off` fires when it reaches zero,
/// split sample-accurately between synth render pieces.
#[derive(Clone, Copy, Debug)]
struct FontNote {
    pitch: u8,
    gate: u64,
    from_song: bool,
}

/// A `rustysynth` instance playing one GM program for one track.
pub(crate) struct FontSynth {
    /// The parsed font, kept for identity checks across library reloads
    /// (`Arc::ptr_eq` tells "same file reparsed" from "same instance").
    font: Arc<SoundFont>,
    font_key: u64,
    program: u8,
    synth: Synthesizer,
    notes: Vec<FontNote>,
    /// The synth may still produce audio: held notes or a release tail. Set on
    /// note-on, cleared when a whole rendered segment comes out silent — rustysynth
    /// exposes no voice count, so output silence is the honest "done" signal.
    sounding: bool,
}

impl FontSynth {
    /// Creates a synth for `font`/`program` at `sample_rate`. Fails only when
    /// the rate is outside rustysynth's 16–192 kHz range (the renderer allows
    /// 8–384 kHz); callers treat failure as "font unavailable" and fall back.
    fn new(
        font: &Arc<SoundFont>,
        font_key: u64,
        sample_rate: f32,
        program: u8,
    ) -> Result<Self, String> {
        let rate = sample_rate.round();
        if !(16_000.0..=192_000.0).contains(&rate) {
            return Err(format!(
                "sample rate {rate} Hz is outside the SoundFont range (16–192 kHz)"
            ));
        }
        let mut settings = SynthesizerSettings::new(rate as i32);
        settings.block_size = MAX_BLOCK;
        settings.maximum_polyphony = FONT_POLYPHONY;
        // Cadence has its own per-track reverb/delay/drive; the synth's internal
        // FX would double up, so they stay off.
        settings.enable_reverb_and_chorus = false;
        let mut synth = Synthesizer::new(font, &settings).map_err(|e| e.to_string())?;
        synth.set_master_volume(FONT_MASTER_VOLUME);
        select_program(&mut synth, program);
        Ok(FontSynth {
            font: Arc::clone(font),
            font_key,
            program,
            synth,
            notes: Vec::with_capacity(NOTE_CAPACITY),
            sounding: false,
        })
    }

    /// Points the synth at `font`/`program`, recreating it when the font
    /// instance changed (library reload, different file) and sending a plain
    /// program change when only the program did. Playing voices are unaffected
    /// by a program change — rustysynth applies it to subsequent note-ons,
    /// matching MIDI semantics.
    fn configure(
        &mut self,
        font: &Arc<SoundFont>,
        font_key: u64,
        sample_rate: f32,
        program: u8,
    ) -> bool {
        if self.font_key != font_key || !Arc::ptr_eq(&self.font, font) {
            match Self::new(font, font_key, sample_rate, program) {
                Ok(fresh) => *self = fresh,
                Err(_) => return false,
            }
        } else if self.program != program {
            select_program(&mut self.synth, program);
            self.program = program;
        }
        true
    }

    /// Starts a note. A same-pitch retrigger releases the held entry first —
    /// the same same-pitch release the voice path applies — so the old note's
    /// gate can never kill the new note later.
    fn note_on(&mut self, pitch: u8, velocity: f32, gate: u64, from_song: bool) {
        if let Some(i) = self.notes.iter().position(|n| n.pitch == pitch) {
            self.notes.remove(i);
            self.synth.note_off(FONT_CHANNEL, pitch as i32);
        }
        let vel = (velocity.clamp(0.0, 1.0) * 127.0).round() as i32;
        self.synth
            .note_on(FONT_CHANNEL, pitch as i32, vel.clamp(1, 127));
        self.notes.push(FontNote {
            pitch,
            gate: gate.max(1),
            from_song,
        });
        self.sounding = true;
    }

    /// Key-up equivalent for pause/play: releases every note (natural
    /// release tail keeps rendering through `sounding`).
    fn release_all(&mut self) {
        self.synth.note_off_all(false);
        self.notes.retain(|n| !n.from_song);
    }

    /// Immediate silence for stop/panic/track deletion.
    fn reset(&mut self) {
        self.synth.note_off_all(true);
        self.notes.clear();
        self.sounding = false;
    }

    /// Renders `[from, to)` into the strip's `MAX_BLOCK` buffers at absolute
    /// positions, accumulating like voices do. The synth is rendered in pieces
    /// split at gate ends so `note_off` lands on the exact sample; `scratch_*`
    /// are renderer-owned `MAX_BLOCK` buffers (synth output is assigned, then
    /// mixed in, so scratch needs no clearing).
    fn render_into(
        &mut self,
        strip_l: &mut [f32],
        strip_r: &mut [f32],
        scratch_l: &mut [f32],
        scratch_r: &mut [f32],
        from: usize,
        to: usize,
    ) {
        let mut cursor = from;
        let mut any = false;
        while cursor < to {
            let remaining = (to - cursor) as u64;
            // Samples until the earliest gate end inside this piece (≥ 1).
            let mut piece = remaining;
            for n in &self.notes {
                piece = piece.min(n.gate);
            }
            let piece_len = piece.clamp(1, remaining) as usize;
            self.synth
                .render(&mut scratch_l[..piece_len], &mut scratch_r[..piece_len]);
            for i in 0..piece_len {
                let (sl, sr) = (scratch_l[i], scratch_r[i]);
                any |= sl != 0.0 || sr != 0.0;
                strip_l[cursor + i] += sl;
                strip_r[cursor + i] += sr;
            }
            for n in &mut self.notes {
                n.gate = n.gate.saturating_sub(piece_len as u64);
            }
            // Gates that reached zero fire note_off now, before the next piece.
            let mut w = 0;
            for i in 0..self.notes.len() {
                if self.notes[i].gate == 0 {
                    self.synth
                        .note_off(FONT_CHANNEL, self.notes[i].pitch as i32);
                } else {
                    self.notes[w] = self.notes[i];
                    w += 1;
                }
            }
            self.notes.truncate(w);
            cursor += piece_len;
        }
        if self.notes.is_empty() && !any {
            self.sounding = false;
        }
    }
}

/// GM program selection on channel 0: bank 0 (reset explicitly so a reused
/// synth never inherits a stale bank) plus the program change.
fn select_program(synth: &mut Synthesizer, program: u8) {
    synth.process_midi_message(FONT_CHANNEL, 0xB0, 0x00, 0);
    synth.process_midi_message(FONT_CHANNEL, 0xC0, program as i32, 0);
}

/// The font registry the renderer uses: the shared [`FontLibrary`] snapshot
/// plus live per-track synths.
pub(crate) struct FontBank {
    library: Arc<FontLibrary>,
    sample_rate: f32,
    /// Song-track synths keyed by `TrackParams::id`.
    synths: HashMap<u64, Box<FontSynth>>,
    /// Synth for the editor preview strip.
    preview: Option<Box<FontSynth>>,
}

impl FontBank {
    pub(crate) fn new(sample_rate: f32, library: Arc<FontLibrary>) -> Self {
        FontBank {
            library,
            sample_rate,
            synths: HashMap::new(),
            preview: None,
        }
    }

    pub(crate) fn library(&self) -> &Arc<FontLibrary> {
        &self.library
    }

    /// Swaps the font catalog. Existing synths keep their `Arc<SoundFont>`
    /// and stay valid; `configure` re-creates them on the next note when the
    /// library hands out a different font instance for the same key.
    pub(crate) fn set_library(&mut self, library: Arc<FontLibrary>) {
        self.library = library;
    }

    /// Routes a `Sound::Font` note to the right synth. Returns `false` when
    /// the font is not installed (or cannot run at this sample rate) so the
    /// caller falls back to the track's fallback sound.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn note_on(
        &mut self,
        preview: bool,
        track_id: u64,
        key: FontKey,
        pitch: u8,
        velocity: f32,
        gate: u64,
        from_song: bool,
    ) -> bool {
        let Some(font) = self.library.get(key.font_key).cloned() else {
            return false;
        };
        let Some(synth) = self.slot(preview, track_id, &font, key) else {
            return false;
        };
        synth.note_on(pitch, velocity, gate, from_song);
        true
    }

    /// Get-or-create the synth for a slot, then point it at `font`/`program`.
    fn slot(
        &mut self,
        preview: bool,
        track_id: u64,
        font: &Arc<SoundFont>,
        key: FontKey,
    ) -> Option<&mut FontSynth> {
        if preview {
            if self.preview.is_none() {
                self.preview = FontSynth::new(font, key.font_key, self.sample_rate, key.program)
                    .ok()
                    .map(Box::new);
            }
            let synth = self.preview.as_deref_mut()?;
            synth
                .configure(font, key.font_key, self.sample_rate, key.program)
                .then_some(synth)
        } else {
            let synth = match self.synths.entry(track_id) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => {
                    match FontSynth::new(font, key.font_key, self.sample_rate, key.program) {
                        Ok(s) => e.insert(Box::new(s)),
                        Err(_) => return None,
                    }
                }
            };
            synth
                .configure(font, key.font_key, self.sample_rate, key.program)
                .then_some(synth)
        }
    }

    /// Renders every sounding synth into the strip buffers for `[from, to)`,
    /// marking strips used so they receive track gain/pan/FX exactly like
    /// voice audio. `scratch_l`/`scratch_r` are renderer-owned `MAX_BLOCK`
    /// buffers reused across strips.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render(
        &mut self,
        tracks: &[TrackParams],
        strip_l: &mut [f32],
        strip_r: &mut [f32],
        strip_used: &mut [bool; STRIPS],
        scratch_l: &mut [f32],
        scratch_r: &mut [f32],
        from: usize,
        to: usize,
    ) {
        if self.synths.is_empty() && self.preview.is_none() {
            return;
        }
        for (s, used) in strip_used.iter_mut().enumerate() {
            let synth = if s == PREVIEW_STRIP {
                self.preview.as_deref_mut()
            } else {
                tracks
                    .get(s)
                    .and_then(|t| self.synths.get_mut(&t.id))
                    .map(|b| &mut **b)
            };
            let Some(synth) = synth else { continue };
            if !synth.sounding {
                continue;
            }
            *used = true;
            let base = s * MAX_BLOCK;
            synth.render_into(
                &mut strip_l[base..base + MAX_BLOCK],
                &mut strip_r[base..base + MAX_BLOCK],
                scratch_l,
                scratch_r,
                from,
                to,
            );
        }
    }

    /// Mirrors `Renderer::release_song_voices` (pause/play): releases song
    /// notes so they ring out naturally; the preview synth is untouched.
    pub(crate) fn release_song_voices(&mut self) {
        for synth in self.synths.values_mut() {
            synth.release_all();
        }
    }

    /// Mirrors `Renderer::hard_reset` (stop/panic): kills every font note.
    pub(crate) fn hard_reset(&mut self) {
        for synth in self.synths.values_mut() {
            synth.reset();
        }
        if let Some(preview) = &mut self.preview {
            preview.reset();
        }
    }

    /// Drops synths whose track id is gone from the newly installed song —
    /// the font counterpart of the voice cleanup in `Renderer::load`. Id-keyed
    /// synths need no remap when tracks are reordered.
    pub(crate) fn retain_tracks(&mut self, tracks: &[TrackParams]) {
        self.synths
            .retain(|id, _| tracks.iter().any(|t| t.id == *id));
    }

    /// Whether the preview-strip synth may still be producing audio — used by
    /// `Renderer::preview` for the same snap decision voices get.
    pub(crate) fn preview_sounding(&self) -> bool {
        self.preview.as_ref().is_some_and(|p| p.sounding)
    }

    /// Held font notes across all synths, for `active_voices` accounting.
    /// Release tails are not counted (same approximation as the meter).
    pub(crate) fn active_notes(&self) -> usize {
        self.synths.values().map(|s| s.notes.len()).sum::<usize>()
            + self.preview.as_ref().map_or(0, |p| p.notes.len())
    }
}
