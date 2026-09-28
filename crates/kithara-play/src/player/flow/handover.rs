use std::ops::Deref;

use kithara_bufpool::HasPool;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_sync::{LoadGeneration, SourceChange};

#[cfg(test)]
use super::super::PlayerImpl;
use super::super::{
    core::PlayerRuntime,
    state::{PendingLoads, PendingNext, PendingNextState},
};
use crate::{
    api::{EngineEvent, SlotId, TrackId},
    bridge::{PlayerCmd, PlayerNotification, TrackPlaybackStopReason},
    error::PlayError,
};

/// Outcome of resolving an arm request under the short phase lock, acted on
/// outside the lock to avoid holding it across `send_to_slot`.
enum ArmDecision {
    /// The same index is already armed; return its src verbatim.
    AlreadyArmed(Arc<str>),
    /// The slot was cleared; optionally unload the previous pending track.
    Clear(Option<PendingNext>),
}

struct ActivatedPending {
    item_id: TrackId,
    load: LoadGeneration,
    duration_seconds: f64,
}

struct Handover<'a, S> {
    player: &'a PlayerRuntime<S>,
}

impl<'a, S> Handover<'a, S> {
    const fn new(player: &'a PlayerRuntime<S>) -> Self {
        Self { player }
    }
}

impl<S> Deref for Handover<'_, S> {
    type Target = PlayerRuntime<S>;

    fn deref(&self) -> &Self::Target {
        self.player
    }
}

