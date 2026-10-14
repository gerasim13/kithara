use std::collections::HashMap;

use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::sync::Mutex;
use ort::{
    environment::Environment,
    ep::CUDA,
    session::{
        Session,
        builder::{GraphOptimizationLevel, SessionBuilder},
    },
    value::TensorRef,
};
use smallvec::SmallVec;

use crate::nn::{api::BeatError, runtime::Tensor};

/// Owns the mutable native session; concurrent analysis serializes model runs.
pub(crate) struct CudaModel {
    session: Mutex<Session>,
}

impl CudaModel {
    pub(crate) fn new(
        bytes: &[u8],
        device_id: i32,
        environment: &Environment,
    ) -> Result<Self, BeatError> {
        Ok(Self {
            session: Mutex::new(Self::builder(device_id, environment)?.commit_from_memory(bytes)?),
        })
    }

    pub(crate) fn builder(
        device_id: i32,
        environment: &Environment,
    ) -> Result<SessionBuilder, BeatError> {
        Ok(Session::builder(environment)?
            .with_execution_providers([CUDA::default()
                .with_device_id(device_id)
                .with_tf32(false)
                .build()
                .error_on_failure()])
            .map_err(ort::Error::from)?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(ort::Error::from)?
            .with_intra_threads(1)
            .map_err(ort::Error::from)?)
    }

    pub(crate) fn run<S>(
        &self,
        input: &Tensor,
        pools: &PoolRegion<S>,
    ) -> Result<HashMap<String, Tensor>, BeatError>
    where
        S: HasPool<f32>,
    {
        let input = TensorRef::from_array_view((input.shape.as_slice(), &input.data[..]))?;
        self.session
            .lock()
            .run(ort::inputs!["spectrogram" => input])?
            .iter()
            .map(|(name, value)| {
                let (dimensions, values) = value.try_extract_tensor::<f32>()?;
                let shape: SmallVec<[usize; 4]> = dimensions
                    .iter()
                    .copied()
                    .map(usize::try_from)
                    .collect::<Result<_, _>>()
                    .map_err(|error| BeatError::Inference {
                        reason: format!("CUDA output '{name}' has invalid dimensions: {error}"),
                    })?;
                let mut data = pools.get_with_len::<f32>(values.len())?;
                data.copy_from_slice(values);
                Ok((name.to_owned(), Tensor { data, shape }))
            })
            .collect()
    }
}

#[cfg(all(test, feature = "embed-small-model"))]
mod tests {
    use std::fs;

    use kithara_config::path_from_env;
    use kithara_platform::sync::Mutex;
    use kithara_test_utils::kithara;
    use serde_json::Value;
    use smallvec::smallvec;

    use super::CudaModel;
    use crate::{
        BEAT_MODEL_BYTES,
        nn::{config::BeatConfig, runtime::Tensor},
        test_pools::pools,
    };

    #[kithara::test(native, flash(false))]
    #[ignore = "requires NVIDIA GPU/runtime and KITHARA_BEAT_CUDA_PROFILE_DIR"]
    fn native_cuda_model_profile_contains_compute_kernels() {
        let directory = path_from_env("KITHARA_BEAT_CUDA_PROFILE_DIR")
            .expect("explicit profile artifact directory");
        assert!(
            directory.is_absolute(),
            "profile artifacts need an absolute path"
        );
        fs::create_dir_all(&directory).expect("profile directory");
        let config = BeatConfig::default();
        let environment = ort::init_from(config.ort_library_path())
            .expect("configured native library")
            .build()
            .expect("caller-owned runtime environment");
        let session = CudaModel::builder(0, &environment)
            .expect("CUDA session builder")
            .with_profiling(directory.join(format!("beat-this-cuda-{}", std::process::id())))
            .expect("profile enabled")
            .commit_from_memory(BEAT_MODEL_BYTES)
            .expect("same embedded CUDA predictor");
        let model = CudaModel {
            session: Mutex::new(session),
        };
        let pools = pools();
        let mut data = pools
            .get_with_len::<f32>(1500 * 128)
            .expect("pooled model input");
        data.fill(1.0);
        let input = Tensor {
            data,
            shape: smallvec![1, 1500, 128],
        };
        for _ in 0..2 {
            let output = model.run(&input, &pools).expect("native CudaModel run");
            assert_eq!(output.len(), 2);
            for tensor in output.values() {
                assert_eq!(tensor.data.len(), 1500);
                assert!(tensor.data.iter().all(|value| value.is_finite()));
            }
        }
        let profile = model
            .session
            .lock()
            .end_profiling()
            .expect("native profile");
        let events: Vec<Value> =
            serde_json::from_slice(&fs::read(&profile).expect("profile bytes"))
                .expect("profile events");
        let kernels: Vec<_> = events
            .iter()
            .filter(|event| {
                let args = &event["args"];
                event["cat"] == "Node"
                    && args["provider"] == "CUDAExecutionProvider"
                    && event["dur"].as_f64().is_some_and(|duration| duration > 0.0)
                    && args["op_name"].as_str().is_some_and(|name| {
                        matches!(
                            name,
                            "MatMul"
                                | "Gemm"
                                | "Conv"
                                | "FusedMatMul"
                                | "Attention"
                                | "LayerNormalization"
                                | "FusedConv"
                                | "SkipLayerNormalization"
                        )
                    })
            })
            .collect();
        assert!(
            !kernels.is_empty(),
            "native predictor must execute CUDA compute kernels, not only copies/provider registration"
        );
        eprintln!(
            "native CUDA compute events={} profile={profile}",
            kernels.len()
        );
    }
}
