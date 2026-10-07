use std::sync::atomic::{AtomicU64, Ordering};

use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_effects::{
    GainDb,
    eq::{EqBandConfig, EqConfig, EqLayout},
};
use kithara_events::{EventBus, EventReceiver, EventSet};
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use kithara_render::{
    bridge::{DeckEqChange, DeckPart, DeckTrash, PlaybackShared, PlayerNotification, SlotControl},
    rt::StreamShape,
};
use kithara_warp::RenderSnapshot;
use ringbuf::traits::Consumer;
use tracing::info;

use super::{config::EngineConfig, slots::DeckSlot};
use crate::{
    api::{EngineEvent, SlotId},
    error::PlayError,
    session::{AllocatedSlot, DeckRegistration, SessionBinding},
};

type SlotHandle = SlotControl;

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct EngineImpl<S> {
    config: EngineConfig<S>,
    #[field(get, vis = "pub(crate)")]
    bus: EventBus,
    slot: Mutex<DeckSlot>,
    session: OnceLock<SessionBinding>,
    /// The audio-thread tick the platform suspended this output at, plus
    /// one; `0` means the output was never taken away.
    suspended_at: AtomicU64,
}

impl<S> EngineImpl<S> {
    /// Create a new engine with the given configuration.
    #[must_use]
    pub fn new(mut config: EngineConfig<S>, bus: EventBus) -> Self {
        let session = config
            .session
            .take()
            .map_or_else(OnceLock::new, OnceLock::from);
        Self {
            config,
            bus,
            session,
            slot: Mutex::default(),
            suspended_at: AtomicU64::new(0),
        }
    }

    pub(crate) fn attach_session(&self, binding: SessionBinding) -> Result<(), PlayError> {
        self.validate_session_sample_rate(binding.requested_sample_rate().get())?;
        self.session
            .set(binding)
            .map_err(|_| PlayError::SessionAlreadyBound)
    }

    pub(crate) fn begin_slot_seek(&self, slot: SlotId, position: Duration) {
        let deck = self.slot.lock();
        if let Some(handle) = deck.get(slot) {
            handle.begin_seek(position);
        }
        drop(deck);
    }

    pub(crate) fn cancel(&self) {
        if let Some(cancel) = &self.config.cancel {
            cancel.cancel();
        }
    }

    pub(crate) fn cancel_token(&self) -> Option<CancelToken> {
        self.config.cancel.clone()
    }

    pub(crate) const fn configured_sample_rate(&self) -> u32 {
        self.config.sample_rate.get()
    }

