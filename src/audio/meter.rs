//! Mixer meter taps. The callback publishes one complete buffer at a time.

use crate::model::{MAX_TRACKS, NUM_BUSES};
use std::sync::atomic::{fence, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::song::TrackParams;

/// Peak (maximum absolute sample across L/R) at a track's output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPeak {
    pub track_id: u64,
    pub peak: f32,
}

/// Post-insert, post-fader linear peaks from the last audio callback.
/// Shared reverb/delay returns and master gain/limiting are not included.
/// Values are not clamped to unity and have no UI decay applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixerPeaks {
    /// Order follows the renderer's current snapshot, not necessarily the
    /// UI project. Always match `track_id`; unused slots are `None`.
    pub tracks: [Option<TrackPeak>; MAX_TRACKS],
    /// `buses[i]` corresponds to `Project::buses[i]` (route `i + 1`).
    pub buses: [f32; NUM_BUSES],
}

impl MixerPeaks {
    /// `None` if the track is not present in the renderer's current snapshot.
    pub fn track(&self, track_id: u64) -> Option<f32> {
        self.tracks
            .iter()
            .flatten()
            .find(|p| p.track_id == track_id)
            .map(|p| p.peak)
    }
}

#[derive(Default)]
pub(crate) struct BlockPeaks {
    pub tracks: [f32; MAX_TRACKS],
    pub buses: [f32; NUM_BUSES],
}

#[derive(Default)]
pub(super) struct SharedMeter {
    sequence: AtomicU32,
    len: AtomicUsize,
    ids: [AtomicU64; MAX_TRACKS],
    peaks: [AtomicU32; MAX_TRACKS],
    buses: [AtomicU32; NUM_BUSES],
}

impl SharedMeter {
    // Single writer (the current callback, or the stream owner after it
    // drops the callback). The sequence prevents pairing a peak with a
    // different track's ID across an insert/delete/reorder.
    pub(super) fn publish(&self, tracks: &[TrackParams], peaks: &BlockPeaks) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.len.store(tracks.len(), Ordering::Relaxed);
        for (i, track) in tracks.iter().enumerate() {
            self.ids[i].store(track.id, Ordering::Relaxed);
            self.peaks[i].store(peaks.tracks[i].to_bits(), Ordering::Relaxed);
        }
        for (slot, peak) in self.buses.iter().zip(peaks.buses) {
            slot.store(peak.to_bits(), Ordering::Relaxed);
        }
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub(super) fn read(&self) -> Option<MixerPeaks> {
        for _ in 0..3 {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                continue;
            }
            let mut levels = MixerPeaks::default();
            for i in 0..self.len.load(Ordering::Relaxed) {
                levels.tracks[i] = Some(TrackPeak {
                    track_id: self.ids[i].load(Ordering::Relaxed),
                    peak: f32::from_bits(self.peaks[i].load(Ordering::Relaxed)),
                });
            }
            for (peak, slot) in levels.buses.iter_mut().zip(&self.buses) {
                *peak = f32::from_bits(slot.load(Ordering::Relaxed));
            }
            // Complete the field reads before validating the sequence.
            fence(Ordering::Acquire);
            if before == self.sequence.load(Ordering::Relaxed) {
                return Some(levels);
            }
        }
        None
    }
}
