use std::num::NonZeroU32;

use delegate::delegate;
use kithara_bufpool::{HasPool, PoolRegion};
use kithara_platform::{
    sync::{Arc, ExclusiveGate, Mutex},
    time::Duration,
};
use kithara_render::{bridge::DeckPart, rt::track::PlayerResource};
use tracing::debug;

use super::{
    PlayerConfig,
    lifecycle::{CloseAdmission, PlayerLifecycle},
    state::{CurrentItem, ItemPresentation, PlayerPhase, Tracks},
    track::Behind,
};
use crate::{
    EngineLoad,
    api::{PlayerEvent, PlayerStatus, TrackId},
    engine::EngineImpl,
    error::PlayError,
    resource::Resource,
    session::SessionBinding,
};

/// An item handed to the processor, with the presentation it publishes once
/// it becomes the current item.
pub(crate) struct EnqueuedItem {
    pub(crate) item_id: TrackId,
    pub(crate) duration_seconds: f64,
    pub(crate) presentation: ItemPresentation,
}

/// Phase-neutral state shared across every player phase.
///
/// Field order is drop order: `engine` releases every registered track before
/// this Player releases its [`PlayWorker`] clone.
pub(crate) struct PlayerCore<S> {
    /// Live shared cost meter of the audio engine (decode + effects).
    /// Constructed once and kept address-stable for the player's lifetime.
    pub(crate) engine_load: Arc<EngineLoad>,

    /// Host lifecycle explicitly detaches the engine session lane before the
    /// worker owner drops.
    pub(crate) engine: EngineImpl<S>,
    /// The item the deck leads, as last announced.
    pub(crate) current: CurrentItem,
    /// Status kept explicit (not derived from phase): `set_status` emits
    /// `StatusChanged` only on change and its values are not 1:1 with phase.
    pub(crate) status: Mutex<PlayerStatus>,
    /// Construction recipe and injected resources. Its worker drops after
    /// the engine and undelivered items.
    pub(crate) config: PlayerConfig<S>,
    /// Where the current item must start when it reaches a processor.
    /// Set by a seek that arrives before the player holds a slot, consumed
    /// by the load that starts playback.
    pub(crate) start_position: Mutex<Option<Duration>>,
    /// The tracks the deck holds.
    pub(crate) tracks: Mutex<Tracks>,
}

/// Concrete Player implementation: one deck and the tracks it holds.
///
/// Owns an [`EngineImpl`] and sends commands to the active slot's processor.
/// A selection hands the deck the resource of the item it makes current,
/// wrapped in [`PlayerResource`], and sends it to the processor via
/// `DeckPart::Attach`; the item list lives with the caller.
///
/// Internally the player is a phase-split typestate: `phase` is a typed
/// `Mutex<PlayerPhase>` carrying the slot / ABR handle / armed-next, while
/// `core` holds the phase-neutral fields. `phase` is declared first so it
/// drops before `core.engine`.
#[doc(hidden)]
#[derive(kithara_config::ConfigOwner)]
#[config_owner(PlayerConfig<S>, core.config)]
pub struct PlayerRuntime<S> {
    pub(crate) phase: Mutex<PlayerPhase>,
    pub(crate) core: PlayerCore<S>,
    /// Admits one operation at a time. An operation waits on the session while
    /// admitted, so a contender parks on the gate instead of blocking a lock.
    pub(super) operations: ExclusiveGate,
    pub(super) lifecycle: PlayerLifecycle,
}

impl<S> PlayerRuntime<S> {
    pub(super) fn attach_session(&self, binding: SessionBinding) -> Result<(), PlayError> {
        self.with_open_result(|runtime| runtime.core.engine.attach_session(binding))
    }

    /// Silences the deck, then closes. A deck that refuses the clear leaves
    /// the player open, so a later close can still silence it.
    pub(super) fn close(&self) -> Result<(), PlayError> {
        let _admission = self.operations.lock();
        if !self.is_closed() {
            self.clear_deck()?;
        }
        match self.begin_close()? {
            CloseAdmission::AlreadyClosed => return Ok(()),
            CloseAdmission::Begin => {}
        }
        self.finish_close();
        self.core.engine.close();
        Ok(())
    }

