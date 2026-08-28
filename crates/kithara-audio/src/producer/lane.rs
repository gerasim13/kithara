use kithara_events::{DeferredBus, Event};
use kithara_platform::sync::Arc;
use kithara_signal::AudioChunk;
use kithara_stream::PlayheadWrite;

use super::PreloadGate;
use crate::{
    Fetch,
    runtime::{Inlet, Outlet},
};

/// Concrete playback output port prepared by `kithara-audio` and driven by
/// the play-owned producer node.
#[doc(hidden)]
pub struct ProducerPort {
    outlet: Outlet<Fetch<AudioChunk>>,
    trash_inlet: Inlet<AudioChunk>,
}

impl ProducerPort {
    pub(crate) const fn new(
        outlet: Outlet<Fetch<AudioChunk>>,
        trash_inlet: Inlet<AudioChunk>,
    ) -> Self {
        Self {
            outlet,
            trash_inlet,
        }
    }

    delegate::delegate! {
        to self.outlet {
            /// Forward a parked item after the consumer frees ring capacity.
            #[must_use]
            pub fn flush(&mut self) -> bool;
            /// Push one produced item into the final playback ring.
            #[call(try_push)]
            #[expr($.is_ok())]
            #[must_use]
            pub fn try_push(&mut self, item: Fetch<AudioChunk>) -> bool;
            /// Report whether one item is parked in the overflow slot.
            #[must_use]
            pub const fn has_pending(&self) -> bool;
            /// Remove the item parked in the overflow slot.
            #[must_use]
            pub const fn take_pending(&mut self) -> Option<Fetch<AudioChunk>>;
            /// Deliver deferred wake signals outside the checked producer core.
            #[call(flush_wake_signals)]
            pub fn flush_wake(&self);
        }
    }

    /// Reclaim spent chunks outside the checked producer core.
    pub fn recycle(&mut self) {
        while self.trash_inlet.try_pop().is_some() {}
    }

    /// Create an isolated port and a consumer probe for unit tests.
    #[cfg(any(test, feature = "probe"))]
    pub fn probe(
        capacity: usize,
    ) -> (
        Self,
        impl FnMut() -> Option<Fetch<AudioChunk>> + Send + 'static,
    ) {
        let (outlet, mut inlet) = crate::runtime::connect(capacity, None);
        let (_trash_outlet, trash_inlet) = crate::runtime::connect(capacity + 2, None);
        (Self::new(outlet, trash_inlet), move || inlet.try_pop())
    }
}

/// Worker-neutral playback lane prepared alongside an [`crate::Audio`]
/// reader. `kithara-play` composes the concrete source and owns its node.
#[doc(hidden)]
#[non_exhaustive]
pub struct PreparedAudioLane<S> {
    /// Deferred event publisher shared with the reader.
    pub emit: Arc<DeferredBus<Event>>,
    /// Canonical playback clock written after final audio admission.
    pub playhead: Arc<dyn PlayheadWrite>,
    /// Gate opened when the final audio ring is preloaded.
    pub preload_gate: Arc<PreloadGate>,
    /// Still-concrete producer source.
    pub source: S,
    /// Final output and spent-buffer return port.
    pub port: ProducerPort,
    /// Number of admitted chunks required before preload completes.
    pub preload_chunks: usize,
}

impl<S> PreparedAudioLane<S> {
    pub(crate) fn map_source_with<A, R, W, F>(
        self,
        auxiliary: A,
        map: F,
    ) -> (R, PreparedAudioLane<W>)
    where
        F: FnOnce(A, S) -> (R, W),
    {
        let (result, source) = map(auxiliary, self.source);
        (
            result,
            PreparedAudioLane {
                emit: self.emit,
                playhead: self.playhead,
                preload_gate: self.preload_gate,
                source,
                port: self.port,
                preload_chunks: self.preload_chunks,
            },
        )
    }
}
