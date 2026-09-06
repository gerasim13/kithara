use kithara_beat::{BEAT_MODEL_BYTES, BeatThis, MEL_MODEL_BYTES};
use kithara_bufpool::{HasPool, PoolRegion};

use super::{
    super::detector::{BeatDetectError, BeatDetector, RawBeats},
    build::marks,
};
use crate::BeatAnalysisConfig;

pub(super) fn detector<B, S>(
    config: &BeatAnalysisConfig<B>,
    pools: &PoolRegion<S>,
) -> Result<BeatThis<S>, BeatDetectError>
where
    S: HasPool<f32>,
{
    BeatThis::builder()
        .mel_model(MEL_MODEL_BYTES)
        .beat_model(BEAT_MODEL_BYTES)
        .pools(pools.clone())
        .config(config.beat())
        .build()
        .map_err(|e| BeatDetectError::Init {
            reason: e.to_string(),
        })
}

impl<S> BeatDetector for BeatThis<S>
where
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn detect(&self, mono_window: &[f32]) -> Result<RawBeats, BeatDetectError> {
        self.analyze(mono_window)
            .map(marks)
            .map_err(|e| BeatDetectError::Detect {
                reason: e.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use kithara_beat::BeatConfig;
    use kithara_resampler::rubato::RubatoBackend;
    use kithara_test_utils::kithara;
    use num_traits::cast::AsPrimitive;

    use super::super::{BeatDetectorKind, build_detector};
    use crate::{BeatAnalysisConfig, test_pools::pools};

    struct Consts;

    impl Consts {
        const SAMPLE_RATE: usize = 22_050;
        const SECONDS: usize = 2;
    }

    fn tone() -> Vec<f32> {
        let rate: f32 = Consts::SAMPLE_RATE.as_();
        let step = std::f32::consts::TAU * 220.0 / rate;
        (0..Consts::SECONDS * Consts::SAMPLE_RATE)
            .map(|n| {
                let t: f32 = n.as_();
                0.5 * (step * t).sin()
            })
            .collect()
    }

    fn config(beat: BeatConfig) -> BeatAnalysisConfig<RubatoBackend> {
        BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .beat(beat)
            .build()
    }

    #[kithara::test(native, flash(false))]
    fn a_non_default_beat_config_reaches_the_picker() {
        let pcm = tone();

        let suppressed = config(BeatConfig::builder().peak_threshold(f32::MAX).build());
        let detector = build_detector(BeatDetectorKind::NnBeatThis, &suppressed, &pools())
            .unwrap_or_else(|e| panic!("suppressed detector init failed: {e}"));
        let raw = detector
            .detect(&pcm)
            .unwrap_or_else(|e| panic!("suppressed detect failed: {e}"));
        assert!(
            raw.beats.is_empty() && raw.downbeats.is_empty(),
            "a threshold above every possible logit must admit no peaks"
        );

        let admit_all = config(
            BeatConfig::builder()
                .peak_threshold(f32::MIN)
                .peak_half_width(0)
                .dedup_width(0)
                .build(),
        );
        let detector = build_detector(BeatDetectorKind::NnBeatThis, &admit_all, &pools())
            .unwrap_or_else(|e| panic!("admit-all detector init failed: {e}"));
        let raw = detector
            .detect(&pcm)
            .unwrap_or_else(|e| panic!("admit-all detect failed: {e}"));
        assert!(
            !raw.beats.is_empty() && !raw.downbeats.is_empty(),
            "a threshold below every possible logit, with no suppression window, must admit a peak at every frame"
        );
    }
}
