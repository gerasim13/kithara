use std::fmt;

use bon::Builder;
#[cfg(feature = "beat-nn")]
use kithara_beat::{BeatConfig, BeatSettings};
use kithara_resampler::{ResamplerBackend, ResamplerQuality};
use struct_patch::Patch;

struct Consts;

impl Consts {
    const DEFAULT_BEAT_BLOCK_FRAMES: usize = 1024;
    const DEFAULT_BEAT_DETECTOR_MIN_WINDOW_SECONDS: u32 = 10;
    const DEFAULT_BEAT_DETECTOR_OVERLAP_SECONDS: u32 = 2;
    const DEFAULT_BEAT_DETECTOR_WINDOW_SECONDS: u32 = 30;
    const DEFAULT_BEAT_RESAMPLER_QUALITY: ResamplerQuality = ResamplerQuality::High;
    const DEFAULT_BEAT_TARGET_RATE: u32 = 22_050;
}

/// Beat-analysis tunables that do not depend on the resampler backend type.
#[derive(Clone, Builder, fieldwork::Fieldwork, Patch)]
#[builder(state_mod(vis = "pub"))]
#[patch(name = "BeatAnalysisSettingsPatch")]
#[patch(attribute(derive(Clone, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
#[fieldwork(get)]
pub struct BeatAnalysisSettings {
    #[builder(default = Consts::DEFAULT_BEAT_RESAMPLER_QUALITY)]
    #[field(get(copy))]
    resampler_quality: ResamplerQuality,
    #[builder(default = Consts::DEFAULT_BEAT_DETECTOR_OVERLAP_SECONDS)]
    detector_overlap_seconds: u32,
    #[builder(default = Consts::DEFAULT_BEAT_DETECTOR_MIN_WINDOW_SECONDS)]
    detector_min_window_seconds: u32,
    #[builder(default = Consts::DEFAULT_BEAT_DETECTOR_WINDOW_SECONDS)]
    detector_window_seconds: u32,
    #[builder(default = Consts::DEFAULT_BEAT_TARGET_RATE)]
    target_rate: u32,
    #[builder(default = Consts::DEFAULT_BEAT_BLOCK_FRAMES)]
    block_frames: usize,
    /// Reaches the detector's peak-picking policy. Nested rather than
    /// flattened so a document can patch `beat:` on its own.
    #[cfg(feature = "beat-nn")]
    #[builder(default)]
    #[field(get(copy))]
    #[patch(name = "BeatSettings")]
    beat: BeatConfig,
}

impl Default for BeatAnalysisSettings {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Beat-analysis tunables used by [`super::AnalyzerBuilder`].
#[derive(Clone, Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
#[fieldwork(get)]
pub struct BeatAnalysisConfig<B> {
    resampler_backend: B,
    #[builder(default)]
    settings: BeatAnalysisSettings,
}

#[cfg(feature = "analysis-beat")]
impl<B> BeatAnalysisConfig<B> {
    delegate::delegate! {
        to self.settings {
            pub(crate) fn resampler_quality(&self) -> ResamplerQuality;
            pub(crate) fn detector_overlap_seconds(&self) -> u32;
            pub(crate) fn detector_min_window_seconds(&self) -> u32;
            pub(crate) fn detector_window_seconds(&self) -> u32;
            pub(crate) fn target_rate(&self) -> u32;
            pub(crate) fn block_frames(&self) -> usize;
        }
    }
}

#[cfg(feature = "beat-nn")]
impl<B> BeatAnalysisConfig<B> {
    delegate::delegate! {
        to self.settings {
            pub(crate) fn beat(&self) -> BeatConfig;
        }
    }
}

impl<B> BeatAnalysisConfig<B>
where
    B: ResamplerBackend,
{
    #[must_use]
    pub fn cache_tag(&self) -> Option<String> {
        super::nn::tag(self)
    }

    fn resampler_backend_name(&self) -> &'static str {
        self.resampler_backend.name()
    }
}

impl<B> fmt::Debug for BeatAnalysisConfig<B>
where
    B: ResamplerBackend,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_struct("BeatAnalysisConfig");
        out.field("block_frames", &self.settings.block_frames);
        out.field("target_rate", &self.settings.target_rate);
        out.field("resampler_quality", &self.settings.resampler_quality);
        out.field("resampler_backend", &self.resampler_backend_name());
        out.field(
            "detector_min_window_seconds",
            &self.settings.detector_min_window_seconds,
        );
        out.field(
            "detector_window_seconds",
            &self.settings.detector_window_seconds,
        );
        out.field(
            "detector_overlap_seconds",
            &self.settings.detector_overlap_seconds,
        );
        #[cfg(feature = "beat-nn")]
        out.field("beat", &self.settings.beat);
        out.finish()
    }
}

