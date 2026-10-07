//! Lower player-to-host session protocol.

mod wire {
    use std::num::NonZeroUsize;

    use kithara_bufpool::PoolRegion;
    use kithara_events::EventBus;
    use kithara_render::{
        bridge::SlotControl,
        rt::{BufferGeometryError, DeckMixerConfig},
    };
    use kithara_warp::{BeatGridId, BeatGridIdAllocationError};

    use crate::api::SlotId;

    pub type PlayerId = u64;

    #[derive(Debug, Clone, thiserror::Error)]
    #[non_exhaustive]
    pub enum SessionError {
        #[error("player not found: {0}")]
        PlayerNotFound(PlayerId),
        #[error("player identity space is exhausted")]
        PlayerIdExhausted,
        #[error("player already started: {0}")]
        AlreadyStarted(PlayerId),
        #[error("player not running: {0}")]
        NotRunning(PlayerId),
        #[error("session context not initialised")]
        NoContext,
        #[error("stream start failed: {0}")]
        StreamStart(String),
        #[error("graph edit failed: {0}")]
        Graph(String),
        #[error("session output tap already has a consumer")]
        TapActive,
        #[error("session transport has not been processed")]
        TransportNotProcessed,
        #[error("host command queue is full")]
        HostQueueFull,
        #[error(transparent)]
        BufferGeometry(#[from] BufferGeometryError),
        #[error("deck {0:?} is not in this session")]
        DeckNotFound(BeatGridId),
        #[error("deck {0:?} is already in this session")]
        DeckAttached(BeatGridId),
        #[error(transparent)]
        BeatGridIdAllocation(#[from] BeatGridIdAllocationError),
        #[error("stream stopped: {reason}; restart failed: {source}")]
        RestartFailed { reason: String, r#source: String },
    }

    /// What a deck joins its session with. The Host registers the deck from it
    /// and builds the deck's slot, answering it as [`AllocatedSlot`].
    #[non_exhaustive]
    pub struct DeckRegistration<S> {
        pub grid_id: BeatGridId,
        pub bus: EventBus,
        pub mixer: DeckMixerConfig,
        pub pools: PoolRegion<S>,
        pub render_quantum_frames: Option<NonZeroUsize>,
        pub response_budget_frames: Option<NonZeroUsize>,
    }

    impl<S> DeckRegistration<S> {
        /// A deck that asks the session for no playback-buffer geometry.
        #[must_use]
        pub const fn new(
            grid_id: BeatGridId,
            bus: EventBus,
            pools: PoolRegion<S>,
            mixer: DeckMixerConfig,
        ) -> Self {
            Self {
                grid_id,
                bus,
                mixer,
                pools,
                render_quantum_frames: None,
                response_budget_frames: None,
            }
        }
    }

    /// What the session knows about its output rate.
    #[derive(Clone, Copy)]
    #[non_exhaustive]
    pub struct SessionSampleRate {
        /// The current Firewheel output rate; `None` means no output is measured.
        pub measured: Option<u32>,
        /// The rate the session last asked the device for.
        pub requested: u32,
    }

    impl SessionSampleRate {
        #[must_use]
        pub const fn new(measured: Option<u32>, requested: u32) -> Self {
            Self {
                measured,
                requested,
            }
        }

        /// The rate to build a resampler for.
        #[must_use]
        pub const fn output(self) -> u32 {
            match self.measured {
                Some(measured) => measured,
                None => self.requested,
            }
        }
    }

    #[non_exhaustive]
    pub struct AllocatedSlot {
        pub control: SlotControl,
        pub slot: SlotId,
    }

    impl AllocatedSlot {
        #[must_use]
        pub fn new(control: SlotControl, slot: SlotId) -> Self {
            Self { control, slot }
        }
    }
}

mod binding {
    use std::num::NonZeroU32;

    use arc_swap::ArcSwap;
    use kithara_audio::ConsumerWakeMode;
    use kithara_platform::sync::Arc;
    use kithara_render::rt::StreamShape;

    use super::wire::SessionSampleRate;

    struct OutputSnapshot {
        sample_rate: SessionSampleRate,
        stream_shape: Option<StreamShape>,
    }

    /// Where a session publishes its output for the decks it holds to read.
    #[derive(Clone)]
    pub struct SessionOutputView(Arc<ArcSwap<OutputSnapshot>>);

    impl SessionOutputView {
        /// A session that has published no output yet: nothing measured, no
        /// shape, the rate its settings ask for.
        #[must_use]
        pub fn new(requested_sample_rate: NonZeroU32) -> Self {
            Self(Arc::new(ArcSwap::from_pointee(OutputSnapshot {
                sample_rate: SessionSampleRate::new(None, requested_sample_rate.get()),
                stream_shape: None,
            })))
        }

        /// The session's output changed: every deck it holds reads this from
        /// now on.
        pub fn publish(&self, sample_rate: SessionSampleRate, stream_shape: Option<StreamShape>) {
            self.0.store(Arc::new(OutputSnapshot {
                sample_rate,
                stream_shape,
            }));
        }

        delegate::delegate! {
            to self.0 {
                /// The output rate the session last published: the rate the
                /// running backend settled on, beside the one the settings ask
                /// for.
                #[must_use]
                #[call(load)]
                #[expr($.sample_rate)]
                pub fn sample_rate(&self) -> SessionSampleRate;
                /// The output shape the session last published: the measured
                /// stream once one runs, the requested block before.
                #[must_use]
                #[call(load)]
                #[expr($.stream_shape)]
                pub fn stream_shape(&self) -> Option<StreamShape>;
            }
        }
    }

    /// What a player joins its session with, once.
    ///
    /// Decorators may only pass it down to their resident Player.
    #[derive(Clone, fieldwork::Fieldwork)]
    #[fieldwork(opt_in)]
    pub struct SessionBinding {
        output: SessionOutputView,
        /// How the audio consumers the player hosts may wake workers.
        #[field(get, copy, vis = "pub(crate)")]
        consumer_wake_mode: ConsumerWakeMode,
        /// The rate the owner's settings name when the player joins.
        #[field(get, copy, vis = "pub(crate)")]
        requested_sample_rate: NonZeroU32,
    }

    impl SessionBinding {
        /// A binding to the session that publishes `output`.
        ///
        /// Every audio consumer the player hosts reads from the session's
        /// render callback and wakes its workers as `consumer_wake_mode` says.
        /// The rate is the one the owner's settings name when the player
        /// joins; a player built for another rate is refused. The session
        /// starts its output at the rate its settings name then, not at this
        /// copy.
        #[doc(hidden)]
        #[must_use]
        pub const fn new(
            output: SessionOutputView,
            consumer_wake_mode: ConsumerWakeMode,
            requested_sample_rate: NonZeroU32,
        ) -> Self {
            Self {
                output,
                consumer_wake_mode,
                requested_sample_rate,
            }
        }

        delegate::delegate! {
            to self.output {
                pub(crate) fn sample_rate(&self) -> SessionSampleRate;
                pub(crate) fn stream_shape(&self) -> Option<StreamShape>;
            }
        }
    }
}

pub use binding::{SessionBinding, SessionOutputView};
pub use wire::{AllocatedSlot, DeckRegistration, PlayerId, SessionError, SessionSampleRate};

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::ConsumerWakeMode;
    use kithara_test_utils::kithara;

    use super::{SessionBinding, SessionOutputView, SessionSampleRate};

    fn sample_rate() -> NonZeroU32 {
        NonZeroU32::new(48_000).expect("fixture sample rate is non-zero")
    }

    #[kithara::test]
    fn a_binding_reads_what_its_session_publishes_after_it_binds() {
        let output = SessionOutputView::new(sample_rate());
        let binding = SessionBinding::new(
            output.clone(),
            ConsumerWakeMode::RealtimeDeferred,
            sample_rate(),
        );
        assert_eq!(binding.sample_rate().measured, None);

        output.publish(SessionSampleRate::new(Some(44_100), 48_000), None);

        assert_eq!(binding.sample_rate().measured, Some(44_100));
        assert_eq!(binding.sample_rate().output(), 44_100);
    }
}
