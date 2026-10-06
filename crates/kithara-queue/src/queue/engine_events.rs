use kithara_audio::AudioEvent;
use kithara_bufpool::HasPool;
use kithara_events::{Envelope, EventSet, TrackId};
use kithara_platform::tokio::sync::broadcast::error::TryRecvError;
use kithara_play::{ItemRole, PlaybackFault, PlayerEvent};
use tracing::debug;

use super::{
    Queue,
    types::{CachedPosition, Transition},
};
use crate::{
    ActionAtItemEnd,
    event::{AdvanceReason, ItemEvent, QueueEvent, TrackStatus},
};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// `CurrentItemChanged` is edge-triggered and de-duplicated by the deck,
    /// so a dropped event cannot be recovered by waiting again.
    pub(super) fn drain_player_events(&mut self) {
        let mut lagged = false;
        loop {
            match self.player_rx.try_recv() {
                Ok(Envelope { event: ev, .. }) => self.process_player_event(&ev),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                Err(TryRecvError::Lagged(_)) => lagged = true,
            }
        }
        if lagged {
            self.handle_current_item_changed();
        }
    }

    /// The gates an end-of-item report must clear before the queue acts on it.
    /// The player names the item it finished with, which is not necessarily the
    /// one being heard, and the queue may have moved on or been paused since.
    /// Every refusal is logged: a drop here is otherwise invisible, and all the
    /// report shows afterwards is silence.
    fn end_of_item_is_actionable(&self, item: &ItemRole, pos: f64, dur: f64) -> bool {
        let track = item.track();
        let current = self.current().map(|entry| entry.id);
        if current != Some(track.id) {
            debug!(
                %track,
                ?current,
                deck_item = ?self.player.current_item(),
                "the end names a track the cursor has left: not advancing"
            );
            return false;
        }
        if self.is_paused() {
            debug!(%track, pos, dur, "paused: not auto-advancing");
            return false;
        }
        if !item.is_leading() {
            debug!(%track, pos, dur, ?item, "not the leading item: not advancing");
            return false;
        }
        true
    }

    /// Whether the deck left `track` for a successor the queue still holds.
    /// The deck reports the end it left first, so it already leads the
    /// successor when the queue reads that end, and its announcement, not the
    /// end, moves the cursor there. A successor removed meanwhile is no
    /// track to follow, so the end advances past it.
    fn deck_led_on_from(&self, track: TrackId) -> bool {
        let deck_item = self.player.current_item();
        let led_on = deck_item.is_some_and(|item| {
            item != track && self.lock_tracks().iter().any(|entry| entry.id == item)
        });
        if led_on {
            debug!(%track, ?deck_item, "the deck led on to its successor: the end moves nothing");
        }
        led_on
    }

    /// The deck announces every item it starts, a removed one included, so
    /// the queue names only an item it still holds. A queued item other than
    /// the one under the cursor is one the deck took on its own, the
    /// successor held behind the track that ended or failed: the cursor
    /// follows it there, so a pause that gates that end's report cannot
    /// leave it behind, and the deck now holds the item. A queue that ran
    /// out has no cursor to move, though the deck keeps its last item.
    pub(super) fn handle_current_item_changed(&mut self) {
        let id = self
            .player
            .current_item()
            .filter(|id| self.lock_tracks().iter().any(|entry| entry.id == *id));
        if let Some(id) = id
            && let Some(left) = self.current()
            && left.id != id
        {
            let reason = if matches!(left.status, TrackStatus::Failed(_)) {
                AdvanceReason::TrackFailed
            } else {
                AdvanceReason::NaturalEof
            };
            self.commit_navigation_to(id);
            self.bus.publish(QueueEvent::CurrentTrackAdvance {
                reason,
                id: Some(id),
            });
            self.set_status(id, TrackStatus::Consumed);
        }
        self.write_cached_position(CachedPosition::Unknown);
        self.bus.publish(QueueEvent::CurrentTrackChanged { id });
    }

    /// Gated on `item` for the same reason as
    /// [`Self::handle_item_did_play_to_end`]: the player reports the item
    /// that aborted, not the one being heard. Only a leading item's
    /// failure may skip and flag, and it flags the entry the event names —
    /// never one merely sharing its source, which a playlist repeating a
    /// track would take out of selection for the rest of the session.
    pub(super) fn handle_item_did_fail(&mut self, item: &ItemRole, fault: PlaybackFault) {
        let track = item.track();
        let snap = self.player.playback_snapshot();
        let pos = snap.map_or(0.0, |s| s.position());
        let dur = snap.map_or(0.0, |s| s.duration());
        debug!(%track, pos, dur, %fault, "ItemDidFail received — track aborted mid-stream");
        if !self.end_of_item_is_actionable(item, pos, dur) {
            return;
        }
        let reason = format!("mid-stream engine failure: {fault}");
        self.set_status(track.id, TrackStatus::Failed(reason.clone()));
        let action = self.action_at_item_end();
        self.bus.publish(QueueEvent::TrackLoadFailed {
            reason,
            id: track.id,
            auto_skipped: action == ActionAtItemEnd::Advance,
        });
        match action {
            ActionAtItemEnd::Advance if !self.deck_led_on_from(track.id) => {
                if let Err(error) =
                    self.advance_to_next_inner(Transition::None, AdvanceReason::TrackFailed)
                {
                    debug!(%error, "failed to advance after track failure");
                }
            }
            ActionAtItemEnd::Pause => self.pause_inner(),
            ActionAtItemEnd::Advance | ActionAtItemEnd::None => {}
        }
    }

    /// `item` is the player's verdict on which item in its arena ended.
    /// The player drains every active slot, and one slot holds more than
    /// one item, so an end says nothing on its own: an orphaned slot or
    /// the outgoing half of a crossfade reports its own end while the
    /// item being heard has minutes left. Only `Leading` advances.
    pub(super) fn handle_item_did_play_to_end(&mut self, item: &ItemRole) {
        let track = item.track();
        let snap = self.player.playback_snapshot();
        let pos = snap.map_or(0.0, |s| s.position());
        let dur = snap.map_or(0.0, |s| s.duration());
        debug!(%track, pos, dur, "ItemDidPlayToEnd received");
        if !self.end_of_item_is_actionable(item, pos, dur) || self.deck_led_on_from(track.id) {
            return;
        }
        match self.action_at_item_end() {
            ActionAtItemEnd::Advance => {
                if let Err(error) =
                    self.advance_to_next_inner(Transition::Crossfade, AdvanceReason::NaturalEof)
                {
                    debug!(%error, "failed to advance after natural EOF");
                }
            }
            ActionAtItemEnd::Pause => self.pause_inner(),
            ActionAtItemEnd::None => {}
        }
    }

    pub(super) fn process_player_event(&mut self, ev: &PlayerBusEvent) {
        match ev {
            PlayerBusEvent::Player(PlayerEvent::ItemDidPlayToEnd { item }) => {
                self.handle_item_did_play_to_end(item);
            }
            PlayerBusEvent::Player(PlayerEvent::ItemDidFail { item, fault }) => {
                self.handle_item_did_fail(item, *fault);
            }
            PlayerBusEvent::Player(PlayerEvent::CurrentItemChanged { .. }) => {
                self.handle_current_item_changed();
            }
            PlayerBusEvent::Audio(AudioEvent::UnderrunStarted { .. }) => {
                self.bus.publish(ItemEvent::PlaybackStalled);
            }
            PlayerBusEvent::Audio(AudioEvent::UnderrunEnded { .. }) => {
                self.bus.publish(ItemEvent::PlaybackLikelyToKeepUp);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_audio::{DecodeErrorKind, mock::TestPcmReader};
    use kithara_events::{DEFAULT_EVENT_BUS_CAPACITY, SlotId, TrackId};
    use kithara_platform::{sync::Arc, time::Duration};
    use kithara_play::{
        ItemRole, PlaybackFault, PlayerEvent, Resource, SelectionPlayback, TrackRef,
    };
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;

    use crate::{
        ActionAtItemEnd, Queue,
        consts::TEST_SAMPLE_RATE,
        event::{AdvanceReason, QueueEvent, TrackStatus},
        queue::{
            state::tests::{make_queue, wait_for_queue_event},
            types::{SelectPhase, Transition},
        },
        test_pools::TestPools,
        track::{TrackRecord, TrackSource},
    };

    fn selected_second(queue: &mut Queue<TestPools>) -> (TrackId, TrackId) {
        let first = queue
            .append("https://example.com/repeated.mp3")
            .expect("open queue accepts first repeated source");
        let second = queue
            .append("https://example.com/repeated.mp3")
            .expect("open queue accepts second repeated source");
        let ids = [first, second];
        queue.lock_navigation_mut().select(second, &ids);
        queue.player.set_rate(1.0);
        (first, second)
    }

    #[kithara::test(tokio)]
    async fn leading_failure_marks_the_played_entry_when_sources_repeat() {
        let mut queue = make_queue();
        let (first, second) = selected_second(&mut queue);

        queue.handle_item_did_fail(
            &ItemRole::Leading(TrackRef::new(
                second,
                SlotId::new(0),
                Arc::from("https://example.com/repeated.mp3"),
            )),
            PlaybackFault::Decode(DecodeErrorKind::InvalidData),
        );

        assert!(
            !matches!(
                queue.track(first).map(|entry| entry.status),
                Some(TrackStatus::Failed(_))
            ),
            "an event for the second repeated source must not fail the first entry"
        );
        assert!(
            matches!(
                queue.track(second).map(|entry| entry.status),
                Some(TrackStatus::Failed(_))
            ),
            "the entry named by the player event must be failed"
        );
    }

    /// The queue's failure text must name the fault the player reported.
    ///
    /// The status and the published event both used to read one constant, so
    /// every mid-stream failure in a run report was the same indistinguishable
    /// string: a decode fault, an output rate the render context disagreed
    /// with, and a range it could not supply were one message. Nothing in a
    /// report could then say which defect ended the track.
    #[kithara::test(tokio)]
    async fn a_leading_failure_records_the_fault_the_player_reported() {
        let mut queue = make_queue();
        let (_first, second) = selected_second(&mut queue);

        queue.handle_item_did_fail(
            &ItemRole::Leading(TrackRef::new(
                second,
                SlotId::new(0),
                Arc::from("https://example.com/repeated.mp3"),
            )),
            PlaybackFault::OutputRateMismatch,
        );

        let Some(TrackStatus::Failed(reason)) = queue.track(second).map(|entry| entry.status)
        else {
            panic!("the entry named by the player event must be failed");
        };
        assert!(
            reason.contains("output sample-rate mismatch"),
            "the failure text must name the fault, got {reason:?}"
        );
    }

    #[kithara::test(tokio)]
    async fn background_end_and_failure_leave_the_current_entry_untouched() {
        let mut queue = make_queue();
        let (background, current) = selected_second(&mut queue);
        let item = ItemRole::Background(TrackRef::new(
            background,
            SlotId::new(1),
            Arc::from("https://example.com/repeated.mp3"),
        ));

        queue.handle_item_did_play_to_end(&item);
        queue.handle_item_did_fail(&item, PlaybackFault::Decode(DecodeErrorKind::InvalidData));

        assert_eq!(queue.current().map(|entry| entry.id), Some(current));
        assert!(
            !matches!(
                queue.track(background).map(|entry| entry.status),
                Some(TrackStatus::Failed(_))
            ),
            "a background failure must not fail its queue entry"
        );
    }

    #[kithara::test(tokio)]
    async fn pause_and_none_suppress_natural_eof_progression() {
        for action in [ActionAtItemEnd::Pause, ActionAtItemEnd::None] {
            let mut queue = make_queue();
            let first = TrackId::allocate();
            let second = TrackId::allocate();
            queue.tracks.lock().extend([
                TrackRecord::new(first, "first".into(), TrackSource::from("first")),
                TrackRecord::new(second, "second".into(), TrackSource::from("second")),
            ]);
            queue.pending_select = SelectPhase::Idle;
            queue.lock_navigation_mut().select(first, &[first, second]);
            queue.player.play();
            queue.set_action_at_item_end(action);
            let mut events = queue.subscribe();

            queue.handle_item_did_play_to_end(&ItemRole::Leading(TrackRef::new(
                first,
                SlotId::new(0),
                Arc::from("first"),
            )));

            assert_eq!(queue.current().map(|entry| entry.id), Some(first));
            assert!(matches!(queue.pending_select, SelectPhase::Idle));
            if action == ActionAtItemEnd::Pause {
                assert!(queue.is_paused());
            }
            assert!(
                !wait_for_queue_event(
                    &mut events,
                    |event| matches!(event, QueueEvent::QueueEnded),
                    50
                )
                .await
            );
        }
    }

    /// The tick that drains a natural end the queue pauses at pauses the deck
    /// on the owner it runs on, posting nothing back to it.
    #[kithara::test(tokio, timeout(Duration::from_secs(10)))]
    async fn a_tick_pauses_at_the_natural_end_it_drains() {
        let mut queue = make_queue();
        let first = TrackId::allocate();
        let second = TrackId::allocate();
        queue.tracks.lock().extend([
            TrackRecord::new(first, "first".into(), TrackSource::from("first")),
            TrackRecord::new(second, "second".into(), TrackSource::from("second")),
        ]);
        queue.pending_select = SelectPhase::Idle;
        queue.lock_navigation_mut().select(first, &[first, second]);
        queue.player.play();
        queue.set_action_at_item_end(ActionAtItemEnd::Pause);

        queue.player.bus().publish(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Leading(TrackRef::new(first, SlotId::new(0), Arc::from("first"))),
        });
        queue.tick().expect("the tick drains the end");

        assert!(queue.is_paused());
    }

    /// A track the queue holds a finished load for.
    fn admitted(queue: &mut Queue<TestPools>, url: &str) -> TrackId {
        let id = queue.append(url).expect("open queue accepts a track");
        let reader = TestPcmReader::new(AudioSpec::new(2, TEST_SAMPLE_RATE), 0.01);
        queue
            .tracks
            .admit(id, Resource::from_reader(reader, Some(Arc::from(url))));
        id
    }

    /// The deck leads on past a track that failed as it does past one that
    /// ended, and the cursor's move names the failure.
    #[kithara::test(tokio)]
    async fn the_deck_leading_on_past_a_failed_track_advances_for_the_failure() {
        let mut queue = make_queue();
        let failed = admitted(&mut queue, "https://example.com/failed.mp3");
        let successor = admitted(&mut queue, "https://example.com/successor.mp3");
        queue
            .select(failed, Transition::None)
            .expect("the first track is selected");
        queue.set_status(failed, TrackStatus::Failed("decode".into()));
        let resource = queue.tracks.take_resource(successor);
        queue
            .player
            .select(successor, resource, SelectionPlayback::Play)
            .expect("the deck takes the successor");

        let mut events = queue.subscribe();
        queue.handle_current_item_changed();

        let advanced = wait_for_queue_event(
            &mut events,
            |event| {
                matches!(
                    event,
                    QueueEvent::CurrentTrackAdvance {
                        reason: AdvanceReason::TrackFailed,
                        id: Some(id),
                    } if *id == successor
                )
            },
            200,
        )
        .await;
        assert!(
            advanced,
            "the cursor leaves the failed track for its failure"
        );
    }

    /// A queue that ran out names no current track though the deck keeps the
    /// last one: recovery from a lag finds no handover there.
    #[kithara::test(tokio)]
    async fn lag_recovery_keeps_an_ended_queue_ended() {
        let mut queue = make_queue();
        let last = admitted(&mut queue, "https://example.com/last.mp3");
        queue
            .select(last, Transition::None)
            .expect("the last track is selected");
        queue.lock_navigation_mut().finish();

        let mut events = queue.subscribe();
        queue.handle_current_item_changed();

        assert_eq!(queue.current().map(|entry| entry.id), None);
        assert!(
            !wait_for_queue_event(
                &mut events,
                |event| matches!(event, QueueEvent::CurrentTrackAdvance { .. }),
                50
            )
            .await,
            "the ended queue advances nowhere"
        );
    }

    /// A lagged receiver lost the deck's announcement, so recovery names
    /// the item the deck holds.
    #[kithara::test(tokio)]
    async fn lagged_player_events_resynchronize_current_track() {
        let mut queue = make_queue();
        let id = admitted(&mut queue, "https://example.com/lagged-events.mp3");
        queue
            .select(id, Transition::None)
            .expect("the loaded track is selected");

        for _ in 0..=DEFAULT_EVENT_BUS_CAPACITY {
            queue
                .player
                .bus()
                .publish(PlayerEvent::RateChanged { rate: 1.0 });
        }

        let mut events = queue.subscribe();
        queue
            .tick()
            .expect("BUG: tick returned error in test setup");

        let saw_current_track = wait_for_queue_event(
            &mut events,
            |event| {
                matches!(
                    event,
                    QueueEvent::CurrentTrackChanged {
                        id: Some(current_id)
                    } if *current_id == id
                )
            },
            200,
        )
        .await;
        assert!(
            saw_current_track,
            "lag recovery should re-announce the current track"
        );
    }
}

#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub(crate) enum PlayerBusEvent {
    Player(PlayerEvent),
    Audio(AudioEvent),
}
