use kithara_signal::AudioChunk;

mod kithara {
    pub(crate) use kithara_test_macros::mock;
}

/// Why a bounded decoded-audio observer did not accept a chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum AudioObserveError {
    /// The observer's bounded input is full.
    #[error("decoded PCM observer input is full")]
    Full,
    /// The observer's consumer is no longer available.
    #[error("decoded PCM observer is closed")]
    Closed,
}

/// Optional best-effort observer of decoder output.
/// [`AudioChunk::meta`] is authoritative after decoder-side conversion; intake
/// must be bounded and nonblocking, and rejection never affects playback.
#[kithara::mock(api = AudioObserverMock)]
pub trait AudioObserver: Send + 'static {
    /// Try to observe one decoded chunk without taking ownership of its pooled buffer.
    ///
    /// # Errors
    ///
    /// Returns [`AudioObserveError::Full`] when the bounded input is saturated,
    /// or [`AudioObserveError::Closed`] when its consumer has stopped.
    fn try_observe(&mut self, chunk: &AudioChunk) -> Result<(), AudioObserveError>;
}