    /// Hand `resource` to the processor as `item_id`, chained behind the
    /// `behind` track when one is named, so the deck plays it on the frame
    /// after that track's last. Its presentation is returned, not published:
    /// the item may be attached ahead of the one playing.
    ///
    /// # Errors
    /// The deck admits the item and its chain together or not at all. On any
    /// error the resource is spent: the item must be loaded again.
    pub(crate) fn enqueue_to_processor(
        &self,
        item_id: TrackId,
        mut resource: Resource,
        behind: Option<TrackId>,
    ) -> Result<EnqueuedItem, PlayError>
    where
        S: HasPool<f32>,
    {
        let duration_seconds = resource
            .duration()
            .map_or(0.0, |duration| duration.as_secs_f64());
        let presentation = ItemPresentation {
            abr_handle: resource.abr_handle(),
        };
        let lane = resource.take_lane();
        if let Some(sample_rate) = NonZeroU32::new(self.core.engine.master_sample_rate()) {
            resource.set_host_sample_rate(sample_rate);
        }
        resource.set_consumer_wake_mode(self.core.engine.consumer_wake_mode());
        let src = Arc::clone(resource.src());
        let player_resource = PlayerResource::new(resource.into(), src, self.core.engine.pools())?;
        let behind = match behind {
            Some(track) => {
                let playback = self.slot_playback().ok_or(PlayError::NoActiveSlot)?;
                Some(Behind {
                    track,
                    epoch: playback.issue_epoch(),
                })
            }
            None => None,
        };
        self.with_tracks(|tracks, out| {
            tracks.load(item_id, Box::new(player_resource), lane, behind, out)
        })?;
        Ok(EnqueuedItem {
            item_id,
            duration_seconds,
            presentation,
        })
    }

    /// The item that became current publishes its ABR handle.
    pub(crate) fn adopt_presentation(&self, presentation: ItemPresentation) {
        self.phase.lock().set_abr_handle(presentation.abr_handle);
    }

    /// Terminal teardown: close the player and cancel its subtree.
    ///
    /// Deliberately skips the admission gate. This runs from `Drop`, and a
    /// player is dropped by whoever last owns it — including a session
    /// dispatcher unwinding its own state. An admitted operation can be parked
    /// on a reply from that same dispatcher, so waiting for the gate here
    /// closes the cycle. Neither step needs it: closing is a store on an
    /// atomic and cancelling fires a token, and a cancel exists to interrupt
    /// an admitted operation rather than to queue behind one. `close` still
    /// takes the gate, so the orderly path keeps its ordering.
    pub(super) fn invalidate(&self) {
        self.finish_close();
        self.core.engine.cancel();
    }

    /// Drop every track the deck holds.
    ///
    /// Also clears any held start position, since the item it targeted no
    /// longer exists once the deck is empty.
    ///
    /// # Errors
    /// Returns the deck's refusal of the clear; the player keeps its tracks
    /// then.
    pub fn remove_all_items(&self) -> Result<(), PlayError>
    where
        S: HasPool<f32>,
    {
        self.clear_deck()?;
        self.unarm_next();
        self.core.current.clear();
        self.set_status(PlayerStatus::Unknown);
        *self.core.start_position.lock() = None;
        self.enter_stopped();
        self.core
            .engine
            .bus()
            .publish(PlayerEvent::RateChanged { rate: 0.0 });
        debug!("all items removed");
        Ok(())
    }