impl<S> Handover<'_, S>
where
    S: HasPool<f32>,
{
    /// Mark the armed-next slot at `index` activated under a short lock,
    /// returning its src. `Ok(None)` when it was already activated.
    fn activate_pending(&self, index: usize) -> Result<Option<ActivatedPending>, PlayError> {
        let mut phase = self.phase.lock();
        let pending = phase
            .pending_mut()
            .and_then(|slot| slot.as_mut())
            .ok_or(PlayError::NotReady)?;
        if pending.index != index {
            return Err(PlayError::ArmIndexMismatch {
                requested: index,
                armed: pending.index,
            });
        }
        let outcome = if pending.state.activated() {
            None
        } else {
            let item_id = pending.item_id;
            let load = pending.load;
            let duration_seconds = pending.duration_seconds;
            pending.state = PendingNextState::ActivatedReady;
            Some(ActivatedPending {
                item_id,
                load,
                duration_seconds,
            })
        };
        drop(phase);
        Ok(outcome)
    }

    /// Load `items[index]` into the audio-thread arena in `Preloading`
    /// state, ready for sample-accurate gapless stitch (cf=0) or parallel
    /// fade (cf>0).
    ///
    /// If a different next is already armed, it is unloaded first.
    /// Idempotent for the same index. Returns `Some(src)` on success;
    /// `None` if `items[index]` is empty (loader hasn't filled it yet) or
    /// `index` is out of range.
    fn arm_next(&self, index: usize) -> Result<Option<Arc<str>>, PlayError> {
        let current_index = self.current_index();
        if index >= self.item_count() {
            return Ok(None);
        }

        let mut phase = self.phase.lock();
        let decision = match phase.pending() {
            Some(existing) if existing.index == index => {
                ArmDecision::AlreadyArmed(existing.src.clone())
            }
            Some(existing) => {
                let preserve = existing.state.activated() && existing.index == current_index;
                let withdrawn = phase.pending_loads_mut().and_then(PendingLoads::withdraw);
                ArmDecision::Clear(withdrawn.filter(|_| !preserve))
            }
            None => ArmDecision::Clear(None),
        };
        drop(phase);

        let to_unload = match decision {
            ArmDecision::AlreadyArmed(src) => return Ok(Some(src)),
            ArmDecision::Clear(unload) => unload,
        };
        if let Some(pending) = to_unload {
            self.unload_pending(&pending);
        }

        let Some((item_id, load, src, duration_seconds)) =
            self.enqueue_to_processor(index, None)?
        else {
            return Ok(None);
        };
        if let Some(pending_slot) = self.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id,
                load,
                index,
                duration_seconds,
                src: src.clone(),
                state: PendingNextState::Armed,
            });
        }
        Ok(Some(src))
    }

    /// Snapshot of the armed-next index. `None` when no slot is armed
    /// (or after `commit_next` has consumed it for the current handover).
    #[must_use]
    fn armed_next(&self) -> Option<usize> {
        self.phase
            .lock()
            .pending()
            .filter(|pending| !pending.state.activated())
            .map(|pending| pending.index)
    }

    /// Commit the previously armed next track and start the cross-fade.
    ///
    /// Sends `FadeIn` to the audio thread for the armed slot, updates
    /// the playlist current index, and publishes `CurrentItemChanged`.
    ///
    /// # Errors
    /// - [`PlayError::NotReady`] if no slot is armed.
    /// - [`PlayError::ArmIndexMismatch`] if `index` does not match
    ///   [`Self::armed_next`].
    fn commit_next(&self, index: usize) -> Result<(), PlayError> {
        let Some(activated) = self.activate_pending(index)? else {
            return Ok(());
        };

        if let Err(error) = self.start_resident(
            activated.item_id,
            activated.load,
            activated.duration_seconds,
        ) {
            if let Some(pending) = self
                .phase
                .lock()
                .pending_mut()
                .and_then(Option::as_mut)
                .filter(|pending| {
                    pending.item_id == activated.item_id && pending.load == activated.load
                })
            {
                pending.state = PendingNextState::Armed;
            }
            return Err(error);
        }
        self.publish_crossfade_started();
        let current_index = self.current_index();
        if index != current_index {
            self.core.items.set_current(index);
            self.announce_current_item(index);
        }
        Ok(())
    }

    fn publish_crossfade_started(&self) {
        let Some(slot) = self.slot() else {
            return;
        };
        self.core
            .engine
            .bus()
            .publish(EngineEvent::CrossfadeStarted {
                from: slot,
                to: slot,
                duration: Duration::from_secs_f32(self.crossfade_duration().max(0.0)),
            });
    }

    /// Drop the armed next slot without committing.
    ///
    /// Unloads the pending item from the audio thread and clears the
    /// pending slot. Skips the unload if the armed slot has
    /// already been activated for the current index (the activated track
    /// is now the leading one — unloading would silence playback).
    fn unarm_next(&self) {
        self.unarm_next_internal(Some(self.current_index()));
    }

    fn unarm_next_internal(&self, current_index_hint: Option<usize>) {
        let pending = self
            .phase
            .lock()
            .pending_loads_mut()
            .and_then(PendingLoads::withdraw);
        let Some(pending) = pending else {
            return;
        };
        let preserve_active_current = current_index_hint
            .is_some_and(|index| pending.state.activated() && pending.index == index);
        if !preserve_active_current {
            if pending.state.activated() {
                self.core
                    .engine
                    .bus()
                    .publish(EngineEvent::CrossfadeCancelled);
            }
            self.unload_pending(&pending);
        }
    }

    /// Unload a pending track the handover no longer wants. An activated one
    /// is the resident, so its accepted unload is reported as a source change
    /// whatever replaces it. An armed one may already have been stitched in,
    /// so the processor drops it only while it still preloads; the phase
    /// keeps it withdrawn until the processor reports the track it played.
    fn unload_pending(&self, pending: &PendingNext) {
        let item_id = pending.item_id;
        if pending.state.activated() {
            let _ = self.send_source_change(
                PlayerCmd::UnloadTrack { item_id },
                SourceChange::Discontinuity,
            );
        } else if self
            .send_to_slot(PlayerCmd::CancelPreload { item_id })
            .is_err()
            && let Some(loads) = self.phase.lock().pending_loads_mut()
        {
            loads.cancel_refused((item_id, pending.load));
        }
    }
}