    /// How the audio consumers this deck hosts may wake workers. A player
    /// may prepare resources before a Host takes it, so until then it takes
    /// the RT-safe production path.
    pub(crate) fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        self.session.get().map_or(
            ConsumerWakeMode::RealtimeDeferred,
            SessionBinding::consumer_wake_mode,
        )
    }

    pub(crate) fn drain_slot_trash(&self, slot: SlotId) -> bool {
        self.slot.lock().get_mut(slot).is_some_and(|handle| {
            Self::drain_slot_trash_handle(handle);
            true
        })
    }

    fn drain_slot_trash_handle(handle: &mut SlotHandle) {
        while let Some(trash) = handle.trash_rx.try_pop() {
            let DeckTrash::Track(track) = trash else {
                continue;
            };
            if let Some(seek) = track.seek_handle() {
                handle.unbind_seek(track.item_id(), &seek);
            }
            if let Some(render) = track.render_reader() {
                handle.unbind_render(track.item_id(), &render);
            }
        }
    }

    fn emit(&self, event: EngineEvent) {
        self.bus.publish(event);
    }

    pub(crate) fn eq_band_count(&self) -> usize {
        self.config.eq_layout.lock().len()
    }

    /// The gain, in dB, `band` of the deck's EQ layout is set to.
    pub(crate) fn eq_gain(&self, band: usize) -> Option<f32> {
        self.config
            .eq_layout
            .lock()
            .get(band)
            .map(|band| f32::from(band.gain_db()))
    }

    /// The part that hands a slot the deck's EQ layout, built off the audio thread.
    pub(crate) fn eq_part(&self) -> Result<DeckPart, PlayError>
    where
        S: HasPool<f32>,
    {
        self.eq_layout_part(&self.config.eq_layout.lock())
    }

    fn eq_layout_part(&self, bands: &[EqBandConfig]) -> Result<DeckPart, PlayError>
    where
        S: HasPool<f32>,
    {
        let config = EqConfig::builder(self.pools().clone()).build();
        let layout = EqLayout::new(&config, bands, self.config.sample_rate)?;
        Ok(DeckPart::Eq(DeckEqChange::Layout(Box::new(layout))))
    }

    /// Effective sample rate of the audio host (from Firewheel / `CoreAudio`).
    ///
    /// Returns the config default until a Host takes the deck.
    /// Used to pre-initialise the resampler in `ResourceConfig` so that
    /// `make_sincs` runs while the resource is prepared (off the worker thread)
    /// instead of lazily on the first `step_track()` call.
    pub fn master_sample_rate(&self) -> u32 {
        if self.slot().is_none() {
            return self.config.sample_rate.get();
        }
        self.session.get().map_or_else(
            || self.config.sample_rate.get(),
            |session| session.sample_rate().output(),
        )
    }

    pub(crate) const fn pools(&self) -> &PoolRegion<S> {
        &self.config.pools
    }

    /// What the deck joins its session with.
    pub(crate) fn registration(&self) -> DeckRegistration<S> {
        DeckRegistration {
            grid_id: self.config.grid_id,
            bus: self.bus.clone(),
            mixer: self.config.mixer,
            pools: self.pools().clone(),
            render_quantum_frames: self.config.render_quantum_frames,
            response_budget_frames: self.config.response_budget_frames,
        }
    }

    /// The Host built the deck's slot: the deck plays through it from now on.
    pub(crate) fn seat(&self, started: AllocatedSlot) {
        self.slot.lock().set(started.slot, started.control);
        info!(
            sample_rate = self.config.sample_rate.get(),
            channels = self.config.channels,
            slot = ?started.slot,
            "engine started"
        );
        self.emit(EngineEvent::Started);
    }

    pub(crate) fn pop_slot_notification(&self, slot: SlotId) -> Option<PlayerNotification> {
        self.slot
            .lock()
            .get_mut(slot)
            .and_then(|handle| handle.notif_rx.try_pop())
    }

    /// Sends `commands` to the slot's deck as one batch, applied together in its next block.
    pub(crate) fn send_slot_cmd(
        &self,
        slot: SlotId,
        commands: Vec<DeckPart>,
    ) -> Result<(), PlayError> {
        self.with_deck(slot, |deck| {
            deck.send_batch(commands)
                .map(drop)
                .map_err(|_| PlayError::SlotChannelFull { slot })
        })
    }

    /// Runs `send` on the control half of the deck's slot under its lock.
    pub(crate) fn with_deck<R>(
        &self,
        slot: SlotId,
        send: impl FnOnce(&mut SlotControl) -> Result<R, PlayError>,
    ) -> Result<R, PlayError> {
        let mut deck = self.slot.lock();
        let result = deck
            .get_mut(slot)
            .map_or(Err(PlayError::SlotNotFound(slot)), send);
        drop(deck);
        result
    }

    /// Sends the deck a gain for `band` and keeps it; an idle player keeps it for its next slot.
    ///
    /// # Errors
    /// Returns [`PlayError::EqBandOutOfRange`] for a band the layout does not have, and the
    /// deck's refusal of the gain; the gain stays as it was then.
    pub(crate) fn set_eq_gain(
        &self,
        band: usize,
        gain: GainDb,
        send: impl FnOnce(DeckPart) -> Result<(), PlayError>,
    ) -> Result<(), PlayError> {
        let mut layout = self.config.eq_layout.lock();
        let bands = layout.len();
        let Some(config) = layout.get_mut(band) else {
            return Err(PlayError::EqBandOutOfRange { band, bands });
        };
        match send(DeckPart::Eq(DeckEqChange::Gain { band, gain })) {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => return Err(error),
        }
        config.set_gain_db(gain);
        drop(layout);
        Ok(())
    }

    /// Sends the deck `bands`, built here, and keeps them; an idle player keeps them for its
    /// next slot.
    ///
    /// # Errors
    /// Returns the pool's refusal to build the layout and the deck's refusal of it; the layout
    /// stays as it was then.
    pub(crate) fn set_eq_layout(
        &self,
        bands: Vec<EqBandConfig>,
        send: impl FnOnce(DeckPart) -> Result<(), PlayError>,
    ) -> Result<(), PlayError>
    where
        S: HasPool<f32>,
    {
        let mut layout = self.config.eq_layout.lock();
        match send(self.eq_layout_part(&bands)?) {
            Ok(()) | Err(PlayError::NoActiveSlot) => {}
            Err(error) => return Err(error),
        }
        *layout = bands;
        drop(layout);
        Ok(())
    }

    pub(crate) fn stream_shape(&self) -> Option<StreamShape> {
        self.session.get().and_then(SessionBinding::stream_shape)
    }

    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }

    /// The platform suspended this session's audio output at `tick`.
    pub fn suspend_output(&self, tick: u64) {
        self.suspended_at
            .store(tick.saturating_add(1), Ordering::Release);
    }

    /// The audio-thread tick this session's output was suspended at, while the
    /// platform still holds it.
    ///
    /// A suspended output leaves the RT processor unscheduled, so every value
    /// it publishes stays at whatever it last wrote. The tick is how a reader
    /// tells the two apart: while the audio thread still stands where it
    /// stood, its publications describe an output that is gone. One tick past
    /// it the processor has drained the commands sent before the suspension,
    /// but it counts a call as the call starts and publishes as it ends, so a
    /// reader may look in between. Two ticks past it that call has published,
    /// so the output speaks for itself again and nothing needs to release it.
    pub fn suspended_at(&self) -> Option<u64> {
        match self.suspended_at.load(Ordering::Acquire) {
            0 => None,
            tick => Some(tick - 1),
        }
    }

    /// The player closed: the deck its Host built plays nothing more.
    pub(crate) fn close(&self) {
        if self.slot().is_some() {
            self.emit(EngineEvent::Stopped);
        }
    }

    /// The deck's slot, once its Host has built it.
    pub fn slot(&self) -> Option<SlotId> {
        self.slot.lock().id()
    }

    fn validate_session_sample_rate(&self, session: u32) -> Result<(), PlayError> {
        let player = self.configured_sample_rate();
        if player == session {
            Ok(())
        } else {
            Err(PlayError::SessionSampleRateMismatch { player, session })
        }
    }

    delegate::delegate! {
        to self.slot.lock() {
            #[call(playback)]
            pub(crate) fn slot_playback(&self, slot: SlotId) -> Option<Arc<PlaybackShared>>;
            #[call(render_snapshot)]
            pub(crate) fn slot_render_snapshot(&self, slot: SlotId) -> Option<RenderSnapshot>;
        }
    }
}

#[cfg(test)]
mod config_tests {
    use std::num::{NonZeroU32, NonZeroUsize};

    use kithara_config::Config as _;
    use kithara_test_utils::kithara;
    use kithara_warp::BeatGridId;

    use super::*;
    use crate::test_pools::{TestPools, pools};

    #[kithara::test]
    fn engine_config_remains_the_live_eq_layout_owner() {
        let config: EngineConfig<TestPools> = EngineConfig::builder()
            .grid_id(BeatGridId::allocate().expect("a grid identity"))
            .pools(pools())
            .sample_rate(NonZeroU32::new(48_000).expect("48000 is not zero"))
            .response_budget_frames(NonZeroUsize::new(448).expect("448 is not zero"))
            .build();
        let engine = EngineImpl::new(config, EventBus::new(32));

        assert_eq!(engine.config.values().sample_rate.get(), 48_000);
        assert_eq!(engine.config.values().eq_layout.len(), 10);
        engine
            .set_eq_layout(kithara_effects::eq::generate_log_spaced_bands(4), |_| {
                Err(PlayError::NoActiveSlot)
            })
            .expect("an idle engine keeps its next layout");
        assert_eq!(engine.config.values().eq_layout.len(), 4);
    }
}
