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

    #[derive(Clone, Copy)]
    pub enum Cmd {
        Tick,
    }

    #[non_exhaustive]
    pub enum Reply {
        Ok,
        Err(SessionError),
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

mod handle {
    use std::{
        num::NonZeroU32,
        sync::atomic::{AtomicU64, Ordering},
    };

    use kithara_audio::ConsumerWakeMode;
    use kithara_platform::{
        maybe_send::{MaybeSend, MaybeSync},
        sync::{Arc, Mutex},
    };
    use kithara_render::rt::StreamShape;

    use super::wire::{Cmd, Reply, SessionSampleRate};
    use crate::error::PlayError;

    /// Handle used by resident players to reach their session owner.
    ///
    /// The handle stays inside the thread that built it: on wasm that is one
    /// worker, so the bound is [`MaybeSend`], which is `Send` on every
    /// threaded target and nothing on wasm.
    pub trait SessionDispatcher<S>: MaybeSend + MaybeSync {
        /// Describe how audio consumers hosted by this session may wake workers.
        /// Every one of them reads from the render callback, offline backends
        /// included.
        fn consumer_wake_mode(&self) -> ConsumerWakeMode;

        fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError>;

        fn exec_ok(&self, cmd: Cmd) -> Result<Reply, PlayError> {
            match self.exec(cmd)? {
                Reply::Err(err) => Err(err.into()),
                reply => Ok(reply),
            }
        }

        /// The output rate the session last published: the rate the running
        /// backend settled on, beside the one the settings ask for.
        fn sample_rate(&self) -> SessionSampleRate;

        /// The output shape the session last published: the measured stream
        /// once one runs, the requested block before.
        fn stream_shape(&self) -> Option<StreamShape>;
    }

    /// Opaque one-shot capability used to attach a Player to its session.
    ///
    /// The dispatcher is deliberately inaccessible: decorators may only pass
    /// this capability down to their resident Player.
    #[derive_where::derive_where(Clone)]
    pub struct SessionBinding<S> {
        dispatcher: Arc<dyn SessionDispatcher<S>>,
        requested_sample_rate: NonZeroU32,
    }

    impl<S> SessionBinding<S> {
        /// Wraps the canonical session for one Host insertion.
        ///
        /// The rate is the one the owner's settings name when the player
        /// joins; a player built for another rate is refused. The session
        /// starts its output at the rate its settings name then, not at this
        /// copy.
        #[doc(hidden)]
        #[must_use]
        pub fn new(
            dispatcher: Arc<dyn SessionDispatcher<S>>,
            requested_sample_rate: NonZeroU32,
        ) -> Self {
            Self {
                dispatcher,
                requested_sample_rate,
            }
        }

        #[must_use]
        pub(crate) fn requested_sample_rate(&self) -> NonZeroU32 {
            self.requested_sample_rate
        }
    }

    struct SessionSlot<S> {
        /// The audio-thread tick the platform suspended this output at, plus
        /// one; `0` means the output was never taken away.
        suspended_at: AtomicU64,
        binding: Mutex<Option<SessionBinding<S>>>,
    }

    #[derive_where::derive_where(Clone)]
    pub struct SessionHandle<S>(Arc<SessionSlot<S>>);

    impl<S> SessionHandle<S> {
        #[must_use]
        pub fn new(binding: SessionBinding<S>) -> Self {
            Self(Arc::new(SessionSlot {
                binding: Mutex::new(Some(binding)),
                suspended_at: AtomicU64::new(0),
            }))
        }

        pub(crate) fn bind(&self, binding: SessionBinding<S>) -> Result<(), PlayError> {
            let mut current = self.0.binding.lock();
            if current.is_some() {
                return Err(PlayError::SessionAlreadyBound);
            }
            *current = Some(binding);
            drop(current);
            Ok(())
        }

        /// An instance may prepare resources before Host insertion, so the pending policy defaults
        /// to the RT-safe production path; explicit offline dispatchers override it once bound.
        #[must_use]
        pub fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            self.dispatcher()
                .map_or(ConsumerWakeMode::RealtimeDeferred, |dispatcher| {
                    dispatcher.consumer_wake_mode()
                })
        }

        pub fn dispatcher(&self) -> Result<Arc<dyn SessionDispatcher<S>>, PlayError> {
            self.0
                .binding
                .lock()
                .as_ref()
                .map(|binding| Arc::clone(&binding.dispatcher))
                .ok_or(PlayError::SessionUnbound)
        }

        pub fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError> {
            self.dispatcher()?.exec(cmd)
        }

        pub fn exec_ok(&self, cmd: Cmd) -> Result<Reply, PlayError> {
            match self.exec(cmd)? {
                Reply::Err(err) => Err(err.into()),
                reply => Ok(reply),
            }
        }

        #[must_use]
        pub(crate) fn pending() -> Self {
            Self(Arc::new(SessionSlot {
                binding: Mutex::default(),
                suspended_at: AtomicU64::new(0),
            }))
        }

        pub(crate) fn stream_shape(&self) -> Option<StreamShape> {
            let dispatcher = self
                .0
                .binding
                .lock()
                .as_ref()
                .map(|binding| Arc::clone(&binding.dispatcher));
            dispatcher.and_then(|dispatcher| dispatcher.stream_shape())
        }

        /// Record that the platform suspended the output at `tick`.
        pub fn suspend_output(&self, tick: u64) {
            self.0
                .suspended_at
                .store(tick.saturating_add(1), Ordering::Release);
        }

        /// The audio-thread tick this output was suspended at, if the platform
        /// has taken it away and has not driven it since.
        ///
        /// A suspended output leaves the RT processor unscheduled, so every
        /// value it publishes stays at whatever it last wrote. The tick is how
        /// a reader tells the two apart: while the audio thread still stands
        /// where it stood, its publications describe an output that is gone.
        /// One tick past it the processor has drained the commands sent before
        /// the suspension, but it counts a call as the call starts and
        /// publishes as it ends, so a reader may look in between. Two ticks
        /// past it that call has published, so the output speaks for itself
        /// again and nothing needs to release it.
        #[must_use]
        pub fn suspended_at(&self) -> Option<u64> {
            match self.0.suspended_at.load(Ordering::Acquire) {
                0 => None,
                tick => Some(tick - 1),
            }
        }

        pub fn tick(&self) -> Result<(), PlayError> {
            self.exec_ok(Cmd::Tick).map(|_| ())
        }

        delegate::delegate! {
            to self.dispatcher()? {
                #[expr(Ok($))]
                pub fn sample_rate(&self) -> Result<SessionSampleRate, PlayError>;
            }
        }
    }
}

