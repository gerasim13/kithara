#[cfg(any(test, feature = "mock"))]
use kithara_decode::PcmChunk;
use kithara_platform::sync::Arc;
use kithara_stream::SeekObserve;

use crate::TrackStep;

mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

/// Worker-independent source of decoded PCM chunks.
///
/// Each step advances at most one source transition; scheduling belongs to the executor.
#[kithara::mock(api = PcmSourceMock, type Chunk = PcmChunk;)]
pub trait PcmSource: Send + 'static {
    type Chunk: Send + 'static;

    /// Decode epoch assigned to the most recent source work.
    /// May lag the live seek epoch until the source applies the seek.
    fn decode_epoch(&self) -> u64 {
        self.seek_observe().epoch()
    }

    /// Deliver off-core signals armed by previous source steps.
    fn flush_deferred(&mut self) {}

    /// Return a discarded pooled chunk for off-core reclamation.
    fn retire_chunk(&self, chunk: Self::Chunk) {
        let _ = chunk;
    }

    /// Narrow seek-observe handle for epoch queries and the decoder seek latch.
    fn seek_observe(&self) -> Arc<dyn SeekObserve>;

    /// Advance the source FSM by at most one transition.
    fn step_track(&mut self) -> TrackStep<Self::Chunk>;

    /// One-time execution-thread warmup before the first checked source step.
    fn warm_up(&mut self) {}
}
