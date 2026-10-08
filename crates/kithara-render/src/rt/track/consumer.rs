use crate::worker::PcmReceiver;

/// The decoded packet ring's consumer; source control remains on the lane.
pub struct PcmConsumer {
    pub(super) receiver: PcmReceiver,
}

impl PcmConsumer {
    #[must_use]
    pub fn new(receiver: PcmReceiver) -> Self {
        Self { receiver }
    }
}
#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::mock::TestPcmReader;
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;

    use super::*;

    fn reader() -> Box<dyn AudioReader> {
        let rate = NonZeroU32::new(44_100).expect("static sample rate");
        Box::new(TestPcmReader::new(AudioSpec::new(2, rate), 0.1))
    }

    #[kithara::test(native)]
    fn playback_rate_reports_only_a_real_warp_control() {
        let mut fixed = PcmConsumer::new(reader());
        assert_eq!(fixed.apply_playback_rate(1.5), 1.0);
        assert_eq!(fixed.playback_rate(), 1.0);

        let mut warped =
            PcmConsumer::new(reader()).with_playback_rate(PlaybackRate::for_warp(1.25));
        let (built, applied) = if supports_playback_rate() {
            (1.25, 1.5)
        } else {
            (1.0, 1.0)
        };
        assert_eq!(warped.playback_rate(), built);
        assert_eq!(warped.apply_playback_rate(1.5), applied);
        assert_eq!(warped.playback_rate(), applied);
    }
}
