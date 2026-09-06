use bon::Builder;
use kithara_macros::Patch;

/// Policy for turning the beat model's raw logits into events.
#[derive(Clone, Copy, Debug, Builder, PartialEq, Patch)]
#[non_exhaustive]
pub struct BeatConfig {
    /// Logit a frame must exceed to be a peak candidate; `0.0` is an even chance.
    #[builder(default = 0.0)]
    pub peak_threshold: f32,
    /// Frames within which consecutive peaks collapse to their mean position.
    #[builder(default = 1)]
    pub dedup_width: usize,
    /// Half-width, in model frames, of the max-pool window a frame must win.
    /// The default keeps beats at least 120 ms apart at 50 fps.
    #[builder(default = 3)]
    pub peak_half_width: usize,
}

impl Default for BeatConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use kithara_test_utils::kithara;

    use super::{BeatConfig, BeatConfigPatch};

    #[kithara::test(native, flash(false))]
    fn a_patch_writes_only_the_field_it_names() {
        let mut config = BeatConfig::builder().dedup_width(4).build();

        let patch: BeatConfigPatch =
            serde_yaml_ng::from_str("peak_half_width: 5\n").expect("valid patch document");
        config.apply(patch);

        assert_eq!(config.peak_half_width, 5);
        assert_eq!(
            config.dedup_width, 4,
            "an unnamed field keeps its seeded value"
        );
    }
}
