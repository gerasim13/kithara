use bon::Builder;
use struct_patch::Patch;

/// Policy for turning the beat model's raw logits into events.
///
/// The chunk geometry the model is run with is not here: it follows the
/// segmentation `beat_this` was trained on and is not a knob.
#[derive(Clone, Copy, Debug, Builder, PartialEq, Patch)]
#[patch(name = "BeatSettings")]
#[patch(attribute(derive(Clone, Copy, Debug, Default, serde::Deserialize)))]
#[patch(attribute(serde(default, deny_unknown_fields)))]
#[patch(attribute(non_exhaustive))]
#[non_exhaustive]
pub struct BeatConfig {
    /// Logit a frame must exceed to be a peak candidate. `0.0` is probability
    /// `0.5` after the sigmoid; lowering it admits quieter beats and the false
    /// positives that come with them.
    #[builder(default = 0.0)]
    pub peak_threshold: f32,
    /// Half-width, in model frames, of the max-pool window a frame must win to
    /// be a peak. The window spans `2 * peak_half_width + 1` frames, so this
    /// sets the shortest gap two beats may be reported at: at 50 fps the
    /// default of 3 keeps beats at least ~120 ms apart, which is 500 BPM.
    #[builder(default = 3)]
    pub peak_half_width: usize,
    /// Frames within which consecutive peaks collapse to their mean position.
    /// Absorbs the plateaus the model produces on a strong beat.
    #[builder(default = 1)]
    pub dedup_width: usize,
}

impl Default for BeatConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use kithara_test_utils::kithara;
    use struct_patch::Patch as _;

    use super::{BeatConfig, BeatSettings};

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_field_it_names() {
        let mut config = BeatConfig::builder().dedup_width(4).build();

        let patch: BeatSettings =
            serde_yaml_ng::from_str("peak_half_width: 5\n").expect("valid patch document");
        config.apply(patch);

        assert_eq!(config.peak_half_width, 5);
        assert_eq!(
            config.dedup_width, 4,
            "an unnamed field keeps its seeded value"
        );
    }
}
