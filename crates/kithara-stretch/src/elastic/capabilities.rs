use super::{ElasticConfig, ElasticLatency, ElasticRateEnvelope};

/// Immutable limits and latency of a prepared elastic engine.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ElasticCapabilities {
    config: ElasticConfig,
    latency: ElasticLatency,
    rate_envelope: ElasticRateEnvelope,
}

impl ElasticCapabilities {
    #[cfg(feature = "stretch-signalsmith")]
    pub(crate) const fn new(config: ElasticConfig, latency: ElasticLatency) -> Self {
        Self {
            config,
            latency,
            rate_envelope: ElasticRateEnvelope::signalsmith(),
        }
    }

    delegate::delegate! {
        to self.config {
            /// Prepared interleaved channel count.
            #[must_use]
            pub fn channels(&self) -> usize;
            /// Largest accepted output block in frames.
            #[must_use]
            pub fn max_output_frames(&self) -> usize;
            /// Largest accepted source block in frames.
            #[must_use]
            pub fn max_source_frames(&self) -> usize;
            /// Prepared source sample rate in Hz.
            #[must_use]
            pub fn sample_rate(&self) -> u32;
        }
    }

    /// Fixed algorithmic latency in both coordinate spaces.
    #[must_use]
    pub const fn latency(self) -> ElasticLatency {
        self.latency
    }

    /// Supported source-frame advance range.
    #[must_use]
    pub const fn rate_envelope(self) -> ElasticRateEnvelope {
        self.rate_envelope
    }
}
