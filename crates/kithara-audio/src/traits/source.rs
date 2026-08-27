#[cfg(any(test, feature = "mock"))]
use kithara_decode::PcmChunk;
use kithara_decode::PcmSpec;
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

    /// Current explicit source discontinuity, when the source has one.
    fn discontinuity(&self) -> Option<SourceDiscontinuity> {
        None
    }

    /// Resolve the active output format before producer decorators are serviced.
    /// Sources without a split shell keep the default no-op phases.
    fn prepare_deferred(&mut self) -> Option<PcmSpec> {
        None
    }

    /// Finish deferred source publication after decorators are serviced.
    fn finish_deferred(&mut self) {}

    /// Reclaim a discarded chunk from scheduler `recycle`, outside the checked
    /// producer tick.
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

#[cfg(test)]
pub(crate) trait PcmSourceExt: PcmSource {
    fn flush_deferred(&mut self) {
        let _ = self.prepare_deferred();
        self.finish_deferred();
    }
}

#[cfg(test)]
impl<S> PcmSourceExt for S where S: PcmSource + ?Sized {}
/// Exact worker-side reset stamp for a decoded PCM lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get, copy)]
#[non_exhaustive]
pub struct SourceDiscontinuity {
    /// Monotonic lane-local reset revision.
    revision: u64,
    /// Output format active after the reset.
    spec: PcmSpec,
}

impl SourceDiscontinuity {
    /// Construct a reset stamp at the active decoded format.
    #[must_use]
    pub const fn new(revision: u64, spec: PcmSpec) -> Self {
        Self { revision, spec }
    }
}
