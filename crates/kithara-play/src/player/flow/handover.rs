use std::ops::Deref;

use kithara_bufpool::HasPool;
use kithara_platform::{sync::Arc, time::Duration};

#[cfg(test)]
use super::super::PlayerImpl;
use super::super::{
    core::PlayerRuntime,
    state::{PendingLoads, PendingNext, PendingNextState},
};
use crate::{
    api::{EngineEvent, SlotId, TrackId},
    bridge::PlayerCmd,
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
            let duration_seconds = pending.duration_seconds;
            pending.state = PendingNextState::ActivatedReady;
            Some(ActivatedPending {
                item_id,
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

        let Some((item_id, src, duration_seconds)) = self.enqueue_to_processor(index)? else {
            return Ok(None);
        };
        if let Some(pending_slot) = self.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id,
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

        self.start_playback(activated.item_id, activated.duration_seconds);
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

    /// Unload a pending track the handover no longer wants. An armed one may
    /// already have been stitched in, so the processor drops it only while it
    /// still preloads; the phase keeps it withdrawn until the processor
    /// reports the track it started.
    fn unload_pending(&self, pending: &PendingNext) {
        let item_id = pending.item_id;
        let command = if pending.state.activated() {
            PlayerCmd::UnloadTrack { item_id }
        } else {
            PlayerCmd::CancelPreload { item_id }
        };
        let _ = self.send_to_slot(command);
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
        let index = pending.index;
        self.core.items.set_current(index);
        self.announce_current_item(index);
    }

    /// Settle a withdrawal still in question once the processor reports the
    /// track it started: the queue follows the successor stitched in, and one
    /// removed from the queue since keeps playing unannounced.
    pub(crate) fn settle_withdrawal(&self, slot_id: SlotId, item_id: TrackId) {
        if self.slot() != Some(slot_id) {
            return;
        }
        let settled = self
            .phase
            .lock()
            .pending_loads_mut()
            .is_some_and(|loads| loads.settle_started(item_id));
        let Some(index) = settled.then(|| self.core.items.index_of(item_id)).flatten() else {
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
    #[derive(Clone, Debug, kithara_events::EventSet)]
    enum TestEvent {
        Engine(EngineEvent),
        Player(PlayerEvent),
    }

    use std::sync::atomic::Ordering;

    use kithara_audio::mock::{AudioControlMock, AudioReadMock, AudioSessionMock};
    use kithara_events::{Envelope, EventBus};
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig,
        api::{EngineEvent, PlayerEvent, SelectionPlayback},
        bridge::{PlayerNotification, TrackPlaybackStopReason},
        mock,
        player::PlayerConfig,
        resource::Resource,
        test_pools::{TestPools, pools},
    };

    fn worker() -> PlayWorker<TestPools> {
        PlayWorker::new(PlayWorkerConfig::builder(pools()).build())
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

        if let Some(pending_slot) = player.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id: TrackId::allocate(),
                src: Arc::from("next.mp3"),
                state: PendingNextState::Armed,
                index: 1,
                duration_seconds: 162.0,
            });
        }

        player.commit_next(1).expect("commit_next must succeed");

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

    /// An audio block already under way when the handover is committed still
    /// renders the outgoing item and publishes its playhead at the end. The
    /// committed item must stay the one the player reports until the audio
    /// thread takes it on.
    #[kithara::test]
    fn committed_item_outlives_a_block_the_outgoing_item_was_rendering() {
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
                src: Arc::from("next.mp3"),
                state: PendingNextState::Armed,
                index: 1,
                duration_seconds: 162.0,
            });
        }

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
    /// thread, so the player must keep reporting the item the audio thread
    /// plays instead of waiting on a handover that will not happen.
    #[kithara::test]
    fn a_rejected_fade_in_leaves_the_playhead_on_the_playing_item() {
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
                src: Arc::from("next.mp3"),
                state: PendingNextState::Armed,
                index: 1,
                duration_seconds: 162.0,
            });
        }
        let playback = player
            .slot()
            .and_then(|slot| player.core.engine.slot_playback(slot))
            .expect("the slot must carry playback state");
        playback.position.store(62.3, Ordering::Relaxed);
        playback.duration.store(64.295, Ordering::Relaxed);
        while player.send_to_slot(PlayerCmd::SetPaused(false)).is_ok() {}

        player.commit_next(1).expect("commit_next must succeed");

        assert_eq!(player.duration_seconds(), Some(64.295));
        assert_eq!(player.position_seconds(), Some(62.3));
    }

    fn resource(src: &str) -> Resource {
        let reader = Unimock::new((
            AudioSessionMock::event_bus
                .each_call(matching!())
                .answers(&|mock| mock.make_ref(EventBus::new(1))),
            AudioSessionMock::duration
                .each_call(matching!())
                .returns(Some(Duration::from_secs(1))),
            AudioReadMock::spec
                .each_call(matching!())
                .returns(AudioSpec::new(2, mock::SAMPLE_RATE)),
            AudioControlMock::preload
                .next_call(matching!())
                .returns(Ok(())),
        ));
        Resource::from_reader(reader, Some(Arc::from(src)))
    }

    /// A deck playing `first` with `second` armed gaplessly behind it, every
    /// command sent so far taken.
    fn deck_with_armed_successor() -> (PlayerImpl<TestPools>, Arc<mock::SessionMock>, [TrackId; 3])
    {
        let (session, audio_thread) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session)
                .build(),
        );
        let ids = [
            TrackId::allocate(),
            TrackId::allocate(),
            TrackId::allocate(),
        ];
        for (src, item_id) in ["first", "second", "third"].into_iter().zip(ids) {
            player.insert(resource(src), item_id, None);
        }
        player.play();
        player.arm_next(1).expect("preload accepted");
        audio_thread.take_commands();
        (player, audio_thread, ids)
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

    fn first_ended(ids: [TrackId; 3]) -> PlayerNotification {
        PlayerNotification::PlaybackStopped {
            src: Arc::from("first"),
            item_id: ids[0],
            reason: TrackPlaybackStopReason::Eof,
            seek_epoch: 0,
        }
    }

    /// The audio thread ends `first` and stitches `second` in, before it
    /// reads anything sent since `second` was armed.
    fn stitch_second_in(audio_thread: &mock::SessionMock, ids: [TrackId; 3]) {
        audio_thread.notify(&first_ended(ids));
        audio_thread.notify(&PlayerNotification::PlaybackStarted {
            src: Arc::from("second"),
            item_id: ids[1],
        });
    }

    /// The new selection fades out a successor the audio thread already
    /// stitched in; withdrawing that successor must not cut it off first.
    #[kithara::test]
    fn selecting_another_item_cancels_the_successor_preload_instead_of_unloading_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();

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
        let (player, audio_thread, ids) = deck_with_armed_successor();
        for _ in 0..30 {
            player
                .send_to_slot(PlayerCmd::SetPaused(true))
                .expect("fixture leaves room for the setting and the withdrawal");
        }

        let _ = player.select_item(2, SelectionPlayback::Play);
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

    #[kithara::test]
    fn re_arming_keeps_a_successor_already_stitched_in_and_reports_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();

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
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player.arm_next(2).expect("re-arm accepted");

        audio_thread.notify(&first_ended(ids));
        audio_thread.notify(&PlayerNotification::PlaybackStarted {
            src: Arc::from("third"),
            item_id: ids[2],
        });
        player.process_notifications();

        assert_eq!(player.current_index(), 2);
        assert_eq!(player.armed_next(), None);
    }
}
