use kithara_bufpool::{SampleBuffer, SamplePool};
use num_traits::cast::AsPrimitive;

use crate::{api::BeatError, config::BeatConfig};

/// Frames per second of the beat model output.
const FPS: f32 = 50.0;

/// Decodes raw beat/downbeat logits into timestamped events: max-pool peak
/// picking, thresholding, deduplication, downbeat-to-beat snapping.
pub(crate) struct PeakPicker {
    config: BeatConfig,
}

impl PeakPicker {
    pub(crate) fn new(config: BeatConfig) -> Self {
        Self { config }
    }

    /// Decode beat and downbeat logits into `(beats, downbeats)` in seconds.
    ///
    /// Both slices must have the same length (one value per mel frame).
    pub(crate) fn decode(
        &self,
        beat_logits: &[f32],
        downbeat_logits: &[f32],
        sample_pool: &SamplePool,
    ) -> Result<(SampleBuffer, SampleBuffer), BeatError> {
        if beat_logits.len() != downbeat_logits.len() {
            return Err(BeatError::Inference {
                reason: format!(
                    "beat_logits length ({}) != downbeat_logits length ({})",
                    beat_logits.len(),
                    downbeat_logits.len()
                ),
            });
        }

        let beats = find_peak_times(beat_logits, &self.config, sample_pool);
        let mut downbeats = find_peak_times(downbeat_logits, &self.config, sample_pool);

        snap_downbeats_to_beats(&beats, &mut downbeats);

        Ok((beats, downbeats))
    }
}

/// Identify local maxima exceeding [`BeatConfig::peak_threshold`].
///
/// Max-pool window of `2 * peak_half_width + 1` frames, stride 1: a frame is a
/// peak if it equals the local maximum and clears the threshold.
fn find_peak_times(logits: &[f32], config: &BeatConfig, sample_pool: &SamplePool) -> SampleBuffer {
    sample_pool.get_with(|times| {
        times.clear();
        let peaks = (0..logits.len()).filter(|&i| {
            let start = i.saturating_sub(config.peak_half_width);
            let end = (i + config.peak_half_width + 1).min(logits.len());
            logits[i] > config.peak_threshold
                && !logits[start..end].iter().any(|&value| value > logits[i])
        });
        visit_deduplicated_peaks(peaks, config.dedup_width, |mean| {
            times.push((mean / f64::from(FPS)).as_());
        });
    })
}

fn visit_deduplicated_peaks(
    mut peaks: impl Iterator<Item = usize>,
    width: usize,
    mut visit: impl FnMut(f64),
) {
    let Some(first) = peaks.next() else {
        return;
    };
    let mut mean: f64 = first.as_();
    let mut count = 1.0_f64;
    for peak in peaks {
        let peak: f64 = peak.as_();
        if peak - mean <= width.as_() {
            count += 1.0;
            mean += (peak - mean) / count;
        } else {
            visit(mean);
            mean = peak;
            count = 1.0;
        }
    }
    visit(mean);
}

#[cfg(test)]
fn find_peaks(logits: &[f32], config: &BeatConfig) -> Vec<f64> {
    let peaks = (0..logits.len()).filter(|&i| {
        let start = i.saturating_sub(config.peak_half_width);
        let end = (i + config.peak_half_width + 1).min(logits.len());
        logits[i] > config.peak_threshold
            && !logits[start..end].iter().any(|&value| value > logits[i])
    });
    let mut means = Vec::new();
    visit_deduplicated_peaks(peaks, config.dedup_width, |mean| means.push(mean));
    means
}

#[cfg(test)]
fn deduplicate_peaks(peaks: &[usize], width: usize) -> Vec<f64> {
    let mut result = Vec::new();
    visit_deduplicated_peaks(peaks.iter().copied(), width, |mean| result.push(mean));
    result
}

