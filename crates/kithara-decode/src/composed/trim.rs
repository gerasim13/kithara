use kithara_platform::time::Duration;

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