impl<B> Default for BeatAnalysisConfig<B>
where
    B: ResamplerBackend + Default,
{
    fn default() -> Self {
        Self::builder().resampler_backend(B::default()).build()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "beat-nn")]
    use kithara_beat::BeatConfig;
    use kithara_resampler::rubato::RubatoBackend;
    use kithara_test_utils::kithara;

    use super::BeatAnalysisConfig;
    #[cfg(feature = "beat-nn")]
    use super::BeatAnalysisSettings;

    #[kithara::test(native, flash(false))]
    fn default_beat_config_reports_configured_backend() {
        assert_eq!(
            BeatAnalysisConfig::<RubatoBackend>::default().resampler_backend_name(),
            "rubato"
        );
    }

    #[cfg(feature = "beat-nn")]
    #[kithara::test(native, flash(false))]
    fn cache_tag_invalidates_pre_confidence_results() {
        let tag = BeatAnalysisConfig::<RubatoBackend>::default()
            .cache_tag()
            .expect("beat NN has a cache tag");

        assert!(
            tag.contains(":grid_bpm_from_beats_v2:"),
            "grid semantics must participate in durable-cache identity"
        );
        assert!(
            !tag.contains(":grid_bpm_from_beats_v1:"),
            "a grid carrying per-marker confidence is not the grid v1 cached"
        );
    }

    #[cfg(feature = "beat-nn")]
    #[kithara::test(native, flash(false))]
    fn a_moved_picking_policy_changes_the_cache_tag() {
        let tag = |beat: BeatConfig| {
            BeatAnalysisConfig::builder()
                .resampler_backend(RubatoBackend::default())
                .settings(BeatAnalysisSettings::builder().beat(beat).build())
                .build()
                .cache_tag()
                .expect("beat NN has a cache tag")
        };

        assert_ne!(
            tag(BeatConfig::default()),
            tag(BeatConfig::builder().peak_threshold(0.25).build()),
            "a moved peak-picking policy must not share a cached grid"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod settings_tests {
    #[cfg(feature = "beat-nn")]
    use kithara_beat::BeatConfig;
    use kithara_resampler::ResamplerQuality;
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{BeatAnalysisSettings, BeatAnalysisSettingsPatch};

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_fields_it_names() {
        let patch: BeatAnalysisSettingsPatch =
            serde_yaml_ng::from_str("target_rate: 48000\n").expect("the document types");
        let mut settings = BeatAnalysisSettings::builder().block_frames(2048).build();

        settings.apply(patch);

        assert_eq!(settings.target_rate(), 48_000);
        assert_eq!(
            settings.block_frames(),
            2048,
            "a silent field must keep the value it already had"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_named_resampler_quality_arrives_as_its_variant() {
        let patch: BeatAnalysisSettingsPatch =
            serde_yaml_ng::from_str("resampler_quality: fast\n").expect("the document types");
        let mut settings = BeatAnalysisSettings::builder()
            .resampler_quality(ResamplerQuality::Normal)
            .build();

        settings.apply(patch);

        assert_eq!(
            settings.resampler_quality(),
            ResamplerQuality::Fast,
            "the document's snake_case name must reach the variant"
        );
    }

    #[cfg(feature = "beat-nn")]
    #[kithara::test(native, flash(false))]
    fn a_nested_beat_patch_reaches_the_inner_field() {
        let patch: BeatAnalysisSettingsPatch =
            serde_yaml_ng::from_str("beat:\n  peak_half_width: 5\n").expect("the document types");
        let mut settings = BeatAnalysisSettings::builder()
            .beat(BeatConfig::builder().dedup_width(4).build())
            .build();

        settings.apply(patch);

        assert_eq!(settings.beat().peak_half_width, 5);
        assert_eq!(
            settings.beat().dedup_width,
            4,
            "a silent inner field must keep the value it already had"
        );
    }

    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<BeatAnalysisSettingsPatch>("target_ratee: 1\n")
            .expect_err("a typo must not be silently ignored");

        assert!(format!("{error}").contains("target_ratee"), "{error}");
    }
}
