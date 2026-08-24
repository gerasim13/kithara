#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
mod backend;

#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
mod config;

#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
mod kind;

#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
mod factory;

#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
mod backends;
#[cfg(feature = "stretch-bungee")]
pub use backends::BungeeElastic;
#[cfg(feature = "stretch-signalsmith")]
pub use backends::SignalsmithElastic;
#[cfg(any(feature = "stretch-signalsmith", feature = "stretch-bungee"))]
pub use {
    backend::{StretchBackend, StretchBackendError},
    config::StretchOptions,
    factory::build_backend,
    kind::StretchKind,
};

mod elastic;
pub use elastic::{
    ElasticCapabilities, ElasticConfig, ElasticCursor, ElasticEngine, ElasticError, ElasticLatency,
    ElasticPriming, ElasticRateEnvelope, ElasticRequest, ElasticSpan, ElasticSpanConfig,
    ElasticSpanPlan, ElasticSpanRequest,
};
