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

    /// Flush one remaining buffered chunk at end of stream.
    ///
    /// Repeated calls must reach `None` after a finite number of chunks and
    /// remain exhausted until [`process`](Self::process) or
    /// [`reset`](Self::reset) starts a new effect lifecycle. Each call is a
    /// bounded real-time transition: it must not block, allocate, free, or
    /// rebuild backend state.
    fn flush(&mut self) -> Option<PcmChunk>;

    /// Process a PCM chunk, returning transformed output.
    ///
    /// Returns `None` if the effect is accumulating data (not enough for output
    /// yet). This is a bounded real-time transition and must not block,
    /// allocate, free, or rebuild backend state.
    fn process(&mut self, chunk: PcmChunk) -> Option<PcmChunk>;

    /// Reset internal state after a seek using only bounded real-time work.
    /// Allocation, destruction, and backend rebuilding belong in
    /// [`service_deferred`](Self::service_deferred).
    fn reset(&mut self);
}