    /// Clears the deck from its next block; a player without a slot has no
    /// deck to clear.
    fn clear_deck(&self) -> Result<(), PlayError> {
        match self.send_to_slot(DeckPart::Clear) {
            Ok(()) | Err(PlayError::NoActiveSlot) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Rate the player's master bus runs at. Decoded frames handed to an
    /// observer use this axis after decoder-side conversion.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.core.engine.master_sample_rate()
    }

    /// Internal: set status and emit event if changed.
    pub(crate) fn set_status(&self, new_status: PlayerStatus) {
        let mut status = self.core.status.lock();
        if *status != new_status {
            *status = new_status;
            drop(status);
            self.core
                .engine
                .bus()
                .publish(PlayerEvent::StatusChanged { status: new_status });
        }
    }

    pub(super) fn with_open<T>(&self, operation: impl FnOnce(&Self) -> T) -> Result<T, PlayError> {
        let _admission = self.operations.lock();
        if self.is_closed() {
            return Err(PlayError::Closed);
        }
        Ok(operation(self))
    }

    pub(super) fn with_open_result<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, PlayError>,
    ) -> Result<T, PlayError> {
        self.with_open(operation)?
    }

    delegate! {
        to self.lifecycle {
            fn begin_close(&self) -> Result<CloseAdmission, PlayError>;
            fn finish_close(&self);
            pub(crate) fn is_closed(&self) -> bool;
        }
        to self.core.config.worker {
            /// Typed pool facade used for resources created by this player.
            #[must_use]
            pub fn pools(&self) -> &PoolRegion<S>;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_assets::AssetStore;
    use kithara_config::Config as _;
    use kithara_decode::GaplessMode;
    use kithara_platform::{CancelToken, time::Duration};
    #[cfg(not(target_arch = "wasm32"))]
    use kithara_platform::{
        sync::mpsc::{self, TryRecvError},
        thread,
    };
    use kithara_render::bridge::DeckPart;
    use kithara_test_utils::kithara;
    use kithara_warp::MIN_SPEED;

    use super::{super::PlayerImpl, *};
    use crate::{
        PlayWorker, PlayWorkerConfig, mock,
        player::{PlayerConfig, PlayerConfigPatch, PlayerControlSource},
        resource::{ResourceConfig, ResourceSrc},
        test_pools::{TestPools, pools},
    };

    fn resource_config(input: &str) -> ResourceConfig<TestPools> {
        let pools = pools();
        let src = ResourceSrc::parse(input).expect("BUG: valid resource config source");
        ResourceConfig::for_src(src)
            .store(AssetStore::builder(pools).build())
            .build()
    }

    fn worker() -> PlayWorker<TestPools> {
        PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
    }

    fn player() -> PlayerImpl<TestPools> {
        PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        )
    }

    /// A player its Host has seated on a deck slot, with the mock that
    /// answers as the slot's audio thread.
    fn seated() -> (PlayerImpl<TestPools>, Arc<mock::SessionMock>) {
        let mut player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .build(),
        );
        let audio_thread = mock::insert(&mut player);
        (player, audio_thread)
    }

