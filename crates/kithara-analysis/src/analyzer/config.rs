use std::fmt;

use bon::Builder;
#[cfg(feature = "beat-nn")]
use kithara_beat::{BeatConfig, BeatConfigPatch};
use kithara_resampler::{ResamplerBackend, ResamplerQuality};
use serde::Deserialize;
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

/// Beat-analysis tunables used by [`super::AnalyzerBuilder`], beside the
/// resampler backend the caller hands over.
///
/// [`BeatAnalysisConfigPatch`] is what a configuration document may say about
/// it.
#[derive(Clone, Builder, fieldwork::Fieldwork)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
#[fieldwork(get)]
pub struct BeatAnalysisConfig<B> {
    resampler_backend: B,
    #[builder(default = Consts::DEFAULT_BEAT_RESAMPLER_QUALITY)]
    #[field(get(copy))]
    pub resampler_quality: ResamplerQuality,
    #[builder(default = Consts::DEFAULT_BEAT_DETECTOR_MIN_WINDOW_SECONDS)]
    pub detector_min_window_seconds: u32,
    #[builder(default = Consts::DEFAULT_BEAT_DETECTOR_OVERLAP_SECONDS)]
    pub detector_overlap_seconds: u32,
    #[builder(default = Consts::DEFAULT_BEAT_DETECTOR_WINDOW_SECONDS)]
    pub detector_window_seconds: u32,
    #[builder(default = Consts::DEFAULT_BEAT_TARGET_RATE)]
    pub target_rate: u32,
    #[builder(default = Consts::DEFAULT_BEAT_BLOCK_FRAMES)]
    pub block_frames: usize,
    /// Reaches the detector's peak-picking policy. Nested rather than
    /// flattened so a document can patch `beat:` on its own.
    #[cfg(feature = "beat-nn")]
    #[builder(default)]
    #[field(get(copy))]
    pub beat: BeatConfig,
}

/// What a configuration document may say about [`BeatAnalysisConfig`].
///
/// Hand-written rather than derived: `struct-patch` copies a struct's generics
/// and where-clause verbatim onto the patch it generates, so a patch of a
/// generic configuration whose generic-carrying field is skipped has a type
/// parameter no field uses and does not compile. `resampler_backend` is the
/// caller's own object and is absent on purpose; `deny_unknown_fields` refuses
/// it by name rather than dropping it silently.
///
/// `Deserialize` only, never `Serialize`: by the time a patch is typed its
/// references are resolved, so the tree it merges into holds secrets in the
/// clear.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct BeatAnalysisConfigPatch {
    /// See [`BeatAnalysisConfig::resampler_quality`].
    pub resampler_quality: Option<ResamplerQuality>,
    /// See [`BeatAnalysisConfig::detector_min_window_seconds`].
    pub detector_min_window_seconds: Option<u32>,
    /// See [`BeatAnalysisConfig::detector_overlap_seconds`].
    pub detector_overlap_seconds: Option<u32>,
    /// See [`BeatAnalysisConfig::detector_window_seconds`].
    pub detector_window_seconds: Option<u32>,
    /// See [`BeatAnalysisConfig::target_rate`].
    pub target_rate: Option<u32>,
    /// See [`BeatAnalysisConfig::block_frames`].
    pub block_frames: Option<usize>,
    /// See [`BeatAnalysisConfig::beat`].
    #[cfg(feature = "beat-nn")]
    pub beat: BeatConfigPatch,
}

impl<B> Patch<BeatAnalysisConfigPatch> for BeatAnalysisConfig<B> {
    fn apply(&mut self, patch: BeatAnalysisConfigPatch) {
        if let Some(resampler_quality) = patch.resampler_quality {
            self.resampler_quality = resampler_quality;
        }
        if let Some(detector_min_window_seconds) = patch.detector_min_window_seconds {
            self.detector_min_window_seconds = detector_min_window_seconds;
        }
        if let Some(detector_overlap_seconds) = patch.detector_overlap_seconds {
            self.detector_overlap_seconds = detector_overlap_seconds;
        }
        if let Some(detector_window_seconds) = patch.detector_window_seconds {
            self.detector_window_seconds = detector_window_seconds;
        }
        if let Some(target_rate) = patch.target_rate {
            self.target_rate = target_rate;
        }
        if let Some(block_frames) = patch.block_frames {
            self.block_frames = block_frames;
        }
        #[cfg(feature = "beat-nn")]
        self.beat.apply(patch.beat);
    }

    fn into_patch(self) -> BeatAnalysisConfigPatch {
        BeatAnalysisConfigPatch {
            resampler_quality: Some(self.resampler_quality),
            detector_min_window_seconds: Some(self.detector_min_window_seconds),
            detector_overlap_seconds: Some(self.detector_overlap_seconds),
            detector_window_seconds: Some(self.detector_window_seconds),
            target_rate: Some(self.target_rate),
            block_frames: Some(self.block_frames),
            #[cfg(feature = "beat-nn")]
            beat: self.beat.into_patch(),
        }
    }

