use super::{ElasticConfig, ElasticError, ElasticLatency, ElasticRateEnvelope, ElasticRequest};

/// Immutable limits, latency and rate window of a prepared elastic engine.
/// Every value is declared by the engine that reports it, so a caller plans
/// against capabilities instead of against a specific backend.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct ElasticCapabilities {
    #[field(skip)]
    config: ElasticConfig,
    /// Fixed algorithmic latency in both coordinate spaces.
    #[field(get, copy)]
    latency: ElasticLatency,
    /// Supported source-frame advance range.
    #[field(get, copy)]
    rate_envelope: ElasticRateEnvelope,
}

impl ElasticCapabilities {
    pub(crate) const fn new(
        config: ElasticConfig,
        latency: ElasticLatency,
        rate_envelope: ElasticRateEnvelope,
    ) -> Self {
        Self {
            config,
            latency,
            rate_envelope,
        }
    }

    /// Interleaved sample count of a frame span at the prepared channel count.
    pub(crate) fn samples(self, frames: usize) -> Result<usize, ElasticError> {
        frames
            .checked_mul(self.channels())
            .ok_or(ElasticError::SampleCountOverflow)
    }

    /// Every engine accepts the same requests: inside the prepared block
    /// limits, matching the buffers it was handed, and inside the declared
    /// rate envelope.
    pub(crate) fn validate(
        self,
        request: ElasticRequest,
        source_samples: usize,
        output_samples: usize,
    ) -> Result<(), ElasticError> {
        if request.source_frames() > self.max_source_frames() {
            return Err(ElasticError::SourceFrameLimit {
                frames: request.source_frames(),
                limit: self.max_source_frames(),
            });
        }
        if request.output_frames() > self.max_output_frames() {
            return Err(ElasticError::OutputFrameLimit {
                frames: request.output_frames(),
                limit: self.max_output_frames(),
            });
        }
        self.validate_spans(request, source_samples, output_samples)
    }

    /// Buffer shape and rate checks shared by rendering and priming; priming
    /// spans are bounded by the declared latency rather than by the block
    /// limits, so it validates these without the limit checks.
    pub(crate) fn validate_spans(
        self,
        request: ElasticRequest,
        source_samples: usize,
        output_samples: usize,
    ) -> Result<(), ElasticError> {
        let expected_source_samples = self.samples(request.source_frames())?;
        if source_samples != expected_source_samples {
            return Err(ElasticError::SourceSampleCount {
                actual: source_samples,
                expected: expected_source_samples,
            });
        }
        let expected_output_samples = self.samples(request.output_frames())?;
        if output_samples != expected_output_samples {
            return Err(ElasticError::OutputSampleCount {
                actual: output_samples,
                expected: expected_output_samples,
            });
        }
        if self.rate_envelope().contains(request) {
            Ok(())
        } else {
            Err(ElasticError::RateOutsideEnvelope {
                source_frames: request.source_frames(),
                output_frames: request.output_frames(),
            })
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
}
