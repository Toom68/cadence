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
/// Shared reverb/delay returns and master gain/limiting are not included in
/// the track/bus peaks. Values are not clamped to unity and have no UI decay
/// applied. The master readings (`master_rms`, `glue_reduction_db`) are taken
/// at the output, after the limiter.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MixerPeaks {
    /// Order follows the renderer's current snapshot, not necessarily the
    /// UI project. Always match `track_id`; unused slots are `None`.
    pub tracks: [Option<TrackPeak>; MAX_TRACKS],
    /// `buses[i]` corresponds to `Project::buses[i]` (route `i + 1`).
    pub buses: [f32; NUM_BUSES],
    /// Linear RMS of the master output (L/R power averaged) over a ~300 ms
    /// window — a steadier "how loud" reading next to the peak meter.
    pub master_rms: f32,
    /// Gain reduction of the master glue compressor in dB (>= 0).
    pub glue_reduction_db: f32,
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
    pub master_rms: f32,
    pub glue_reduction_db: f32,
}

#[derive(Default)]
pub(super) struct SharedMeter {
    sequence: AtomicU32,
    len: AtomicUsize,
    ids: [AtomicU64; MAX_TRACKS],
    peaks: [AtomicU32; MAX_TRACKS],
    buses: [AtomicU32; NUM_BUSES],
    master_rms: AtomicU32,
    glue_reduction_db: AtomicU32,
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
        self.master_rms
            .store(peaks.master_rms.to_bits(), Ordering::Relaxed);
        self.glue_reduction_db
            .store(peaks.glue_reduction_db.to_bits(), Ordering::Relaxed);
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
            levels.master_rms = f32::from_bits(self.master_rms.load(Ordering::Relaxed));
            levels.glue_reduction_db =
                f32::from_bits(self.glue_reduction_db.load(Ordering::Relaxed));
            // Complete the field reads before validating the sequence.
            fence(Ordering::Acquire);
            if before == self.sequence.load(Ordering::Relaxed) {
                return Some(levels);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_readings_round_trip_through_the_shared_meter() {
        let meter = SharedMeter::default();
        let peaks = BlockPeaks {
            master_rms: 0.25,
            glue_reduction_db: 1.5,
            ..BlockPeaks::default()
        };
        meter.publish(&[], &peaks);
        let read = meter.read().unwrap();
        assert_eq!(read.master_rms, 0.25);
        assert_eq!(read.glue_reduction_db, 1.5);
        meter.publish(&[], &BlockPeaks::default());
        let read = meter.read().unwrap();
        assert_eq!((read.master_rms, read.glue_reduction_db), (0.0, 0.0));
    }
}