    fn into_patch_by_diff(self, previous: Self) -> BeatAnalysisConfigPatch {
        BeatAnalysisConfigPatch {
            resampler_quality: (self.resampler_quality != previous.resampler_quality)
                .then_some(self.resampler_quality),
            detector_min_window_seconds: (self.detector_min_window_seconds
                != previous.detector_min_window_seconds)
                .then_some(self.detector_min_window_seconds),
            detector_overlap_seconds: (self.detector_overlap_seconds
                != previous.detector_overlap_seconds)
                .then_some(self.detector_overlap_seconds),
            detector_window_seconds: (self.detector_window_seconds
                != previous.detector_window_seconds)
                .then_some(self.detector_window_seconds),
            target_rate: (self.target_rate != previous.target_rate).then_some(self.target_rate),
            block_frames: (self.block_frames != previous.block_frames).then_some(self.block_frames),
            #[cfg(feature = "beat-nn")]
            beat: self.beat.into_patch_by_diff(previous.beat),
        }
    }

    fn new_empty_patch() -> BeatAnalysisConfigPatch {
        BeatAnalysisConfigPatch::default()
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
        out.field("block_frames", &self.block_frames);
        out.field("target_rate", &self.target_rate);
        out.field("resampler_quality", &self.resampler_quality);
        out.field("resampler_backend", &self.resampler_backend_name());
        out.field(
            "detector_min_window_seconds",
            &self.detector_min_window_seconds,
        );
        out.field("detector_window_seconds", &self.detector_window_seconds);
        out.field("detector_overlap_seconds", &self.detector_overlap_seconds);
        #[cfg(feature = "beat-nn")]
        out.field("beat", &self.beat);
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
                .beat(beat)
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
mod document_tests {
    #[cfg(feature = "beat-nn")]
    use kithara_beat::BeatConfig;
    use kithara_resampler::{ResamplerQuality, rubato::RubatoBackend};
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{BeatAnalysisConfig, BeatAnalysisConfigPatch};

    fn config() -> BeatAnalysisConfig<RubatoBackend> {
        BeatAnalysisConfig::builder()
            .resampler_backend(RubatoBackend::default())
            .build()
    }

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_fields_it_names() {
        let patch: BeatAnalysisConfigPatch =
            serde_yaml_ng::from_str("target_rate: 48000\n").expect("the document types");
        let mut config = config();
        // Seeded off the crate default (1024) so a whole-struct `apply` that
        // resets every unnamed field cannot pass this assertion by chance.
        config.block_frames = 2048;

        config.apply(patch);

        assert_eq!(config.target_rate, 48_000);
        assert_eq!(
            config.block_frames, 2048,
            "a silent field must keep the value it already had"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_named_resampler_quality_arrives_as_its_variant() {
        let patch: BeatAnalysisConfigPatch =
            serde_yaml_ng::from_str("resampler_quality: fast\n").expect("the document types");
        let mut config = config();
        config.resampler_quality = ResamplerQuality::Normal;

        config.apply(patch);

        assert_eq!(
            config.resampler_quality,
            ResamplerQuality::Fast,
            "the document's snake_case name must reach the variant"
        );
    }

    #[cfg(feature = "beat-nn")]
    #[kithara::test(native, flash(false))]
    fn a_nested_beat_patch_reaches_the_inner_field() {
        let patch: BeatAnalysisConfigPatch =
            serde_yaml_ng::from_str("beat:\n  peak_half_width: 5\n").expect("the document types");
        let mut config = config();
        config.beat = BeatConfig::builder().dedup_width(4).build();

        config.apply(patch);

        assert_eq!(config.beat.peak_half_width, 5);
        assert_eq!(
            config.beat.dedup_width, 4,
            "a silent inner field must keep the value it already had"
        );
    }

    #[kithara::test(native, flash(false))]
    fn an_unknown_field_is_rejected_and_named() {
        let error = serde_yaml_ng::from_str::<BeatAnalysisConfigPatch>("target_rate_hz: 1\n")
            .expect_err("a typo must not be silently ignored");

        assert!(format!("{error}").contains("target_rate_hz"), "{error}");
    }

    /// `resampler_backend` is the caller's own object, not a value a document
    /// can name (see the patch's doc comment).
    #[kithara::test(native, flash(false))]
    fn the_caller_owned_backend_is_not_a_document_key() {
        let error =
            serde_yaml_ng::from_str::<BeatAnalysisConfigPatch>("resampler_backend: rubato\n")
                .expect_err("a passed object must not be settable from a document");

        assert!(format!("{error}").contains("resampler_backend"), "{error}");
    }
}
