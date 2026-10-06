use std::ops::Deref;

use kithara_bufpool::HasPool;
use kithara_platform::time::Duration;
use kithara_render::bridge::{PlayerNotification, TrackPlaybackStopReason};

#[cfg(test)]
use super::super::PlayerImpl;
use super::super::{
    core::{EnqueuedItem, PlayerRuntime},
    state::{ItemPresentation, PendingLoads, PendingNext, PendingNextState, Played},
    track::TrackCommand,
};
use crate::{
    CrossfadeSettings,
    api::{EngineEvent, SlotId, SuccessorLink, TrackId},
    error::PlayError,
    resource::Resource,
};

struct ActivatedPending {
    item_id: TrackId,
    duration_seconds: f64,
    presentation: ItemPresentation,
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
    /// Mark the armed successor `item` activated under a short lock.
    /// `Ok(None)` when it was already activated.
    fn activate_pending(&self, item: TrackId) -> Result<Option<ActivatedPending>, PlayError> {
        let mut phase = self.phase.lock();
        let pending = phase
            .pending_mut()
            .and_then(|slot| slot.as_mut())
            .ok_or(PlayError::NotReady)?;
        if pending.item_id != item {
            return Err(PlayError::ArmedItemMismatch {
                requested: item,
                armed: pending.item_id,
            });
        }
        let outcome = match std::mem::replace(&mut pending.state, PendingNextState::ActivatedReady)
        {
            PendingNextState::ActivatedReady => None,
            PendingNextState::Armed(presentation) => Some(ActivatedPending {
                item_id: pending.item_id,
                duration_seconds: pending.duration_seconds,
                presentation,
            }),
        };
        drop(phase);
        Ok(outcome)
    }

    /// Load `resource` as `item` into the audio-thread arena in `Preloading`
    /// state, chained behind the current item for a gapless `link` so the
    /// deck stitches it in sample-accurately, or left for a crossfade commit.
    ///
    /// A different armed successor is unloaded first; arming the one already
    /// armed changes nothing. On an error nothing is armed and the resource is
    /// spent.
    fn arm_next(
        &self,
        item: TrackId,
        resource: Resource,
        link: SuccessorLink,
    ) -> Result<(), PlayError> {
        let current = self.current_item();
        let mut phase = self.phase.lock();
        let to_unload = match phase.pending() {
            Some(existing) if existing.item_id == item => return Ok(()),
            Some(existing) => {
                let preserve = existing.state.activated() && Some(existing.item_id) == current;
                let withdrawn = phase.pending_loads_mut().and_then(PendingLoads::withdraw);
                withdrawn.filter(|_| !preserve)
            }
            None => None,
        };
        drop(phase);
        if let Some(pending) = to_unload {
            self.unload_pending(&pending);
        }

        let behind = match link {
            SuccessorLink::Gapless => current,
            SuccessorLink::Fade => None,
        };
        let EnqueuedItem {
            item_id,
            duration_seconds,
            presentation,
        } = self.enqueue_to_processor(item, resource, behind)?;
        if let Some(pending_slot) = self.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id,
                duration_seconds,
                state: PendingNextState::Armed(presentation),
            });
        }
        Ok(())
    }

    /// The armed successor. `None` when none is armed (or after
    /// `commit_next` has consumed it for the current handover).
    #[must_use]
    fn armed_next(&self) -> Option<TrackId> {
        self.phase
            .lock()
            .pending()
            .filter(|pending| !pending.state.activated())
            .map(|pending| pending.item_id)
    }

    /// Commit the armed successor `item` and start the cross-fade with this
    /// transition's own `settings`.
    ///
    /// Sends `FadeIn` to the audio thread for the armed track and publishes
    /// `CurrentItemChanged`.
    ///
    /// # Errors
    /// - [`PlayError::NotReady`] if no successor is armed.
    /// - [`PlayError::ArmedItemMismatch`] if `item` is not
    ///   [`Self::armed_next`].
    fn commit_next(&self, item: TrackId, settings: CrossfadeSettings) -> Result<(), PlayError> {
        let Some(activated) = self.activate_pending(item)? else {
            return Ok(());
        };

        self.adopt_presentation(
            activated.item_id,
            activated.duration_seconds,
            activated.presentation,
        );
        self.start_playback_with(activated.item_id, activated.duration_seconds, settings);
        self.publish_crossfade_started(settings.duration);
        self.core.current.announce(item);
        Ok(())
    }

    fn publish_crossfade_started(&self, seconds: f32) {
        let Some(slot) = self.slot() else {
            return;
        };
        self.core
            .engine
            .bus()
            .publish(EngineEvent::CrossfadeStarted {
                from: slot,
                to: slot,
                duration: Duration::from_secs_f32(seconds.max(0.0)),
            });
    }

    /// Drop the armed successor without committing.
    ///
    /// Unloads it from the audio thread and clears the pending slot. Skips
    /// the unload if the successor has already been activated as the current
    /// item (the activated track is now the leading one — unloading would
    /// silence playback).
    fn unarm_next(&self) {
        self.unarm_next_internal(self.current_item());
    }

    fn unarm_next_internal(&self, current: Option<TrackId>) {
        let pending = self
            .phase
            .lock()
            .pending_loads_mut()
            .and_then(PendingLoads::withdraw);
        let Some(pending) = pending else {
            return;
        };
        let preserve_active_current = pending.state.activated() && Some(pending.item_id) == current;
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
    /// reports the track it played.
    fn unload_pending(&self, pending: &PendingNext) {
        let item_id = pending.item_id;
        if pending.state.activated() {
            let _ =
                self.with_tracks(|tracks, out| tracks.apply(item_id, TrackCommand::Release, out));
        } else if self
            .with_tracks(|tracks, out| tracks.apply(item_id, TrackCommand::Withdraw, out))
            .is_err()
            && let Some(loads) = self.phase.lock().pending_loads_mut()
        {
            loads.cancel_refused(item_id);
        }
    }
}

