use std::num::NonZeroU32;

use kithara::host::HostConfig;

use crate::types::FfiError;

/// Settings fixed for the lifetime of the process-wide audio host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi", derive(uniffi::Record))]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct FfiHostConfig {
    /// Initial device sample-rate hint in hertz.
    pub sample_rate_hint: u32,
    /// Optional native output callback size in frames.
    pub output_block_frames: Option<u32>,
}

impl Default for FfiHostConfig {
    fn default() -> Self {
        Self {
            sample_rate_hint: 44_100,
            output_block_frames: None,
        }
    }
}

impl FfiHostConfig {
    pub(crate) fn into_domain<S>(self) -> Result<HostConfig<S>, FfiError> {
        let sample_rate_hint =
            NonZeroU32::new(self.sample_rate_hint).ok_or_else(|| FfiError::InvalidArgument {
                reason: "host sample rate must be greater than zero".to_owned(),
            })?;
        let output_block_frames = self
            .output_block_frames
            .map(|frames| {
                NonZeroU32::new(frames).ok_or_else(|| FfiError::InvalidArgument {
                    reason: "host output block size must be greater than zero".to_owned(),
                })
            })
            .transpose()?;
        Ok(HostConfig::builder()
            .sample_rate_hint(sample_rate_hint)
            .maybe_output_block_frames(output_block_frames)
            .build())
    }
}

/// Return canonical host defaults without initializing runtime resources.
#[must_use]
#[cfg_attr(feature = "uniffi", uniffi::export)]
#[cfg_attr(
    target_arch = "wasm32",
    wasm_bindgen::prelude::wasm_bindgen(js_name = defaultHostConfig)
)]
pub fn default_host_config() -> FfiHostConfig {
    FfiHostConfig::default()
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;
    use crate::pools::FfiPools;

    #[kithara::test]
    fn rejects_zero_host_dimensions_without_initializing_resources() {
        let zero_rate = FfiHostConfig {
            sample_rate_hint: 0,
            ..FfiHostConfig::default()
        };
        assert!(matches!(
            zero_rate.into_domain::<FfiPools>(),
            Err(FfiError::InvalidArgument { .. })
        ));
        let zero_block = FfiHostConfig {
            output_block_frames: Some(0),
            ..FfiHostConfig::default()
        };
        assert!(matches!(
            zero_block.into_domain::<FfiPools>(),
            Err(FfiError::InvalidArgument { .. })
        ));
    }
}
