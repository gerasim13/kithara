use kithara_decode::{PcmChunk, PcmSpec};

mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

/// Audio processing effect in the chain (transforms PCM chunks).
#[kithara::mock(api = AudioEffectMock)]
pub trait AudioEffect: Send + 'static {
    /// Service work deferred by the checked audio-production path.
    ///
    /// The scheduler shell calls this after lifecycle transitions, outside
    /// `produce_tick_rt`, and before the next checked tick, using the active
    /// decoder specification. Effects without deferred work can keep the
    /// default no-op implementation.
    fn service_deferred(&mut self, spec: PcmSpec) {
        let _ = spec;
    }

    /// Flush remaining buffered data (called at end of stream).
    fn flush(&mut self) -> Option<PcmChunk>;

    /// Process a PCM chunk, returning transformed output.
    ///
    /// Returns `None` if the effect is accumulating data (not enough for output yet).
    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk>;

    /// Reset internal state (called after seek).
    fn reset(&mut self);
}
