use super::{SAW_PERIOD, phase};

/// Detected direction of a saw-tooth signal in decoded PCM.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum SignalDirection {
    /// Values increase each frame.
    Ascending,
    /// Values decrease each frame.
    Descending,
    /// Direction could not be determined.
    Unknown,
}

/// Detect a saw-tooth direction from a buffer of interleaved `f32` samples.
#[must_use]
pub fn detect_direction(samples: &[f32], channels: usize) -> SignalDirection {
    if channels == 0 {
        return SignalDirection::Unknown;
    }
    let frames = samples.len() / channels;
    if frames < 2 {
        return SignalDirection::Unknown;
    }

    let check_count = 10.min(frames - 1);
    let mut ascending_votes = 0u32;
    let mut descending_votes = 0u32;

    for frame in 0..check_count {
        let current = phase::units(samples[frame * channels]);
        let next = phase::units(samples[(frame + 1) * channels]);
        let expected_asc = (current + 1) % SAW_PERIOD;
        let expected_desc = (current + SAW_PERIOD - 1) % SAW_PERIOD;

        if next == expected_asc {
            ascending_votes += 1;
        } else if next == expected_desc {
            descending_votes += 1;
        }
    }

    if ascending_votes > descending_votes && ascending_votes > 0 {
        SignalDirection::Ascending
    } else if descending_votes > ascending_votes && descending_votes > 0 {
        SignalDirection::Descending
    } else {
        SignalDirection::Unknown
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{SignalDirection, detect_direction};

    #[kithara::test(native, flash(false))]
    fn one_ascending_step_per_frame_reads_as_ascending() {
        assert_eq!(
            detect_direction(&[-1.0, -1.0, -0.9999695, -0.9999695], 2),
            SignalDirection::Ascending
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_channel_less_buffer_has_no_direction() {
        assert_eq!(detect_direction(&[-1.0, -0.5], 0), SignalDirection::Unknown);
    }
}