pub use handle::{SessionBinding, SessionDispatcher, SessionHandle};
pub use wire::{
    AllocatedSlot, Cmd, DeckRegistration, PlayerId, Reply, SessionError, SessionSampleRate,
};

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_audio::ConsumerWakeMode;
    use kithara_platform::sync::Arc;
    use kithara_render::rt::StreamShape;
    use kithara_test_utils::kithara;

    use super::{Cmd, Reply, SessionBinding, SessionDispatcher, SessionHandle, SessionSampleRate};
    use crate::{PlayError, test_pools::TestPools};

    struct DefaultSession;

    fn sample_rate() -> NonZeroU32 {
        NonZeroU32::new(48_000).expect("fixture sample rate is non-zero")
    }

    impl SessionDispatcher<TestPools> for DefaultSession {
        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }

        fn exec(&self, _cmd: Cmd) -> Result<Reply, PlayError> {
            Ok(Reply::Ok)
        }

        fn sample_rate(&self) -> SessionSampleRate {
            SessionSampleRate::new(None, sample_rate().get())
        }

        fn stream_shape(&self) -> Option<StreamShape> {
            None
        }
    }

    #[kithara::test]
    fn session_handle_delegates_explicit_consumer_wake_mode() {
        let handle: SessionHandle<TestPools> =
            SessionHandle::new(SessionBinding::new(Arc::new(DefaultSession), sample_rate()));

        assert_eq!(
            handle.consumer_wake_mode(),
            ConsumerWakeMode::RealtimeDeferred
        );
    }

    #[kithara::test]
    fn pending_session_binds_once() {
        let handle: SessionHandle<TestPools> = SessionHandle::pending();
        assert_eq!(
            handle.consumer_wake_mode(),
            ConsumerWakeMode::RealtimeDeferred
        );
        assert!(matches!(
            handle.exec(Cmd::Tick),
            Err(PlayError::SessionUnbound)
        ));

        handle
            .bind(SessionBinding::new(Arc::new(DefaultSession), sample_rate()))
            .expect("bind canonical session");
        assert_eq!(
            handle.consumer_wake_mode(),
            ConsumerWakeMode::RealtimeDeferred
        );
        assert!(matches!(handle.exec(Cmd::Tick), Ok(Reply::Ok)));
        assert!(matches!(
            handle.bind(SessionBinding::new(Arc::new(DefaultSession), sample_rate())),
            Err(PlayError::SessionAlreadyBound)
        ));
    }
}