fn snap_downbeats_to_beats(beat_times: &[f32], downbeat_times: &mut SampleBuffer) {
    if beat_times.is_empty() || downbeat_times.is_empty() {
        return;
    }

    for d_time in downbeat_times.iter_mut() {
        let pos = beat_times.partition_point(|&b| b < *d_time);

        let best = match (pos.checked_sub(1), beat_times.get(pos)) {
            (Some(before), Some(&after)) => {
                if (*d_time - beat_times[before]).abs() <= (after - *d_time).abs() {
                    beat_times[before]
                } else {
                    after
                }
            }
            (Some(before), None) => beat_times[before],
            (None, Some(&after)) => after,
            (None, None) => continue,
        };

        *d_time = best;
    }

    downbeat_times.sort_by(f32::total_cmp);
    downbeat_times.dedup();
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    fn samples(values: impl IntoIterator<Item = f32>) -> SampleBuffer {
        SamplePool::default().collect(values)
    }

    /// The threshold decides which model outputs become beats at all. Raising
    /// it past a peak's logit drops that beat; the picker must not carry a
    /// fixed sensitivity the caller cannot move.
    #[kithara::test(native, flash(false))]
    fn a_raised_threshold_drops_a_weak_peak() {
        let logits = [0.0, 0.0, 0.5, 1.0, 0.5, 0.0, 0.0];
        let strict = BeatConfig::builder().peak_threshold(1.5).build();

        assert_eq!(find_peaks(&logits, &BeatConfig::default()), vec![3.0]);
        assert!(find_peaks(&logits, &strict).is_empty());
    }

    /// The max-pool half-width is the shortest gap two beats may be reported
    /// at. Widening it must suppress a smaller neighbour the default keeps.
    #[kithara::test(native, flash(false))]
    fn a_wider_window_suppresses_a_neighbour_the_default_keeps() {
        // 4 frames apart: each wins its own +-3 window, neither wins a +-4 one.
        let mut logits = vec![0.0; 10];
        logits[2] = 2.0;
        logits[6] = 1.0;
        let wide = BeatConfig::builder().peak_half_width(4).build();

        assert_eq!(find_peaks(&logits, &BeatConfig::default()), vec![2.0, 6.0]);
        assert_eq!(find_peaks(&logits, &wide), vec![2.0]);
    }

    /// Dedup width is how far apart two surviving peaks still count as one
    /// beat. A wider one must merge peaks the default reports separately.
    #[kithara::test(native, flash(false))]
    fn a_wider_dedup_merges_peaks_the_default_reports_apart() {
        let peaks = [10, 14];

        assert_eq!(deduplicate_peaks(&peaks, 1), vec![10.0, 14.0]);
        assert_eq!(deduplicate_peaks(&peaks, 4), vec![12.0]);
    }

    #[kithara::test(native, flash(false))]
    fn find_peaks_single_peak() {
        let logits = [0.0, 0.0, 0.5, 1.0, 0.5, 0.0, 0.0];
        let peaks = find_peaks(&logits, &BeatConfig::default());
        assert_eq!(peaks, vec![3.0]);
    }

    #[kithara::test(native, flash(false))]
    fn find_peaks_below_threshold() {
        let logits = [-1.0, -0.5, -2.0, -0.1];
        let peaks = find_peaks(&logits, &BeatConfig::default());
        assert!(peaks.is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn find_peaks_multiple_peaks() {
        // Two peaks separated by more than 3 frames.
        let mut logits = vec![0.0; 20];
        logits[3] = 2.0;
        logits[15] = 1.5;
        let peaks = find_peaks(&logits, &BeatConfig::default());
        assert_eq!(peaks, vec![3.0, 15.0]);
    }

    #[kithara::test(native, flash(false))]
    fn find_peaks_window_suppresses_smaller_neighbour() {
        // A smaller positive value 3 frames from a larger one is not a peak.
        let mut logits = vec![0.0; 10];
        logits[4] = 2.0;
        logits[7] = 1.0;
        let peaks = find_peaks(&logits, &BeatConfig::default());
        assert_eq!(peaks, vec![4.0]);
    }

    #[kithara::test(native, flash(false))]
    fn find_peaks_outside_window_both_survive() {
        // 4 frames apart: each is the max of its own ±3 window.
        let mut logits = vec![0.0; 10];
        logits[2] = 2.0;
        logits[6] = 1.0;
        let peaks = find_peaks(&logits, &BeatConfig::default());
        assert_eq!(peaks, vec![2.0, 6.0]);
    }

    #[kithara::test(native, flash(false))]
    fn find_peaks_plateau_collapses_to_centre() {
        // Adjacent frames with equal positive values: both tie the max-pool,
        // dedup merges them to the plateau centre.
        let logits = [0.0, 1.0, 1.0, 0.0];
        let peaks = find_peaks(&logits, &BeatConfig::default());
        assert_eq!(peaks.len(), 1);
        assert_eq!(peaks[0], 1.5);
    }

    #[kithara::test(native, flash(false))]
    fn deduplicate_peaks_empty() {
        let peaks = deduplicate_peaks(&[], 1);
        assert!(peaks.is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn deduplicate_peaks_no_adjacent() {
        let peaks = deduplicate_peaks(&[5, 10, 20], 1);
        assert_eq!(peaks, vec![5.0, 10.0, 20.0]);
    }

    #[kithara::test(native, flash(false))]
    fn deduplicate_peaks_merge() {
        // 10 and 11 merge (gap 1) to 10.5; 12 is 1.5 from the mean → new group.
        let peaks = deduplicate_peaks(&[10, 11, 12, 20], 1);
        assert_eq!(peaks, vec![10.5, 12.0, 20.0]);

        // {10, 11, 11}: running mean 32/3, kept fractional.
        let peaks = deduplicate_peaks(&[10, 11, 11, 20], 1);
        assert_eq!(peaks.len(), 2);
        assert!((peaks[0] - 32.0 / 3.0).abs() < 1e-9);
        assert_eq!(peaks[1], 20.0);
    }

    #[kithara::test(native, flash(false))]
    fn deduplicate_peaks_single() {
        let peaks = deduplicate_peaks(&[42], 1);
        assert_eq!(peaks, vec![42.0]);
    }

    #[kithara::test(native, flash(false))]
    fn snap_downbeats() {
        let beats = vec![1.0, 2.0, 3.0];
        let mut downbeats = samples([1.1, 2.8]);
        snap_downbeats_to_beats(&beats, &mut downbeats);
        assert_eq!(&downbeats[..], &[1.0, 3.0]);
    }

    #[kithara::test(native, flash(false))]
    fn snap_downbeats_dedup() {
        let beats = vec![1.0, 2.0, 3.0];
        // Both downbeats snap to 2.0 and collapse to one.
        let mut downbeats = samples([1.8, 2.1]);
        snap_downbeats_to_beats(&beats, &mut downbeats);
        assert_eq!(&downbeats[..], &[2.0]);
    }

    #[kithara::test(native, flash(false))]
    fn snap_downbeats_empty_beats() {
        let beats: Vec<f32> = vec![];
        let mut downbeats = samples([1.0, 2.0]);
        snap_downbeats_to_beats(&beats, &mut downbeats);
        assert_eq!(&downbeats[..], &[1.0, 2.0]);
    }

    #[kithara::test(native, flash(false))]
    fn snap_downbeats_empty_downbeats() {
        let beats = vec![1.0, 2.0];
        let mut downbeats = samples([]);
        snap_downbeats_to_beats(&beats, &mut downbeats);
        assert!(downbeats.is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn decode_full() {
        let mut beat_logits = vec![-5.0; 200];
        let mut downbeat_logits = vec![-5.0; 200];

        beat_logits[50] = 3.0;
        beat_logits[100] = 2.5;
        beat_logits[150] = 4.0;
        // Downbeat at frame 51 snaps to the beat at frame 50.
        downbeat_logits[51] = 2.0;

        let pp = PeakPicker::new(BeatConfig::default());
        let (beats, downbeats) = pp
            .decode(&beat_logits, &downbeat_logits, &SamplePool::default())
            .unwrap();

        assert_eq!(&beats[..], &[1.0, 2.0, 3.0]);
        assert_eq!(&downbeats[..], &[1.0]);
    }

    #[kithara::test(native, flash(false))]
    fn decode_empty_logits() {
        let pp = PeakPicker::new(BeatConfig::default());
        let (beats, downbeats) = pp.decode(&[], &[], &SamplePool::default()).unwrap();
        assert!(beats.is_empty());
        assert!(downbeats.is_empty());
    }

    #[kithara::test(native, flash(false))]
    fn decode_reuses_the_injected_pool() {
        let pool = SamplePool::new(16, 1024);
        let pp = PeakPicker::new(BeatConfig::default());
        let beat_logits = [0.0, 1.0, 0.0];
        let downbeat_logits = [0.0, 0.8, 0.0];

        drop(pp.decode(&beat_logits, &downbeat_logits, &pool).unwrap());
        let misses = pool.stats().alloc_misses;
        drop(pp.decode(&beat_logits, &downbeat_logits, &pool).unwrap());

        assert_eq!(pool.stats().alloc_misses, misses);
    }

    #[kithara::test(native, flash(false))]
    fn decode_mismatched_lengths() {
        let pp = PeakPicker::new(BeatConfig::default());
        let err = pp.decode(&[1.0, 2.0], &[1.0], &SamplePool::default());
        assert!(err.is_err());
    }
}
