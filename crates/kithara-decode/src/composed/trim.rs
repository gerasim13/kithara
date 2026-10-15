use std::num::{NonZeroU32, NonZeroU128};

use kithara_platform::time::Duration;
use kithara_signal::{AudioChunkInfo, AudioSpec};

use crate::error::{DecodeError, DecodeResult};

/// Per-channel sample count to drop from the head of the target packet
/// so the emitted chunk starts at the user's seek target rather than
/// at the packet boundary. Returns 0 when `frame_pts >= target` or
/// `sample_rate == 0`. Rounds to nearest sample (half-up) so the trim
/// matches `round(target_secs * sample_rate)` callers use to index
/// PCM by absolute sample frame.
pub(super) fn frames_to_trim(frame_pts: Duration, target: Duration, sample_rate: u32) -> u64 {
    if sample_rate == 0 || frame_pts >= target {
        return 0;
    }
    let delta_nanos = target.saturating_sub(frame_pts).as_nanos();
    let sr_u128 = u128::from(sample_rate);
    let frames_u128 = delta_nanos
        .saturating_mul(sr_u128)
        .saturating_add(500_000_000)
        / 1_000_000_000;
    u64::try_from(frames_u128).unwrap_or(u64::MAX)
}

pub(super) fn frames_to_trim_prefix(
    frame_pts: Duration,
    target: Duration,
    sample_rate: NonZeroU32,
) -> DecodeResult<u64> {
    const NANOS_PER_SECOND: NonZeroU128 = match NonZeroU128::new(1_000_000_000) {
        Some(value) => value,
        None => unreachable!(),
    };
    if frame_pts >= target {
        return Ok(0);
    }
    let spec = AudioSpec::new(1, sample_rate);
    let first = spec.frame_at(frame_pts)?;
    let basis = AudioChunkInfo {
        spec,
        frame_offset: first,
        frames: 1,
        timestamp: frame_pts,
        ..AudioChunkInfo::default()
    }
    .source_geometry()
    .ok_or(DecodeError::InvalidData {
        detail: "seek prefix PCM geometry overflow",
    })?;
    let target_numerator = target
        .as_nanos()
        .checked_mul(u128::from(sample_rate.get()))
        .ok_or(DecodeError::InvalidData {
            detail: "seek prefix source position overflow",
        })?;
    let (numerator, denominator) = basis
        .output_ratio_at_source(first, (target_numerator, NANOS_PER_SECOND))
        .ok_or(DecodeError::InvalidData {
            detail: "seek prefix inverse geometry overflow",
        })?;
    let position =
        u64::try_from(numerator / denominator.get()).map_err(|_| DecodeError::InvalidData {
            detail: "seek prefix physical position overflow",
        })?;
    Ok(position.saturating_sub(first))
}