impl<S> PlayerRuntime<S>
where
    S: HasPool<f32>,
{
    /// Attach `resource` to the deck as `item`, ahead of the current item and
    /// joined to it by `link`.
    ///
    /// # Errors
    /// The failure to allocate its buffers or the deck's refusal for want of
    /// room. Nothing is armed then, and the resource is spent.
    pub fn arm_next(
        &self,
        item: TrackId,
        resource: Resource,
        link: SuccessorLink,
    ) -> Result<(), PlayError> {
        Handover::new(self).arm_next(item, resource, link)
    }

    /// The successor armed on the deck and not yet committed.
    #[must_use]
    pub fn armed_next(&self) -> Option<TrackId> {
        Handover::new(self).armed_next()
    }

    /// Commit the armed successor `item` with this transition's `settings`.
    ///
    /// # Errors
    /// [`PlayError::NotReady`] without an armed successor,
    /// [`PlayError::ArmedItemMismatch`] when another is armed.
    pub fn commit_next(&self, item: TrackId, settings: CrossfadeSettings) -> Result<(), PlayError> {
        Handover::new(self).commit_next(item, settings)
    }

    /// Retire a committed successor once a track ends. An armed one waits for
    /// the processor's report that it played.
    pub(crate) fn retire_activated_at_end(&self) {
        let _ = self
            .phase
            .lock()
            .pending_loads_mut()
            .and_then(PendingLoads::take_activated);
    }

    /// Settle the successor once the processor reports the track it played,
    /// by its start or its natural or failed end: the armed successor or a
    /// withdrawn one stitched in becomes current, and its start takes on the
    /// epoch it was stitched in under, so the deck publishes its playhead. A
    /// withdrawn successor the processor reports unloaded can no longer be
    /// stitched in and leaves the question.
    pub(crate) fn settle_withdrawal(&self, slot_id: SlotId, notification: &PlayerNotification) {
        if self.slot() != Some(slot_id) {
            return;
        }
        let (played, epoch) = match notification {
            PlayerNotification::PlaybackStarted { item_id, epoch, .. } => (*item_id, Some(*epoch)),
            PlayerNotification::PlaybackStopped {
                reason: TrackPlaybackStopReason::Eof | TrackPlaybackStopReason::Failed(_),
                item_id,
                ..
            } => (*item_id, None),
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
        let duration_seconds = match settled {
            Some(Played::Armed {
                duration_seconds,
                presentation,
            }) => {
                self.adopt_presentation(played, duration_seconds, presentation);
                duration_seconds
            }
            Some(Played::Withdrawn) => 0.0,
            None => return,
        };
        if let Some(epoch) = epoch
            && let Some(playback) = self.slot_playback()
        {
            playback.take_on(epoch, duration_seconds);
        }
        self.core.current.announce(played);
    }

    pub fn unarm_next(&self) {
        Handover::new(self).unarm_next();
    }

    pub(crate) fn unarm_next_internal(&self, current: Option<TrackId>) {
        Handover::new(self).unarm_next_internal(current);
    }
}

#[cfg(test)]
mod tests {
    #[derive(Clone, Debug, kithara_events::EventSet)]
    enum TestEvent {
        Engine(EngineEvent),
        Player(PlayerEvent),
    }

    use kithara_audio::mock::{AudioControlMock, AudioReadMock, AudioSessionMock};
    use kithara_events::{Envelope, EventBus};
    use kithara_platform::sync::Arc;
    use kithara_render::bridge::{DeckPart, PlaybackFault, TrackTransition};
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;
    use kithara_warp::BeatGrid;
    use unimock::{MockFn, Unimock, matching};

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig,
        api::{EngineEvent, PlayerEvent, SelectionPlayback},
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
        let err = player
            .commit_next(TrackId::allocate(), CrossfadeSettings::default())
            .expect_err("must error");
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

        let next = TrackId::allocate();
        if let Some(pending_slot) = player.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id: next,
                state: PendingNextState::Armed(ItemPresentation {
                    beat_grid: Arc::default(),
                    abr_handle: None,
                    staging: None,
                }),
                duration_seconds: 162.0,
            });
        }

        player
            .commit_next(next, CrossfadeSettings::default())
            .expect("commit_next must succeed");

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
        let next = TrackId::allocate();
        if let Some(pending_slot) = player.phase.lock().pending_mut() {
            *pending_slot = Some(PendingNext {
                item_id: next,
                state: PendingNextState::Armed(ItemPresentation {
                    beat_grid: Arc::default(),
                    abr_handle: None,
                    staging: None,
                }),
                duration_seconds: 162.0,
            });
        }

        player
            .commit_next(next, CrossfadeSettings::default())
            .expect("commit_next must succeed");
        let playback = player
            .slot()
            .and_then(|slot| player.core.engine.slot_playback(slot))
            .expect("the slot must carry playback state");
        kithara_render::mock::publish_playhead(&playback, 62.3, 64.295);

        assert_eq!(player.duration_seconds(), Some(162.0));
        assert_eq!(player.position_seconds(), Some(0.0));
    }

    /// A `FadeIn` the full command queue rejects never reaches the audio
    /// thread, so the player must keep reporting the item the audio thread
    /// plays instead of waiting on a handover that will not happen.
    #[kithara::test]
    fn a_rejected_fade_in_leaves_the_playhead_on_the_playing_item() {
        let (session, audio_thread) = mock::session_with_mock();
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(mock::SAMPLE_RATE)
                .worker(worker())
                .session(session)
                .build(),
        );
        let [first, second] = [TrackId::allocate(), TrackId::allocate()];
        player
            .select(first, Some(resource("first")), SelectionPlayback::Play)
            .expect("the first item loads");
        let lead = audio_thread
            .take_commands()
            .iter()
            .find_map(|command| match command {
                DeckPart::Fade(TrackTransition::FadeIn { epoch, .. }) => Some(*epoch),
                _ => None,
            })
            .expect("play fades the first item in");
        player
            .arm_next(second, resource("second"), SuccessorLink::Gapless)
            .expect("preload accepted");
        let playback = player
            .slot_playback()
            .expect("the slot must carry playback state");
        kithara_render::mock::adopt(&playback, lead, 62.3, 64.295);
        while player.send_to_slot(DeckPart::StartAll).is_ok() {}

        player
            .commit_next(second, CrossfadeSettings::default())
            .expect("commit_next must succeed");

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
        player
            .select(ids[0], Some(resource("first")), SelectionPlayback::Play)
            .expect("the first item loads");
        player
            .arm_next(ids[1], resource("second"), SuccessorLink::Gapless)
            .expect("preload accepted");
        audio_thread.take_commands();
        (player, audio_thread, ids)
    }

    fn unloaded(commands: &[DeckPart]) -> Vec<TrackId> {
        commands
            .iter()
            .filter_map(|command| match command {
                DeckPart::Detach { item_id } => Some(*item_id),
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

    fn started(src: &str, item_id: TrackId, epoch: u64) -> PlayerNotification {
        PlayerNotification::PlaybackStarted {
            src: Arc::from(src),
            item_id,
            epoch,
        }
    }

    /// The epoch the processor stitches `item_id` in under, from the chain that armed it.
    fn chain_epoch(commands: &[DeckPart], item_id: TrackId) -> u64 {
        commands
            .iter()
            .find_map(|command| match command {
                DeckPart::Chain { to, epoch, .. } if *to == item_id => Some(*epoch),
                _ => None,
            })
            .expect("the successor is chained")
    }

    /// Select `third` while the command ring has room only for the
    /// selection's setting and the successor's withdrawal.
    fn select_third_with_its_load_refused(player: &PlayerImpl<TestPools>, third: TrackId) {
        for _ in 0..30 {
            player
                .send_to_slot(DeckPart::StopAll)
                .expect("fixture leaves room for the setting and the withdrawal");
        }
        let _ = player.select(third, Some(resource("third")), SelectionPlayback::Play);
    }

    /// The audio thread ends `first` and stitches `second` in, before it
    /// reads anything sent since `second` was armed.
    fn stitch_second_in(audio_thread: &mock::SessionMock, ids: [TrackId; 3]) {
        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("second", ids[1], 0));
    }

    /// The new selection fades out a successor the audio thread already
    /// stitched in; withdrawing that successor must not cut it off first.
    #[kithara::test]
    fn selecting_another_item_cancels_the_successor_preload_instead_of_unloading_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();

        player
            .select(ids[2], Some(resource("third")), SelectionPlayback::Play)
            .expect("selection accepted");
        let commands = audio_thread.take_commands();
        assert_eq!(unloaded(&commands), []);
        assert!(commands.iter().any(
            |command| matches!(command, DeckPart::Withdraw { item_id } if *item_id == ids[1])
        ));

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(
            player.current_item(),
            Some(ids[2]),
            "the selection leads over the successor it fades out"
        );
    }

    /// A selection whose load the full command ring refuses leaves the
    /// successor stitched in as the track that plays, so the track end must
    /// report it.
    #[kithara::test]
    fn a_refused_selection_keeps_a_successor_already_stitched_in_and_reports_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();

        select_third_with_its_load_refused(&player, ids[2]);
        let commands = audio_thread.take_commands();
        assert_eq!(unloaded(&commands), []);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, DeckPart::Attach { .. })),
            "the ring refused the selection's load"
        );

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[1]));
    }

    #[kithara::test]
    fn re_arming_keeps_a_successor_already_stitched_in_and_reports_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();

        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");
        assert_eq!(unloaded(&audio_thread.take_commands()), []);

        stitch_second_in(&audio_thread, ids);
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[1]));
        assert_eq!(player.armed_next(), Some(ids[2]));
    }

    /// The audio thread stitches in whichever preload it finds first, so the
    /// withdrawn successor is not promoted just because no unload was seen.
    #[kithara::test]
    fn a_re_armed_successor_the_audio_thread_stitched_in_is_promoted() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("third", ids[2], 0));
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[2]));
        assert_eq!(player.armed_next(), None);
    }

    /// A successor shorter than the rest of its stitch block ends before the
    /// processor reports its start; its natural end still settles it.
    #[kithara::test]
    fn a_withdrawn_successor_that_ends_in_its_stitch_block_is_reported() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        select_third_with_its_load_refused(&player, ids[2]);

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&ended("second", ids[1]));
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[1]));
    }

    /// A cancel the full ring refused never reached the processor, so the
    /// successor stays preloaded behind the next leader and is reported when
    /// that one ends and it is stitched in.
    #[kithara::test]
    fn a_successor_whose_cancel_the_ring_refused_is_reported_when_stitched_in_later() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        while player.send_to_slot(DeckPart::StopAll).is_ok() {}
        player.unarm_next();
        assert!(
            !audio_thread
                .take_commands()
                .iter()
                .any(|command| matches!(command, DeckPart::Withdraw { .. })),
            "the ring refused the cancel"
        );

        player
            .select(ids[2], Some(resource("third")), SelectionPlayback::Play)
            .expect("selection accepted");
        audio_thread.notify(&ended("third", ids[2]));
        audio_thread.notify(&started("second", ids[1], 0));
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[1]));
    }

    /// A successor that fails on its first render never reports a start, but
    /// its failed end still names the track the processor stitched in.
    #[kithara::test]
    fn a_re_armed_successor_that_fails_in_its_stitch_block_is_reported() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&PlayerNotification::PlaybackStopped {
            src: Arc::from("third"),
            item_id: ids[2],
            reason: TrackPlaybackStopReason::Failed(PlaybackFault::OutputRangeUnavailable),
            seek_epoch: 0,
        });
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[2]));
        assert_eq!(player.armed_next(), None);
    }

    /// The processor can evict a preload whose cancel the full ring refused;
    /// once it reports the unload, that successor is out of question and the
    /// processor's start of the armed successor promotes it as usual.
    #[kithara::test]
    fn an_unloaded_successor_whose_cancel_the_ring_refused_leaves_the_question() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        while player.send_to_slot(DeckPart::StopAll).is_ok() {}
        player.unarm_next();
        audio_thread.take_commands();
        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");

        audio_thread.notify(&PlayerNotification::Unloaded {
            src: Arc::from("second"),
            item_id: ids[1],
        });
        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("third", ids[2], 0));
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[2]));
        assert_eq!(player.armed_next(), None);
    }

    /// The predecessor's end does not crown the armed successor: the
    /// processor may not have stitched it in. Its own start does.
    #[kithara::test]
    fn an_armed_successor_is_not_current_until_the_processor_plays_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");

        audio_thread.notify(&ended("first", ids[0]));
        player.process_notifications();
        assert_eq!(player.current_item(), Some(ids[0]));
        assert_eq!(player.armed_next(), Some(ids[2]));

        audio_thread.notify(&started("third", ids[2], 0));
        player.process_notifications();
        assert_eq!(player.current_item(), Some(ids[2]));
        assert_eq!(player.armed_next(), None);
    }

    /// The processor stitches the armed successor in under the epoch its
    /// chain carries, and publishes its playhead only once the player takes
    /// that epoch on: the player does as it makes the successor current.
    #[kithara::test]
    fn a_stitched_successor_is_taken_on_as_it_becomes_current() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");
        let epoch = chain_epoch(&audio_thread.take_commands(), ids[2]);
        let playback = player
            .slot_playback()
            .expect("the slot must carry playback state");
        assert!(!kithara_render::mock::publishes(&playback, epoch));

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("third", ids[2], epoch));
        player.process_notifications();

        assert_eq!(player.current_item(), Some(ids[2]));
        assert!(kithara_render::mock::publishes(&playback, epoch));
    }

    /// Arming ahead leaves the playing item's geometry published; the armed
    /// successor's grid takes over only once the processor plays it.
    #[kithara::test]
    fn an_armed_successor_publishes_its_grid_only_once_the_processor_plays_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        let playing = player.core.track_grid.snapshot().revision();

        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("re-arm accepted");
        assert_eq!(
            player.core.track_grid.snapshot().revision(),
            playing,
            "arming a successor must not publish its grid"
        );

        audio_thread.notify(&ended("first", ids[0]));
        audio_thread.notify(&started("third", ids[2], 0));
        player.process_notifications();
        assert!(
            player.core.track_grid.snapshot().revision() > playing,
            "the successor the processor plays publishes its grid"
        );
    }

    /// Only a gapless successor is chained behind the leading item; a
    /// crossfade's waits for its commit.
    #[kithara::test]
    fn only_a_gapless_successor_is_chained_behind_the_leading_item() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player.unarm_next();
        audio_thread.take_commands();

        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Fade)
            .expect("crossfade arm accepted");
        let commands = audio_thread.take_commands();
        assert!(commands.iter().any(
            |command| matches!(command, DeckPart::Attach { item_id, .. } if *item_id == ids[2])
        ));
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, DeckPart::Chain { .. })),
            "a crossfade successor must not be chained"
        );

        player.unarm_next();
        audio_thread.take_commands();
        player
            .arm_next(
                TrackId::allocate(),
                resource("fourth"),
                SuccessorLink::Gapless,
            )
            .expect("gapless arm accepted");
        assert!(
            audio_thread
                .take_commands()
                .iter()
                .any(|command| matches!(command, DeckPart::Chain { from, .. } if *from == ids[0])),
            "a gapless successor is chained behind the leading item"
        );
    }

    /// A deck that has room for one more batch takes a gapless successor
    /// whole: its attach and the chain behind the leading item are admitted
    /// together, so a successor is never armed without its chain.
    #[kithara::test]
    fn a_gapless_successor_is_chained_in_the_batch_that_attaches_it() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player.unarm_next();
        audio_thread.take_commands();
        let mut capacity = 0;
        while player.send_to_slot(DeckPart::StopAll).is_ok() {
            capacity += 1;
        }
        audio_thread.take_commands();
        for _ in 1..capacity {
            player
                .send_to_slot(DeckPart::StopAll)
                .expect("the ring has room below its capacity");
        }

        player
            .arm_next(ids[2], resource("third"), SuccessorLink::Gapless)
            .expect("one batch has room");

        assert!(
            audio_thread.take_commands().iter().any(|command| matches!(
                command,
                DeckPart::Chain { from, to, .. } if *from == ids[0] && *to == ids[2]
            )),
            "the armed successor must be chained behind the leading item"
        );
    }

    /// An arm the full deck refuses fails and leaves no successor armed:
    /// nothing reached the processor to stitch in.
    #[kithara::test]
    fn an_arm_the_deck_has_no_room_for_fails_and_arms_nothing() {
        let (player, audio_thread, ids) = deck_with_armed_successor();
        player.unarm_next();
        audio_thread.take_commands();
        while player.send_to_slot(DeckPart::StopAll).is_ok() {}

        let armed = player.arm_next(ids[2], resource("third"), SuccessorLink::Gapless);

        assert!(
            matches!(armed, Err(PlayError::SlotChannelFull { .. })),
            "the refused arm must fail: {armed:?}"
        );
        assert_eq!(player.armed_next(), None);
    }
}
