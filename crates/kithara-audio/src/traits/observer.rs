use std::num::NonZeroU32;

use crossbeam_queue::ArrayQueue;
use kithara_platform::sync::Arc;
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
    /// The observer is bound to a different sample-rate axis.
    #[error("decoded PCM sample rate {actual} does not match observer rate {expected}")]
    UnsupportedSampleRate {
        expected: NonZeroU32,
        actual: NonZeroU32,
    },
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
    /// [`AudioObserveError::Closed`] when its consumer has stopped, or
    /// [`AudioObserveError::UnsupportedSampleRate`] when the chunk is measured
    /// on an incompatible sample-rate axis.
    fn try_observe(&mut self, chunk: &AudioChunk) -> Result<(), AudioObserveError>;
}

/// Persistent per-track slot for a decoded-audio observer.
///
/// The capacity is one and replacement is latest-wins: a pass restarted before
/// the decoder's next chunk replaces the obsolete producer rather than being
/// stranded behind it. The slot survives individual resource lifetimes.
#[doc(hidden)]
#[derive(Clone)]
pub struct AudioObserverSlot {
    pending: Arc<ArrayQueue<Box<dyn AudioObserver>>>,
}

impl AudioObserverSlot {
    /// Make `observer` the next one adopted by the decoder.
    pub fn attach(&self, observer: Box<dyn AudioObserver>) {
        drop(self.pending.force_push(observer));
    }

    /// Create a decoder-side relay over this persistent slot.
    #[must_use]
    pub fn relay(&self) -> AudioObserverRelay {
        AudioObserverRelay {
            pending: Arc::clone(&self.pending),
            observer: None,
        }
    }
}

impl Default for AudioObserverSlot {
    fn default() -> Self {
        Self {
            pending: Arc::new(ArrayQueue::new(1)),
        }
    }
}

/// Decoder-side relay whose attachment channel is bounded and lock-free.
///
/// The decoder owns this half for its whole lifetime. Until an observer is
/// attached it is a no-op; afterwards it forwards chunks directly without
/// retaining their pooled buffers.
#[doc(hidden)]
pub struct AudioObserverRelay {
    pending: Arc<ArrayQueue<Box<dyn AudioObserver>>>,
    observer: Option<Box<dyn AudioObserver>>,
}

impl AudioObserver for AudioObserverRelay {
    fn try_observe(&mut self, chunk: &AudioChunk) -> Result<(), AudioObserveError> {
        if let Some(observer) = self.pending.pop() {
            self.observer = Some(observer);
        }
        let Some(observer) = &mut self.observer else {
            return Ok(());
        };
        let result = observer.try_observe(chunk);
        if matches!(result, Err(AudioObserveError::Closed)) {
            self.observer = None;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kithara_platform::sync::Arc;
    use kithara_signal::{AudioChunk, AudioChunkInfo};
    use kithara_test_utils::kithara;

    use super::{AudioObserveError, AudioObserver, AudioObserverSlot};
    use crate::test_pools::sample_buffer;

    struct CountingObserver(Arc<AtomicUsize>);

    impl AudioObserver for CountingObserver {
        fn try_observe(&mut self, _chunk: &AudioChunk) -> Result<(), AudioObserveError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[kithara::test]
    fn replacement_before_the_first_chunk_is_latest_wins() {
        let slot = AudioObserverSlot::default();
        let mut relay = slot.relay();
        let replaced = Arc::new(AtomicUsize::new(0));
        let current = Arc::new(AtomicUsize::new(0));
        slot.attach(Box::new(CountingObserver(Arc::clone(&replaced))));
        slot.attach(Box::new(CountingObserver(Arc::clone(&current))));

        let chunk = AudioChunk::new(AudioChunkInfo::default(), sample_buffer(&[]));
        relay
            .try_observe(&chunk)
            .expect("the current observer accepts the chunk");

        assert_eq!(replaced.load(Ordering::Relaxed), 0);
        assert_eq!(current.load(Ordering::Relaxed), 1);
    }
}
