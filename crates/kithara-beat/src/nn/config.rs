#[cfg(feature = "cuda")]
use std::{
    env::consts::{DLL_PREFIX, DLL_SUFFIX},
    path::PathBuf,
};

use kithara_config::Config;
#[cfg(feature = "cuda")]
use kithara_config::path_from_env;
use kithara_derive::Patch;

use crate::nn::backend::BeatBackend;

/// Model runtime and policy for turning raw logits into beat events.
#[derive(Clone, Debug, Config, PartialEq, Patch)]
#[config(default, fields(value))]
#[non_exhaustive]
pub struct BeatConfig {
    /// Beat predictor runtime; mel extraction remains on CPU.
    #[config(builder(default), get(copy))]
    backend: BeatBackend,
    /// Resolved native GPU ONNX Runtime library used by the CUDA backend.
    ///
    /// Defaults to `ORT_DYLIB_PATH`, then the platform's native library filename.
    /// The runtime loads one library per process; configure this before any other
    /// runtime use and use the same library for every detector.
    #[cfg(feature = "cuda")]
    #[config(builder(default = default_library_path()), get(ref))]
    ort_library_path: PathBuf,
    /// Logit a frame must exceed to be a peak candidate; `0.0` is an even chance.
    #[config(builder(default = 0.0))]
    pub peak_threshold: f32,
    /// Frames within which consecutive peaks collapse to their mean position.
    #[config(builder(default = 1))]
    pub dedup_width: usize,
    /// Half-width, in model frames, of the max-pool window a frame must win.
    /// The default keeps beats at least 120 ms apart at 50 fps.
    #[config(builder(default = 3))]
    pub peak_half_width: usize,
}

#[cfg(feature = "cuda")]
fn default_library_path() -> PathBuf {
    path_from_env("ORT_DYLIB_PATH")
        .unwrap_or_else(|| PathBuf::from(format!("{DLL_PREFIX}onnxruntime{DLL_SUFFIX}")))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    #[cfg(feature = "cuda")]
    use std::path::PathBuf;

    use kithara_test_utils::kithara;

    use super::{BeatBackend, BeatConfig, BeatConfigPatch};

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
        assert_eq!(config.backend(), BeatBackend::Cpu);
    }

    #[cfg(feature = "cuda")]
    #[kithara::test(native, flash(false))]
    fn a_document_patch_configures_the_model_runtime() {
        let mut config = BeatConfig::default();
        let patch: BeatConfigPatch = serde_json::from_str(
            r#"{"backend":{"cuda":{"device_id":2}},"ort_library_path":"configured-runtime"}"#,
        )
        .expect("runtime configuration document");
        config.apply(patch);

        assert_eq!(config.backend(), BeatBackend::Cuda { device_id: 2 });
        assert_eq!(
            config.ort_library_path(),
            &PathBuf::from("configured-runtime")
        );
        assert_eq!(config.peak_threshold, 0.0);
        assert_eq!(config.dedup_width, 1);
        assert_eq!(config.peak_half_width, 3);
    }
}