    #[kithara::test(native)]
    fn player_config_values_follow_live_controls() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .gapless_mode(GaplessMode::Disabled)
                .crossfade_duration(2.0)
                .build(),
        );

        let values = player.core.config.values();
        assert_eq!(values.gapless_mode, GaplessMode::Disabled);
        assert_eq!(values.crossfade_duration, 2.0);
        assert_eq!(player.crossfade_duration(), 2.0);

        player.set_crossfade_duration(3.0);
        assert_eq!(player.crossfade_duration(), 3.0);
        assert_eq!(player.core.config.values().crossfade_duration, 3.0);

        player
            .set_default_rate(0.75)
            .expect("a finite rate is accepted");
        player.set_volume(0.4).expect("the player takes the volume");
        player.set_muted(true).expect("the player takes the mute");
        let values = player.core.config.values();
        assert_eq!(values.default_rate, 0.75);
        assert_eq!(values.volume, 0.4);
        assert!(values.muted);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(timeout(Duration::from_secs(5)))]
    fn close_waits_for_an_admitted_operation() {
        let player = player();
        let runtime = Arc::clone(&player.runtime);
        let closing = Arc::clone(&player.runtime);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let operation = thread::spawn(move || {
            runtime
                .with_open(|_| {
                    entered_tx.send(()).expect("report admitted operation");
                    release_rx.recv().expect("release admitted operation");
                })
                .expect("operation remains admitted");
        });
        entered_rx
            .recv()
            .expect("operation entered the admission gate");

        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (closing_tx, closing_rx) = mpsc::channel();
        let closer = thread::spawn(move || {
            attempting_tx.send(()).expect("report close attempt");
            closing_tx
                .send(closing.close())
                .expect("report close result");
        });
        attempting_rx
            .recv()
            .expect("close reached the admission gate");
        kithara_test_utils::test::wall_sleep(Duration::from_millis(50));
        assert!(matches!(closing_rx.try_recv(), Err(TryRecvError::Empty)));

        release_tx.send(()).expect("release admitted operation");
        operation.join().expect("operation thread completed");
        closer.join().expect("close thread completed");
        closing_rx
            .recv()
            .expect("close result returned after the operation")
            .expect("close succeeds");
        assert!(player.runtime.is_closed());
        assert!(matches!(player.reset_eq(), Err(PlayError::Closed)));
        assert!(matches!(
            player.runtime.with_open(|_| ()),
            Err(PlayError::Closed)
        ));
    }

    /// The claim rests on the ordering, not on the wait: the operation
    /// is released only after the drop has been observed, so a drop that
    /// queued behind it could never be observed at all. The test timeout
    /// turns that deadlock into a named failure.
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(timeout(Duration::from_secs(5)))]
    fn drop_does_not_wait_for_an_admitted_operation() {
        let player = player();
        let runtime = Arc::clone(&player.runtime);
        let observer = player
            .prepare_config(resource_config("https://example.com/song.mp3"))
            .expect("test session answers stream-shape queries")
            .cancel
            .expect("prepare_config must populate cancel")
            .child();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let operation = thread::spawn(move || {
            runtime
                .with_open(|_| {
                    entered_tx.send(()).expect("report admitted operation");
                    release_rx.recv().expect("release admitted operation");
                })
                .expect("operation remains admitted");
        });
        entered_rx
            .recv()
            .expect("operation entered the admission gate");

        let closed = Arc::clone(&player.runtime);
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(player);
            dropped_tx.send(()).expect("report completed drop");
        });
        dropped_rx
            .recv()
            .expect("drop must not queue behind an admitted operation");
        assert!(closed.is_closed(), "drop must close the player at once");
        assert!(
            observer.is_cancelled(),
            "drop must cancel the subtree while the operation is still admitted"
        );

        release_tx.send(()).expect("release admitted operation");
        operation.join().expect("operation thread completed");
        dropper.join().expect("drop thread completed");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test]
    fn player_lifecycle_admits_only_one_concurrent_close() {
        let lifecycle = Arc::new(PlayerLifecycle::open());
        assert!(
            matches!(lifecycle.begin_close(), Ok(CloseAdmission::Begin)),
            "first closer owns the transition"
        );

        let concurrent = Arc::clone(&lifecycle);
        let result = thread::spawn(move || concurrent.begin_close())
            .join()
            .expect("BUG: lifecycle probe thread panicked");
        assert!(matches!(result, Err(PlayError::Closed)));

        lifecycle.finish_close();
        assert!(matches!(
            lifecycle.begin_close(),
            Ok(CloseAdmission::AlreadyClosed)
        ));
    }

    #[kithara::test]
    fn prepare_config_applies_player_gapless_mode() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .gapless_mode(GaplessMode::Disabled)
                .build(),
        );
        let mut config = resource_config("https://example.com/song.mp3");

        config = player
            .prepare_config(config)
            .expect("test session answers stream-shape queries");

        assert_eq!(config.decoder.gapless_mode(), GaplessMode::Disabled);
        assert!(
            config.cancel.is_some(),
            "prepare_config must inject a per-track cancel child"
        );
    }

    /// A document naming `player.gapless_mode` must reach the same prepared
    /// decoder config as the Rust-level builder above -- riding the full
    /// chain (`PlayerConfig` -> `PlayerCore` -> `AudioDecoderConfig`) rather
    /// than stopping at the configuration the patch writes into.
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test]
    fn a_document_named_gapless_mode_reaches_the_prepared_decoder() {
        let patch: PlayerConfigPatch = serde_yaml_ng::from_str("gapless_mode:\n  mode: disabled\n")
            .expect("the document types");
        let mut config = PlayerConfig::builder()
            .worker(worker())
            .session(mock::session())
            .sample_rate(NonZeroU32::new(44_100).expect("44100 is not zero"))
            .build();
        config.apply(patch).expect("valid player document patch");

        let player = PlayerImpl::new(config);
        let mut config = resource_config("https://example.com/song.mp3");

        config = player
            .prepare_config(config)
            .expect("test session answers stream-shape queries");

        assert_eq!(config.decoder.gapless_mode(), GaplessMode::Disabled);
    }

    #[kithara::test]
    fn prepare_config_per_track_cancel_is_child_of_player_master() {
        let player = player();
        let mut rc = resource_config("https://example.com/song.mp3");
        rc = player
            .prepare_config(rc)
            .expect("test session answers stream-shape queries");

        let track_cancel = rc.cancel.expect("prepare_config must populate cancel");
        let observer = track_cancel.child();
        assert!(!observer.is_cancelled());

        drop(player);
        assert!(
            observer.is_cancelled(),
            "dropping the player must cancel the per-track child via the master"
        );
    }

    #[kithara::test]
    fn prepare_config_preserves_caller_supplied_master() {
        let parent_master = CancelToken::never();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .cancel(parent_master.clone())
                .build(),
        );
        let mut rc = resource_config("https://example.com/song.mp3");
        rc = player
            .prepare_config(rc)
            .expect("test session answers stream-shape queries");

        let track_cancel = rc.cancel.expect("prepare_config must populate cancel");
        let observer = track_cancel.child();
        assert!(!observer.is_cancelled());

        parent_master.cancel();
        assert!(observer.is_cancelled());
    }

    #[kithara::test]
    fn player_default_rate_getter_setter() {
        let player = player();
        assert!((player.default_rate() - 1.0).abs() < f32::EPSILON);
        player
            .set_default_rate(0.75)
            .expect("a finite rate is accepted");
        assert!((player.default_rate() - 0.75).abs() < f32::EPSILON);
        assert!((player.core.tracks.lock().next().speed() - 0.75).abs() < f32::EPSILON);
        assert_eq!(player.rate(), 0.0);
    }

    #[kithara::test]
    fn set_rate_without_active_slot_updates_only_the_requested_target() {
        let player = player();
        player.set_rate(2.0).expect("a finite rate is accepted");
        assert!((player.rate() - 0.0).abs() < f32::EPSILON);
        assert!((player.core.tracks.lock().next().speed() - 2.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    #[case(0.0)]
    #[case(-1.0)]
    fn a_rate_under_the_floor_requests_the_slowest_speed(#[case] rate: f32) {
        let player = player();
        player.set_rate(rate).expect("a finite rate is accepted");
        assert!((player.core.tracks.lock().next().speed() - MIN_SPEED).abs() < f32::EPSILON);
    }

    #[kithara::test]
    #[case(f32::NAN)]
    #[case(f32::INFINITY)]
    fn a_rate_that_is_not_a_finite_number_is_refused(#[case] rate: f32) {
        let player = player();
        assert!(matches!(
            player.set_rate(rate),
            Err(PlayError::InvalidParameter { .. })
        ));
        assert!(matches!(
            player.set_default_rate(rate),
            Err(PlayError::InvalidParameter { .. })
        ));
        assert!((player.default_rate() - 1.0).abs() < f32::EPSILON);
        assert!((player.core.tracks.lock().next().speed() - 1.0).abs() < f32::EPSILON);
    }

    /// A rate the deck has no room for is refused whole: neither the target
    /// nor the default moves, so the caller sends it again once the deck
    /// has room.
    #[kithara::test]
    fn a_rate_the_deck_has_no_room_for_changes_nothing() {
        let (player, _audio_thread) = seated();
        player.play();
        while player.send_to_slot(DeckPart::StopAll).is_ok() {}

        let refused = player.set_rate(2.0);
        assert!(
            matches!(refused, Err(PlayError::SlotChannelFull { .. })),
            "a full deck refuses the rate, not {refused:?}"
        );
        let refused = player.set_default_rate(2.0);
        assert!(
            matches!(refused, Err(PlayError::SlotChannelFull { .. })),
            "a full deck refuses the default rate, not {refused:?}"
        );
        assert!((player.default_rate() - 1.0).abs() < f32::EPSILON);
        assert!((player.core.tracks.lock().next().speed() - 1.0).abs() < f32::EPSILON);
    }

    /// Wakes the test once per post its player's mailbox takes.
    #[cfg(not(target_arch = "wasm32"))]
    struct PostWake(mpsc::Sender<()>);

    #[cfg(not(target_arch = "wasm32"))]
    impl std::task::Wake for PostWake {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }

    /// A command posted through a control runs where the player's holder
    /// drains it: the post wakes the holder, and the player is untouched
    /// until that drain.
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(timeout(Duration::from_secs(5)))]
    fn a_posted_command_waits_for_its_holder_to_drain_it() {
        use crate::player::Player as _;

        let (mut player, _audio_thread) = seated();
        let before = player.volume();
        let (woke_tx, woke_rx) = mpsc::channel();
        player.hold(std::task::Waker::from(Arc::new(PostWake(woke_tx))));
        let control = player.control();
        let poster = thread::spawn(move || control.set_volume(0.25));

        woke_rx.recv().expect("the post wakes the holder");
        assert!(
            (player.volume() - before).abs() < f32::EPSILON,
            "a posted volume reached the player before its holder drained it"
        );
        player.drain();
        poster
            .join()
            .expect("the poster thread ends")
            .expect("the drained player takes the volume");
        assert!((player.volume() - 0.25).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn remove_all_items_releases_the_slot_and_allows_fresh_playback() {
        let (player, _audio_thread) = seated();
        player.play();
        assert!(player.slot().is_some(), "setup must take the deck's slot");

        player.remove_all_items().expect("the deck takes the clear");

        assert!(
            player.slot().is_none(),
            "remove_all_items must release slot ownership"
        );
        assert!(player.current_abr_handle().is_none());
        assert!((player.rate() - 0.0).abs() < f32::EPSILON);

        player.play();

        assert!(player.slot().is_some(), "play must take the slot again");
        player.remove_all_items().expect("the deck takes the clear");
    }

    #[kithara::test]
    fn pause_from_idle_is_noop() {
        use super::super::state::phase::PlayerPhaseKind;

        let player = player();
        assert_eq!(player.phase_kind(), PlayerPhaseKind::Idle);
        player.pause();
        assert_eq!(
            player.phase_kind(),
            PlayerPhaseKind::Idle,
            "pause from Idle must not leak a phase transition"
        );
        assert!((player.rate() - 0.0).abs() < f32::EPSILON);
    }

    #[kithara::test]
    fn a_session_at_another_rate_refuses_the_player() {
        let mut player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .build(),
        );

        assert!(matches!(
            player.attach_session(mock::session_at(
                NonZeroU32::new(48_000).expect("48000 is not zero"),
            )),
            Err(PlayError::SessionSampleRateMismatch {
                player: 44_100,
                session: 48_000,
            })
        ));
    }

    #[kithara::test]
    fn send_to_slot_without_a_slot_is_an_error() {
        let player = player();
        assert!(player.send_to_slot(DeckPart::StopAll).is_err());
    }
}
