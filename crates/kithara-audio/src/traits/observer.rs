use kithara_decode::PcmChunk;

mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

/// Why a bounded decoded-PCM observer did not accept a chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum PcmObserveError {
    /// The observer's bounded input is full.
    #[error("decoded PCM observer input is full")]
    Full,
    /// The observer's consumer is no longer available.
    #[error("decoded PCM observer is closed")]
    Closed,
}

/// Optional best-effort observer of decoder-output PCM.
/// [`PcmChunk::meta`] is authoritative after decoder-side conversion; intake
/// must be bounded and nonblocking, and rejection never affects playback.
#[kithara::mock(api = PcmObserverMock)]
pub trait PcmObserver: Send + 'static {
    /// Try to observe one decoded chunk without taking ownership of its pooled buffer.
    ///
    /// # Errors
    ///
    /// Returns [`PcmObserveError::Full`] when the bounded input is saturated,
    /// or [`PcmObserveError::Closed`] when its consumer has stopped.
    fn try_observe(&mut self, chunk: &PcmChunk) -> Result<(), PcmObserveError>;
}
