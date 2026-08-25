#[cfg(not(any(feature = "stretch-signalsmith", feature = "stretch-bungee")))]
compile_error!(
    "kithara-stretch requires at least one backend feature: \
     enable stretch-signalsmith (default) or stretch-bungee. \
     A build with no stretch backend should not depend on this crate."
);

#[cfg(all(feature = "stretch-signalsmith", target_arch = "wasm32"))]
compile_error!("kithara-stretch: stretch-signalsmith is unavailable on wasm32 targets");

#[cfg(all(
    feature = "stretch-bungee",
    any(
        target_arch = "wasm32",
        all(target_os = "windows", target_env = "msvc")
    )
))]
compile_error!("kithara-stretch: stretch-bungee is unavailable on this target");

mod backend;
pub use backend::{StretchBackend, StretchBackendError};

mod config;
pub use config::StretchOptions;

mod kind;
pub use kind::StretchKind;

mod factory;
pub use factory::build_backend;

mod backends;
#[cfg(all(
    feature = "stretch-bungee",
    not(target_arch = "wasm32"),
    not(all(target_os = "windows", target_env = "msvc"))
))]
pub use backends::BungeeElastic;
#[cfg(all(feature = "stretch-signalsmith", not(target_arch = "wasm32")))]
pub use backends::SignalsmithElastic;

mod elastic;
pub use elastic::{
    ElasticCapabilities, ElasticConfig, ElasticCursor, ElasticEngine, ElasticError, ElasticLatency,
    ElasticPriming, ElasticRateEnvelope, ElasticRequest, ElasticSpan, ElasticSpanConfig,
    ElasticSpanPlan, ElasticSpanRequest,
};
