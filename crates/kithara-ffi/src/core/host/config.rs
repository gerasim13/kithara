use std::num::NonZeroU32;

use kithara::host::HostConfig;

use crate::{FfiLimiterConfig, types::FfiError};

/// Settings fixed for the lifetime of the process-wide audio host.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    any(feature = "uniffi", feature = "uniffi-web"),
    derive(uniffi::Record)
)]
#[cfg_attr(target_arch = "wasm32", wasm_bindgen::prelude::wasm_bindgen)]
pub struct FfiHostConfig {
    /// Initial device sample-rate hint in hertz.
    pub sample_rate_hint: u32,
    /// Optional native output callback size in frames.
    pub output_block_frames: Option<u32>,
    /// Output limiter policy prepared when the host starts.
    pub limiter: FfiLimiterConfig,
}

impl Default for FfiHostConfig {
    fn default() -> Self {
        Self {
            sample_rate_hint: 44_100,
            output_block_frames: None,
            limiter: FfiLimiterConfig::default(),
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
            .limiter(self.limiter.try_into()?)
            .build())
    }
}

/// Return canonical host defaults without initializing runtime resources.
#[must_use]
#[cfg_attr(any(feature = "uniffi", feature = "uniffi-web"), uniffi::export)]
#[cfg_attr(
    target_arch = "wasm32",
    wasm_bindgen::prelude::wasm_bindgen(js_name = defaultHostConfig)
)]
pub fn default_host_config() -> FfiHostConfig {
    FfiHostConfig::default()
}

/// Initialize the platform host through the generated SDK surface.
///
/// # Errors
/// Returns a typed lifecycle or host-construction error.
#[cfg_attr(any(feature = "uniffi", feature = "uniffi-web"), uniffi::export)]
pub fn initialize_host(config: FfiHostConfig) -> Result<(), FfiError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::native::session::initialize_host(config)
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::web::bridge::initialize_host_domain(config)
    }
}

/// Ensure the native process host exists with its default configuration.
///
/// # Errors
/// Returns a host-construction error if default initialization fails.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(feature = "uniffi", uniffi::export)]
pub fn ensure_default_host() -> Result<(), FfiError> {
    crate::native::session::ensure_default_host()
}

#[cfg(test)]
mod tests {
    use ::kithara::play::effects::LimiterConfig;
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

        let invalid_limiter = FfiHostConfig {
            limiter: FfiLimiterConfig {
                ceiling: 1.5,
                ..FfiLimiterConfig::default()
            },
            ..FfiHostConfig::default()
        };
        assert!(matches!(
            invalid_limiter.into_domain::<FfiPools>(),
            Err(FfiError::InvalidArgument { .. })
        ));
    }

    #[kithara::test]
    fn host_configuration_carries_validated_limiter_settings() {
        let defaults = FfiHostConfig::default();
        let domain_defaults = LimiterConfig::default();
        assert_eq!(defaults.limiter.ceiling, domain_defaults.ceiling());
        assert_eq!(defaults.limiter.release_ms, domain_defaults.release_ms());

        let wire = FfiHostConfig {
            limiter: FfiLimiterConfig {
                ceiling: 0.5,
                release_ms: 75.0,
            },
            ..FfiHostConfig::default()
        };
        let HostConfig::Realtime { limiter, .. } = wire.into_domain::<FfiPools>().unwrap() else {
            panic!("FFI host settings select the realtime session");
        };
        assert_eq!(limiter.ceiling(), 0.5);
        assert_eq!(limiter.release_ms(), 75.0);
    }
}
