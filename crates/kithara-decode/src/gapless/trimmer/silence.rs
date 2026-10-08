use kithara_signal::AudioChunk;
use num_traits::AsPrimitive;

use super::buffer::{chunk_frames, usize_from_u64_saturating};
use crate::{consts, gapless::heuristic::SilenceTrimParams};

/// Walk frames from the end of `tail_buffer`, group them into
/// `consts::TRAILING_SILENCE_WINDOW_MS` windows, and count frames as silent
/// while window-mean-|sample| stays below `threshold_amp`. Returns the
/// largest tail length whose energy is still below the floor.
///
/// Per-sample testing (the original implementation) misclassifies
/// zero-crossings of any periodic signal as silence: AAC quantisation
/// noise around a ZCR can dip below `1e-3` for a handful of frames at
/// every cycle. Integrating over a few-millisecond window prevents
/// the search from chewing into audible content via those gaps.
///
/// Mean-|sample| (rather than RMS) is used because it is less peak-
/// sensitive: for an audible sine its mean-abs is ≈0.6 of peak, while
/// for a noisy quiet region it tracks the average linear amplitude.
/// This widens the gap between "real" audio and codec quantisation
/// noise, making the threshold easier to pick.
pub(super) fn trailing_silent_frames(tail_buffer: &[AudioChunk], threshold_amp: f32) -> u64 {
    if tail_buffer.is_empty() {
        return 0;
    }

    let sample_rate = tail_buffer
        .first()
        .map_or(48_000, |chunk| chunk.spec().sample_rate.get())
        .max(1);
    let window_frames =
        (u64::from(sample_rate).saturating_mul(consts::TRAILING_SILENCE_WINDOW_MS) / 1000).max(1);
    let threshold = f64::from(threshold_amp);

    let mut silent_frames = 0u64;
    let mut window_sum_abs = 0.0_f64;
    let mut window_count: u64 = 0;

    for chunk in tail_buffer.iter().rev() {
        let chunk_total_frames = chunk_frames(chunk);
        let samples = &chunk.samples[..];
        let channels = usize::from(chunk.spec().channels.max(1));
        let channels_f64: f64 = channels.max(1).as_();
        for frame in (0..chunk_total_frames).rev() {
            let frame_start = usize_from_u64_saturating(frame).saturating_mul(channels);
            let frame_end = frame_start.saturating_add(channels).min(samples.len());
            if frame_end <= frame_start {
                continue;
            }
            let frame_sum_abs: f64 = samples[frame_start..frame_end]
                .iter()
                .map(|&sample| f64::from(sample.abs()))
                .sum();
            let frame_mean_abs = frame_sum_abs / channels_f64;
            window_sum_abs += frame_mean_abs;
            window_count = window_count.saturating_add(1);

            if window_count >= window_frames {
                let window_count_f64: f64 = window_count.as_();
                let mean_abs = window_sum_abs / window_count_f64;
                if mean_abs <= threshold {
                    silent_frames = silent_frames.saturating_add(window_count);
                    window_sum_abs = 0.0;
                    window_count = 0;
                } else {
                    return silent_frames;
                }
            }
        }
    }

    if window_count > 0 {
        let window_count_f64: f64 = window_count.as_();
        let mean_abs = window_sum_abs / window_count_f64;
        if mean_abs <= threshold {
            silent_frames = silent_frames.saturating_add(window_count);
        }
    }

    silent_frames
}

/// Find the first non-silent frame in the buffered leading audio.
///
/// Returns:
/// - `Some(n)` — the boundary is at frame `n` (counting silent
///   frames seen so far) AND `n >= params.min_trim_frames`. Caller
///   can drop `n` frames safely.
/// - `None` — either no boundary was found within
///   `params.scan_window_frames` (the audio looks like one long
///   fade-in and we choose to leave it alone), or a boundary was
///   found but with too few preceding silent frames to be considered
///   trim-worthy.
pub(super) fn find_leading_trim_frames(
    buffer: &[AudioChunk],
    params: &SilenceTrimParams,
    threshold_amp: f32,
) -> Option<u64> {
    let mut scanned_frames = 0_u64;
    let mut trim_frames = 0_u64;

    for chunk in buffer {
        let chunk_frames = chunk_frames(chunk);
        let samples = &chunk.samples[..];
        let channels = usize::from(chunk.spec().channels.max(1));
        for frame in 0..chunk_frames {
            if scanned_frames >= params.scan_window_frames {
                return None;
            }

            let frame_start = usize_from_u64_saturating(frame).saturating_mul(channels);
            let frame_end = frame_start.saturating_add(channels).min(samples.len());
            if frame_end <= frame_start {
                scanned_frames = scanned_frames.saturating_add(1);
                trim_frames = trim_frames.saturating_add(1);
                continue;
            }

            if !frame_is_silent(&samples[frame_start..frame_end], threshold_amp) {
                return (trim_frames >= params.min_trim_frames).then_some(trim_frames);
            }

            scanned_frames = scanned_frames.saturating_add(1);
            trim_frames = trim_frames.saturating_add(1);
        }
    }

    None
}

fn frame_is_silent(samples: &[f32], threshold_amp: f32) -> bool {
    samples.iter().all(|sample| sample.abs() <= threshold_amp)
}