impl<S> PlayerRuntime<S>
where
    S: HasPool<f32>,
{
    pub fn arm_next(&self, index: usize) -> Result<Option<Arc<str>>, PlayError> {
        Handover::new(self).arm_next(index)
    }

    #[must_use]
    pub fn armed_next(&self) -> Option<usize> {
        Handover::new(self).armed_next()
    }

    pub fn commit_next(&self, index: usize) -> Result<(), PlayError> {
        Handover::new(self).commit_next(index)
    }

    /// Settle the successor when a track ends: an armed one now leads unless a
    /// withdrawal is still in question, and an activated one is retired.
    pub(crate) fn finalize_handover_if_armed(&self) {
        let pending = self
            .phase
            .lock()
            .pending_loads_mut()
            .and_then(PendingLoads::take_at_end);
        let Some(pending) = pending else {
            return;
        };

        if pending.state.activated() {
            return;
        }

        if pending.index >= self.item_count() {
            return;
        }
        self.adopt_stitched((pending.item_id, pending.load));
        let index = pending.index;
        self.core.items.set_current(index);
        self.announce_current_item(index);
    }

    /// Record a successor the audio thread already stitched in as the
    /// resident. Nothing is left to abort, so the change is reported, not
    /// held.
    fn adopt_stitched(&self, stitched: (TrackId, LoadGeneration)) {
        let edit = self.core.engine.edit_source();
        self.phase.lock().set_resident(stitched);
        match edit {
            Ok(edit) => edit.commit(SourceChange::Discontinuity),
            Err(error) => tracing::warn!(%error, "gapless promotion unreported to sync owner"),
        }
    }

    /// Settle a withdrawal still in question once the processor reports the
    /// track it played, by its start or its natural or failed end: the queue
    /// follows the successor stitched in, and one removed from the queue since
    /// keeps playing unannounced. A withdrawn successor the processor reports
    /// unloaded can no longer be stitched in and leaves the question.
    pub(crate) fn settle_withdrawal(&self, slot_id: SlotId, notification: &PlayerNotification) {
        if self.slot() != Some(slot_id) {
            return;
        }
        let played = match notification {
            PlayerNotification::PlaybackStarted { item_id, .. }
            | PlayerNotification::PlaybackStopped {
                reason: TrackPlaybackStopReason::Eof | TrackPlaybackStopReason::Failed(_),
                item_id,
                ..
            } => *item_id,
            PlayerNotification::Unloaded { item_id, .. } => {
                if let Some(loads) = self.phase.lock().pending_loads_mut() {
                    loads.retire(*item_id);
                }
                return;
            }
            _ => return,
        };
        let settled = self
            .phase
            .lock()
            .pending_loads_mut()
            .and_then(|loads| loads.settle_played(played));
        let Some(load) = settled else {
            return;
        };
        self.adopt_stitched((played, load));
        let Some(index) = self.core.items.index_of(played) else {
            return;
        };
        self.core.items.set_current(index);
        self.announce_current_item(index);
    }

    pub fn unarm_next(&self) {
        Handover::new(self).unarm_next();
    }

    pub(crate) fn unarm_next_internal(&self, current_index_hint: Option<usize>) {
        Handover::new(self).unarm_next_internal(current_index_hint);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroU32,
        sync::atomic::{AtomicU64, Ordering},
    };

    #[derive(Clone, Debug, kithara_events::EventSet)]
    enum TestEvent {
        Engine(EngineEvent),
        Player(PlayerEvent),
    }

    use kithara_audio::{
        SeekBegin, SeekOutcome,
        mock::{AudioControlMock, AudioReadMock, AudioSessionMock},
    };
    use kithara_events::{Envelope, EventBus};
    use kithara_platform::time::Duration;
    use kithara_signal::AudioSpec;
    use kithara_sync::mock::MemberOwner;
    use kithara_test_utils::kithara;
    use kithara_warp::{BeatGrid, BeatGridId};
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig,
        api::{CrossfadeSettings, EngineEvent, PlayerEvent, SelectionPlayback},
        bridge::PlaybackFault,
        mock,
        player::{PlayerConfig, flow::SelectTransition},
        resource::Resource,
        test_pools::{TestPools, pools},
    };

    fn worker() -> PlayWorker<TestPools> {
        PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
    }

    fn resource(src: &str) -> Resource {
        resource_with_seek(src, None)
    }

    fn resource_with_seek(src: &str, seek: Option<Arc<dyn SeekBegin>>) -> Resource {
        let reader = Unimock::new((
            AudioSessionMock::event_bus
                .each_call(matching!())
                .answers(&|mock| mock.make_ref(EventBus::new(1))),
            AudioSessionMock::duration
                .each_call(matching!())
                .returns(Some(Duration::from_secs(1))),
            AudioReadMock::spec
                .each_call(matching!())
                .returns(AudioSpec::new(
                    2,
                    NonZeroU32::new(44_100).expect("fixture rate"),
                )),
            AudioControlMock::preload
                .next_call(matching!())
                .returns(Ok(())),
            AudioControlMock::seek_handle
                .each_call(matching!())
                .returns(seek),
        ));
        Resource::from_reader(reader, Some(Arc::from(src)))
    }

    struct SeekCounter(Arc<AtomicU64>);

    impl SeekBegin for SeekCounter {
        fn begin(&self, position: Duration) -> SeekOutcome {
            self.0.fetch_add(1, Ordering::Relaxed);
            SeekOutcome::Landed {
                target: position,
                landed_at: position,
            }
        }
    }

    #[kithara::test]
    fn preloading_successor_keeps_committed_load_until_handover() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        );
        let first_id = TrackId::allocate();
        let second_id = TrackId::allocate();
        player.insert(resource("first"), first_id, None);
        player.insert(resource("second"), second_id, None);
        player.play();
        let control = player.make_control();
        let first = control
            .resident_sync_observation()
            .expect("player open")
            .expect("first load committed");
        assert_eq!(first.item_id(), first_id);
        assert_eq!(first.load(), LoadGeneration::first());

        player.arm_next(1).expect("preload accepted");
        let still_first = control
            .resident_sync_observation()
            .expect("player open")
            .expect("first load still committed");
        assert_eq!(
            (still_first.item_id(), still_first.load()),
            (first_id, first.load())
        );

        player.commit_next(1).expect("handover accepted");
        let second = control
            .resident_sync_observation()
            .expect("player open")
            .expect("second load committed");
        assert_eq!(second.item_id(), second_id);
        assert_eq!(
            second.load(),
            first.load().checked_next().expect("fixture generation")
        );
    }

    #[kithara::test]
    fn commit_next_without_arm_returns_not_ready() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        );
        let err = player.commit_next(1).expect_err("must error");
        assert!(matches!(err, PlayError::NotReady));
    }

    #[kithara::test]
    fn commit_next_publishes_snapshot_before_current_item_changed() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        );
        player
            .ensure_engine_started()
            .expect("engine start must succeed");
        player.ensure_slot().expect("slot allocation must succeed");
        let mut rx = player.subscribe();

        let item_id = TrackId::allocate();
        let load = LoadGeneration::first();
        if let Some(pending_slot) = player.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id,
                load,
                src: Arc::from("next.mp3"),
                state: PendingNextState::Armed,
                index: 1,
                duration_seconds: 162.0,
            });
        }

        let control = player.make_control();
        assert!(
            control
                .resident_sync_observation()
                .expect("player open")
                .is_none(),
            "preloaded successor is not yet the committed resident"
        );

        player.commit_next(1).expect("commit_next must succeed");

        player.pause();
        let observation = control
            .resident_sync_observation()
            .expect("player open")
            .expect("committed resident");
        assert_eq!((observation.item_id(), observation.load()), (item_id, load));
        assert!(matches!(
            observation.render(),
            kithara_sync::ResidentRender::Missing
        ));
        assert_eq!(
            observation.staging(),
            kithara_sync::ResidentStaging::Unavailable
        );

        assert!(matches!(
            rx.try_recv(),
            Ok(Envelope {
                event: TestEvent::Engine(EngineEvent::CrossfadeStarted { .. }),
                ..
            })
        ));
        assert_eq!(player.duration_seconds(), Some(162.0));
        assert!(matches!(
            rx.try_recv(),
            Ok(Envelope {
                event: TestEvent::Player(PlayerEvent::CurrentItemChanged { .. }),
                ..
            })
        ));
    }

    #[kithara::test]
    fn rejected_load_does_not_advance_generation_or_publish_resident() {
        let (session, mock_session) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session)
                .build(),
        );
        player.ensure_engine_started().expect("engine started");
        player.ensure_slot().expect("slot allocated");
        let before_grid = player.core.track_grid.snapshot().revision();
        player.insert(resource("rejected"), TrackId::allocate(), None);
        for _ in 0..32 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture fills command ring");
        }

        assert!(matches!(
            player.enqueue_to_processor(0, None),
            Err(PlayError::SlotChannelFull { .. })
        ));
        assert_eq!(*player.core.last_load.lock(), None);
        assert!(player.core.items.has_resource(0));
        assert_eq!(player.core.track_grid.snapshot().revision(), before_grid);
        assert!(player.core.staging.stageable_media().is_none());
        assert!(
            player
                .make_control()
                .resident_sync_observation()
                .expect("player open")
                .is_none()
        );

        mock_session.take_commands();
        let retry = player
            .enqueue_to_processor(0, None)
            .expect("capacity returned")
            .expect("resource was retained");
        assert_eq!(retry.1, LoadGeneration::first());
        assert_eq!(*player.core.last_load.lock(), Some(retry.1));
        assert!(!player.core.items.has_resource(0));
    }

    #[kithara::test]
    fn reserved_load_keeps_slot_alive_across_stop_and_close() {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        );
        player.ensure_engine_started().expect("engine started");
        let slot = player.ensure_slot().expect("slot allocated");
        let load = player
            .core
            .engine
            .reserve_slot_load(slot, None)
            .expect("load capacity reserved");

        assert!(matches!(
            player.core.engine.stop(),
            Err(PlayError::SlotBusy { slot: busy }) if busy == slot
        ));
        assert!(matches!(
            player.core.engine.close(),
            Err(PlayError::SlotBusy { slot: busy }) if busy == slot
        ));
        assert!(player.core.engine.is_running());
        assert_eq!(player.core.engine.active_slots(), vec![slot]);

        drop(load);
        player
            .core
            .engine
            .close()
            .expect("close after load release");
        assert!(!player.core.engine.is_running());
        assert!(player.core.engine.active_slots().is_empty());
    }

    #[kithara::test]
    fn resumed_loaded_deck_does_not_reserve_another_load() {
        let (session, mock_session) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session)
                .build(),
        );
        let item_id = TrackId::allocate();
        player.insert(resource("resident"), item_id, None);
        player.play();
        player.pause();
        let before = player
            .make_control()
            .resident_sync_observation()
            .expect("player open")
            .expect("track resident");
        mock_session.take_commands();
        for _ in 0..29 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture leaves three command entries");
        }

        player.play();

        let after = player
            .make_control()
            .resident_sync_observation()
            .expect("player open")
            .expect("same track resident");
        assert_eq!(
            (after.item_id(), after.load()),
            (before.item_id(), before.load())
        );
        assert_eq!(*player.core.last_load.lock(), Some(before.load()));
        let pauses: Vec<bool> = mock_session
            .take_commands()
            .into_iter()
            .filter_map(|command| match command {
                PlayerCmd::SetPaused(paused) => Some(paused),
                _ => None,
            })
            .collect();
        assert!(pauses.ends_with(&[false]));
    }

    #[kithara::test]
    fn rejected_fade_in_does_not_publish_a_resident_or_track_snapshot() {
        let (session, mock_session) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session)
                .build(),
        );
        player.ensure_engine_started().expect("engine started");
        player.ensure_slot().expect("slot allocated");
        let item_id = TrackId::allocate();
        player.insert(resource("not-yet-activated"), item_id, None);
        let before_status = player.status();
        for _ in 0..29 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture leaves room for load and two settings");
        }

        player.play();

        assert_eq!(player.status(), before_status);
        assert_eq!(player.duration_seconds(), None);
        assert_eq!(*player.core.last_load.lock(), None);
        assert!(player.core.items.has_resource(0));
        assert!(
            player
                .make_control()
                .resident_sync_observation()
                .expect("player open")
                .is_none()
        );

        mock_session.take_commands();
        player.play();
        let resident = player
            .make_control()
            .resident_sync_observation()
            .expect("player open")
            .expect("retry committed the load and FadeIn");
        assert_eq!(
            (resident.item_id(), resident.load()),
            (item_id, LoadGeneration::first())
        );
        assert_eq!(player.duration_seconds(), Some(1.0));
    }

    #[kithara::test]
    fn rejected_select_retains_cursor_resident_and_resource_for_retry() {
        let (session, mock_session) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session)
                .build(),
        );
        let first_id = TrackId::allocate();
        let second_id = TrackId::allocate();
        player.insert(resource("first"), first_id, None);
        player.insert(resource("second"), second_id, None);
        player.play();
        let control = player.make_control();
        let first = control
            .resident_sync_observation()
            .expect("player open")
            .expect("first resident");
        mock_session.take_commands();
        for _ in 0..30 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture leaves one entry after select setting");
        }
        let transition = SelectTransition {
            playback: SelectionPlayback::Play,
            crossfade: CrossfadeSettings::default(),
        };

        assert!(matches!(
            player.select_item_with_crossfade(1, transition),
            Err(PlayError::SlotChannelFull { .. })
        ));
        assert_eq!(player.current_index(), 0);
        assert!(player.core.items.has_resource(1));
        let after_rejection = control
            .resident_sync_observation()
            .expect("player open")
            .expect("old resident remains");
        assert_eq!(
            (after_rejection.item_id(), after_rejection.load()),
            (first.item_id(), first.load())
        );

        mock_session.take_commands();
        player
            .select_item_with_crossfade(1, transition)
            .expect("retry commits load and FadeIn");
        assert_eq!(player.current_index(), 1);
        assert!(!player.core.items.has_resource(1));
        let after_retry = control
            .resident_sync_observation()
            .expect("player open")
            .expect("new resident committed");
        assert_eq!(after_retry.item_id(), second_id);
        assert_eq!(
            after_retry.load(),
            first.load().checked_next().expect("fixture generation")
        );
    }

    #[kithara::test]
    fn an_accepted_resident_unload_reports_its_change_when_the_replacement_is_refused() {
        let owner = MemberOwner::new(BeatGridId::allocate().expect("member id"));
        let (session, mock_session) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session.with_sync_gate(owner.gate()))
                .build(),
        );
        for src in ["first", "second", "third"] {
            player.insert(resource(src), TrackId::allocate(), None);
        }
        player.play();
        player.arm_next(1).expect("preload accepted");
        player.commit_next(1).expect("handover accepted");
        let _ = owner.reconcile().expect("owner enters");
        mock_session.take_commands();
        for _ in 0..29 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture leaves room for the setting and the unload");
        }
        let transition = SelectTransition {
            playback: SelectionPlayback::Play,
            crossfade: CrossfadeSettings::default(),
        };

        assert!(matches!(
            player.select_item_with_crossfade(2, transition),
            Err(PlayError::SlotChannelFull { .. })
        ));

        assert_eq!(
            owner.pending_change().expect("owner enters"),
            Some(SourceChange::Discontinuity),
            "the audio thread holds the resident's unload"
        );
        mock_session.take_commands();
        player
            .select_item_with_crossfade(2, transition)
            .expect("the refused replacement kept its resource for a retry");
    }

    #[kithara::test]
    fn full_seek_ring_does_not_begin_reader_seek_or_publish_epoch() {
        let begins = Arc::new(AtomicU64::new(0));
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        );
        player.ensure_engine_started().expect("engine started");
        player.ensure_slot().expect("slot allocated");
        let reader_seek: Arc<dyn SeekBegin> = Arc::new(SeekCounter(Arc::clone(&begins)));
        player.insert(
            resource_with_seek("seekable", Some(reader_seek)),
            TrackId::allocate(),
            None,
        );
        player.enqueue_to_processor(0, None).expect("load accepted");
        for _ in 0..31 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture fills command ring after load");
        }
        let playback = player
            .core
            .engine
            .slot_playback(player.slot().expect("fixture slot"))
            .expect("fixture playback");
        let before_position = playback.position.load(Ordering::Relaxed);

        assert!(matches!(
            player.seek_seconds(0.25),
            Err(PlayError::SlotChannelFull { .. })
        ));
        assert_eq!(begins.load(Ordering::Relaxed), 0);
        assert_eq!(playback.seek_epoch.load(Ordering::SeqCst), 0);
        assert_eq!(playback.position.load(Ordering::Relaxed), before_position);
    }

    fn armed_player() -> PlayerImpl<TestPools> {
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(mock::session())
                .build(),
        );
        player
            .ensure_engine_started()
            .expect("engine start must succeed");
        player.ensure_slot().expect("slot allocation must succeed");
        if let Some(pending_slot) = player.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id: TrackId::allocate(),
                load: LoadGeneration::first(),
                src: Arc::from("next.mp3"),
                state: PendingNextState::Armed,
                index: 1,
                duration_seconds: 162.0,
            });
        }
        player
    }

    /// An audio block already under way when the handover is committed still
    /// renders the outgoing item and publishes its playhead at the end. The
    /// committed item must stay the one the player reports until the audio
    /// thread takes it on.
    #[kithara::test]
    fn committed_item_outlives_a_block_the_outgoing_item_was_rendering() {
        let player = armed_player();

        player.commit_next(1).expect("commit_next must succeed");
        let playback = player
            .slot()
            .and_then(|slot| player.core.engine.slot_playback(slot))
            .expect("the slot must carry playback state");
        playback.position.store(62.3, Ordering::Relaxed);
        playback.duration.store(64.295, Ordering::Relaxed);

        assert_eq!(player.duration_seconds(), Some(162.0));
        assert_eq!(player.position_seconds(), Some(0.0));
    }

    /// A `FadeIn` the full command queue rejects never reaches the audio
    /// thread, so the player keeps reporting the item the audio thread plays
    /// and keeps the successor armed for a retry.
    #[kithara::test]
    fn a_rejected_fade_in_leaves_the_playhead_on_the_playing_item() {
        let player = armed_player();
        let playback = player
            .slot()
            .and_then(|slot| player.core.engine.slot_playback(slot))
            .expect("the slot must carry playback state");
        playback.position.store(62.3, Ordering::Relaxed);
        playback.duration.store(64.295, Ordering::Relaxed);
        while player.send_to_slot(PlayerCmd::SetPaused(false)).is_ok() {}

        assert!(matches!(
            player.commit_next(1),
            Err(PlayError::SlotChannelFull { .. })
        ));

        assert_eq!(player.armed_next(), Some(1));
        assert_eq!(player.duration_seconds(), Some(64.295));
        assert_eq!(player.position_seconds(), Some(62.3));
    }

    /// A resource the command ring never lets load: only its insertion reads
    /// it.
    fn refused_resource(src: &str) -> Resource {
        let reader = Unimock::new((
            AudioSessionMock::event_bus
                .each_call(matching!())
                .answers(&|mock| mock.make_ref(EventBus::new(1))),
            AudioControlMock::preload
                .next_call(matching!())
                .returns(Ok(())),
        ));
        Resource::from_reader(reader, Some(Arc::from(src)))
    }

    /// A deck under a sync owner playing `first` with `second` armed
    /// gaplessly behind it and `third` queued, every command sent so far
    /// taken and every source change reconciled.
    fn deck_with_armed_successor(
        third: Resource,
    ) -> (
        PlayerImpl<TestPools>,
        Arc<mock::SessionMock>,
        MemberOwner,
        [TrackId; 3],
    ) {
        let owner = MemberOwner::new(BeatGridId::allocate().expect("member id"));
        let (session, audio_thread) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session.with_sync_gate(owner.gate()))
                .build(),
        );
        let ids = [
            TrackId::allocate(),
            TrackId::allocate(),
            TrackId::allocate(),
        ];
        player.insert(resource("first"), ids[0], None);
        player.insert(resource("second"), ids[1], None);
        player.insert(third, ids[2], None);
        player.play();
        player.arm_next(1).expect("preload accepted");
        audio_thread.take_commands();
        let _ = owner.reconcile().expect("owner enters");
        (player, audio_thread, owner, ids)
    }

    fn unloaded(commands: &[PlayerCmd]) -> Vec<TrackId> {
        commands
            .iter()
            .filter_map(|command| match command {
                PlayerCmd::UnloadTrack { item_id } => Some(*item_id),
                _ => None,
            })
            .collect()
    }

    fn ended(src: &str, item_id: TrackId) -> PlayerNotification {
        PlayerNotification::PlaybackStopped {
            src: Arc::from(src),
            item_id,
            reason: TrackPlaybackStopReason::Eof,
            seek_epoch: 0,
        }
    }

    fn started(src: &str, item_id: TrackId) -> PlayerNotification {
        PlayerNotification::PlaybackStarted {
            src: Arc::from(src),
            item_id,
        }
    }

    /// Select `third` while the command ring has room only for the
    /// selection's setting and the successor's withdrawal.
    fn select_third_with_its_load_refused(player: &PlayerImpl<TestPools>) {
        for _ in 0..30 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture leaves room for the setting and the withdrawal");
        }
        let _ = player.select_item(2, SelectionPlayback::Play);
    }

    /// The audio thread ends `first` and stitches `second` in, before it
    /// reads anything sent since `second` was armed.
    fn stitch_second_in(audio_thread: &mock::SessionMock, ids: [TrackId; 3]) {
        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("second", ids[1]));
    }

    /// The new selection fades out a successor the audio thread already
    /// stitched in; withdrawing that successor must not cut it off first.
    #[kithara::test]
    fn selecting_another_item_cancels_the_successor_preload_instead_of_unloading_it() {
        let (player, audio_thread, _owner, ids) = deck_with_armed_successor(resource("third"));

        player
            .select_item(2, SelectionPlayback::Play)
            .expect("selection accepted");
        let commands = audio_thread.take_commands();
        assert_eq!(unloaded(&commands), []);
        assert!(commands.iter().any(
            |command| matches!(command, PlayerCmd::CancelPreload { item_id } if *item_id == ids[1])
        ));

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(
            player.current_index(),
            2,
            "the selection leads over the successor it fades out"
        );
    }

    /// A selection whose load the full command ring refuses leaves the
    /// successor stitched in as the track that plays, so the track end must
    /// report it.
    #[kithara::test]
    fn a_refused_selection_keeps_a_successor_already_stitched_in_and_reports_it() {
        let (player, audio_thread, _owner, ids) =
            deck_with_armed_successor(refused_resource("third"));

        select_third_with_its_load_refused(&player);
        let commands = audio_thread.take_commands();
        assert_eq!(unloaded(&commands), []);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, PlayerCmd::LoadTrack { .. })),
            "the ring refused the selection's load"
        );

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(player.current_index(), 1);
    }

    fn armed_load(player: &PlayerImpl<TestPools>) -> LoadGeneration {
        player
            .phase
            .lock()
            .pending()
            .map(|pending| pending.load)
            .expect("a successor is armed")
    }

    fn resident(player: &PlayerImpl<TestPools>) -> (TrackId, LoadGeneration) {
        let resident = player
            .make_control()
            .resident_sync_observation()
            .expect("player open")
            .expect("a track is resident");
        (resident.item_id(), resident.load())
    }

    /// The sync owner pairs rendered evidence with the resident, so a
    /// withdrawn successor the audio thread stitched in becomes the resident
    /// under the load it was armed with, and its start is reported.
    #[kithara::test]
    fn a_withdrawn_successor_stitched_in_becomes_the_resident_under_its_armed_load() {
        let (player, audio_thread, owner, ids) =
            deck_with_armed_successor(refused_resource("third"));
        let second = armed_load(&player);
        select_third_with_its_load_refused(&player);
        let _ = owner.reconcile().expect("owner enters");

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(resident(&player), (ids[1], second));
        assert_eq!(
            owner.reconcile().expect("owner enters"),
            Some(SourceChange::Discontinuity)
        );
    }

    #[kithara::test]
    fn re_arming_keeps_a_successor_already_stitched_in_and_reports_it() {
        let (player, audio_thread, _owner, ids) = deck_with_armed_successor(resource("third"));

        player.arm_next(2).expect("re-arm accepted");
        assert_eq!(unloaded(&audio_thread.take_commands()), []);

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(player.current_index(), 1);
        assert_eq!(player.armed_next(), Some(2));
    }

    /// The audio thread stitches in whichever preload it finds first, so the
    /// withdrawn successor is not promoted just because no unload was seen.
    #[kithara::test]
    fn a_re_armed_successor_the_audio_thread_stitched_in_is_promoted() {
        let (player, audio_thread, _owner, ids) = deck_with_armed_successor(resource("third"));
        player.arm_next(2).expect("re-arm accepted");
        let third = armed_load(&player);

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("third", ids[2]));
        player.process_notifications();

        assert_eq!(player.current_index(), 2);
        assert_eq!(player.armed_next(), None);
        assert_eq!(resident(&player), (ids[2], third));
    }

    /// A successor shorter than the rest of its stitch block ends before the
    /// processor reports its start; its natural end still settles it.
    #[kithara::test]
    fn a_withdrawn_successor_that_ends_in_its_stitch_block_is_reported() {
        let (player, audio_thread, _owner, ids) =
            deck_with_armed_successor(refused_resource("third"));
        select_third_with_its_load_refused(&player);

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&ended("second", ids[1]));
        player.process_notifications();

        assert_eq!(player.current_index(), 1);
    }

    /// A cancel the full ring refused never reached the processor, so the
    /// successor stays preloaded behind the next leader and is reported when
    /// that one ends and it is stitched in.
    #[kithara::test]
    fn a_successor_whose_cancel_the_ring_refused_is_reported_when_stitched_in_later() {
        let (player, audio_thread, _owner, ids) = deck_with_armed_successor(resource("third"));
        while player.send_to_slot(PlayerCmd::SetPaused(true)).is_ok() {}
        player.unarm_next();
        assert!(
            !audio_thread
                .take_commands()
                .iter()
                .any(|command| matches!(command, PlayerCmd::CancelPreload { .. })),
            "the ring refused the cancel"
        );

        player
            .select_item(2, SelectionPlayback::Play)
            .expect("selection accepted");
        audio_thread.notify(&ended("third", ids[2]));
        audio_thread.notify(&started("second", ids[1]));
        player.process_notifications();

        assert_eq!(player.current_index(), 1);
    }

    /// A successor that fails on its first render never reports a start, but
    /// its failed end still names the track the processor stitched in.
    #[kithara::test]
    fn a_re_armed_successor_that_fails_in_its_stitch_block_is_reported() {
        let (player, audio_thread, _owner, ids) = deck_with_armed_successor(resource("third"));
        player.arm_next(2).expect("re-arm accepted");

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&PlayerNotification::PlaybackStopped {
            src: Arc::from("third"),
            item_id: ids[2],
            reason: TrackPlaybackStopReason::Failed(PlaybackFault::OutputRangeUnavailable),
            seek_epoch: 0,
        });
        player.process_notifications();

        assert_eq!(player.current_index(), 2);
        assert_eq!(player.armed_next(), None);
    }

    /// The processor can evict a preload whose cancel the full ring refused;
    /// once it reports the unload, that successor is out of question and the
    /// next track end promotes the armed successor as usual.
    #[kithara::test]
    fn an_unloaded_successor_whose_cancel_the_ring_refused_leaves_the_question() {
        let (player, audio_thread, _owner, ids) = deck_with_armed_successor(resource("third"));
        while player.send_to_slot(PlayerCmd::SetPaused(true)).is_ok() {}
        player.unarm_next();
        audio_thread.take_commands();
        player.arm_next(2).expect("re-arm accepted");

        audio_thread.notify(&PlayerNotification::Unloaded {
            src: Arc::from("second"),
            item_id: ids[1],
        });
        audio_thread.notify(&ended("first", ids[0]));
        player.process_notifications();

        assert_eq!(player.current_index(), 2);
        assert_eq!(player.armed_next(), None);
    }
}
