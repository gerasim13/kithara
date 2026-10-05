use kithara_audio::AudioEvent;
use kithara_bufpool::HasPool;
use kithara_events::{Envelope, EventSet, TrackId};
use kithara_platform::tokio::sync::broadcast::error::TryRecvError;
use kithara_play::{ItemRole, PlaybackFault, PlayerEvent};
use tracing::debug;

use super::{
    QueueControl,
    types::{CachedPosition, CrossfadeArm, Transition},
};
use crate::{
    ActionAtItemEnd,
    attempts::LoadClass,
    event::{AdvanceReason, ItemEvent, QueueEvent, TrackStatus},
};

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Every exit is reported: a pre-arm that declines leaves no trace in the
    /// event stream, and the next thing anyone sees is an end-of-item that
    /// advances nothing.
    pub(super) fn advance_loaded_successor(&self, current_id: TrackId, transition: Transition) {
        let track = current_id.as_u64();
        let action = self.action_at_item_end();
        if action != ActionAtItemEnd::Advance {
            debug!(
                track,
                ?action,
                "pre-arm declined: the queue does not advance"
            );
            return;
        }
        let Some(next) = self.next_selectable_entry(AdvanceReason::CrossfadePreArm) else {
            debug!(
                track,
                current = ?self.current().map(|entry| entry.id),
                player_index = self.player.current_index(),
                "pre-arm declined: navigation offers no successor"
            );
            return;
        };
        if !matches!(next.status, TrackStatus::Loaded) {
            debug!(
                track,
                next = next.id.as_u64(),
                status = ?next.status,
                current = ?self.current().map(|entry| entry.id),
                player_index = self.player.current_index(),
                "pre-arm declined: the successor is not loaded"
            );
            return;
        }

        let before_index = self.player.current_index();
        if let Err(error) =
            self.select_with_reason(next.id, transition, AdvanceReason::CrossfadePreArm)
        {
            debug!(
                %error,
                track,
                next = next.id.as_u64(),
                current = ?self.current().map(|entry| entry.id),
                player_index = self.player.current_index(),
                "pre-arm declined: the successor would not select"
            );
            return;
        }
        if self.player.current_index() != before_index {
            self.write_armed_for(CrossfadeArm::armed(current_id));
        }
    }

    /// If an advance was already armed from `tick()`, consume it and
    /// return `true` — the engine's trailing `ItemDidPlayToEnd` for
    /// the same track must not advance again.
    pub(super) fn consume_armed_advance(&self, ended_id: TrackId, pos: f64, dur: f64) -> bool {
        if self.take_armed_for_if_matches(ended_id) {
            debug!(
                track_id = ended_id.as_u64(),
                pos, dur, "consumed ItemDidPlayToEnd (armed pre-end)"
            );
            true
        } else {
            false
        }
    }

    /// `CurrentItemChanged` is edge-triggered and de-duplicated by
    /// `ItemQueue::announce_current_item`, so a dropped event cannot be recovered by waiting again.
    pub(super) fn drain_player_events(&self) {
        let mut lagged = false;
        {
            let mut rx = self.player_rx.lock();
            loop {
                match rx.try_recv() {
                    Ok(Envelope { event: ev, .. }) => self.process_player_event(&ev),
                    Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    Err(TryRecvError::Lagged(_)) => lagged = true,
                }
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
                player_index = self.player.current_index(),
                "the end names a track the cursor has left: not advancing"
            );
            return false;
        }
        if self.is_paused() {
            debug!(%track, pos, dur, "paused: not auto-advancing");
            return false;
        }
        if self.consume_armed_advance(track.id, pos, dur) {
            return false;
        }
        if !item.is_leading() {
            debug!(%track, pos, dur, ?item, "not the leading item: not advancing");
            return false;
        }
        true
    }

    pub(super) fn handle_current_item_changed(&self) {
        let idx = self.player.current_index();
        let id = self.lock_tracks().get(idx).map(|e| e.id);
        self.write_cached_position(CachedPosition::Unknown);
        self.bus.publish(QueueEvent::CurrentTrackChanged { id });
    }

    /// Gated on `item` for the same reason as
    /// [`Self::handle_item_did_play_to_end`]: the request names the track
    /// that is running out, not the one the queue is on. Committing an
    /// advance moves the queue's cursor at once while the outgoing track
    /// keeps rendering and keeps its own triggers armed, so its handover
    /// can still arrive after the queue has left it — and applied to the
    /// successor it reads as "this track is about to end" before a single
    /// block of the successor has been heard.
    pub(super) fn handle_handover_requested(&self, item: &ItemRole) {
        if self.is_paused() {
            return;
        }
        let Some(entry) = self.current() else {
            return;
        };
        if entry.id != item.track().id {
            return;
        }
        self.advance_loaded_successor(entry.id, Transition::Crossfade);
    }

    /// Gated on `item` for the same reason as
    /// [`Self::handle_item_did_play_to_end`]: the player reports the item
    /// that aborted, not the one being heard. Only a leading item's
    /// failure may skip and flag, and it flags the entry the event names —
    /// never one merely sharing its source, which a playlist repeating a
    /// track would take out of selection for the rest of the session.
    pub(super) fn handle_item_did_fail(&self, item: &ItemRole, fault: PlaybackFault) {
        let track = item.track();
        let snap = self.player.playback_snapshot();
        let pos = snap.map_or(0.0, |s| s.position());
        let dur = snap.map_or(0.0, |s| s.duration());
        debug!(%track, pos, dur, %fault, "ItemDidFail received — track aborted mid-stream");
        if !self.end_of_item_is_actionable(item, pos, dur) {
            return;
        }
        let reason = fault.to_string();
        self.set_status(track.id, TrackStatus::Failed(reason.clone()));
        let action = self.action_at_item_end();
        self.bus.publish(QueueEvent::TrackLoadFailed {
            reason,
            id: track.id,
            auto_skipped: action == ActionAtItemEnd::Advance,
        });
        match action {
            ActionAtItemEnd::Advance => {
                if let Err(error) =
                    self.advance_to_next_inner(Transition::None, AdvanceReason::TrackFailed)
                {
                    debug!(%error, "failed to advance after track failure");
                }
            }
            ActionAtItemEnd::Pause => self.pause(),
            ActionAtItemEnd::None => {}
        }
    }

    /// `item` is the player's verdict on which item in its arena ended.
    /// The player drains every active slot, and one slot holds more than
    /// one item, so an end says nothing on its own: an orphaned slot or
    /// the outgoing half of a crossfade reports its own end while the
    /// item being heard has minutes left. Only `Leading` advances.
    pub(super) fn handle_item_did_play_to_end(&self, item: &ItemRole) {
        let track = item.track();
        let snap = self.player.playback_snapshot();
        let pos = snap.map_or(0.0, |s| s.position());
        let dur = snap.map_or(0.0, |s| s.duration());
        debug!(%track, pos, dur, "ItemDidPlayToEnd received");
        if !self.end_of_item_is_actionable(item, pos, dur) {
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
            ActionAtItemEnd::Pause => self.pause(),
            ActionAtItemEnd::None => {}
        }
    }

    fn handle_prefetch_requested(&self) {
        if self.action_at_item_end() != ActionAtItemEnd::Advance {
            return;
        }
        let Some(next) = self.peek_selectable_entry() else {
            return;
        };
        if !matches!(next.status, TrackStatus::Consumed) {
            return;
        }
        let Some(source) = self.tracks.source(next.id) else {
            return;
        };
        self.set_status(next.id, TrackStatus::Pending);
        self.spawn_apply_after_load(next.id, source, LoadClass::Prefetch);
    }

    pub(super) fn process_player_event(&self, ev: &PlayerBusEvent) {
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
            PlayerBusEvent::Player(PlayerEvent::PrefetchRequested) => {
                self.handle_prefetch_requested();
            }
            PlayerBusEvent::Player(PlayerEvent::HandoverRequested { item }) => {
                self.handle_handover_requested(item);
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
    use kithara_audio::{DecodeErrorKind, TrackFailureKind};
    use kithara_events::{DEFAULT_EVENT_BUS_CAPACITY, SlotId, TrackId};
    use kithara_platform::sync::Arc;
    use kithara_play::{ItemRole, PlaybackFault, PlayerEvent, TrackRef};
    use kithara_test_utils::kithara;

    use super::PlayerBusEvent;

    use crate::{
        ActionAtItemEnd, QueueControl,
        event::{QueueEvent, TrackStatus},
        queue::{
            state::tests::{make_queue, wait_for_queue_event},
            types::SelectPhase,
        },
        test_pools::TestPools,
        track::{TrackRecord, TrackSource},
    };

    fn selected_second(queue: &QueueControl<TestPools>) -> (TrackId, TrackId) {
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
        let queue = make_queue();
        let (first, second) = selected_second(&queue);

        queue.handle_item_did_fail(
            &ItemRole::Leading(TrackRef::new(
                second,
                SlotId::new(0),
                Arc::from("https://example.com/repeated.mp3"),
            )),
            PlaybackFault::Source(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData,
            }),
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
    #[case::invalid_data(PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::InvalidData }))]
    #[case::unsupported_codec(PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::UnsupportedCodec }))]
    #[case::direct_io(PlaybackFault::Source(TrackFailureKind::Decode { kind: DecodeErrorKind::Io }))]
    #[case::output_rate(PlaybackFault::OutputRateMismatch)]
    #[case::output_range(PlaybackFault::OutputRangeUnavailable)]
    async fn a_leading_failure_records_the_fault_the_player_reported(
        #[case] fault: PlaybackFault,
    ) {
        let queue = make_queue();
        let (first, second) = selected_second(&queue);
        queue.set_action_at_item_end(ActionAtItemEnd::None);
        let mut events = queue.subscribe::<QueueEvent>();

        queue.process_player_event(&PlayerBusEvent::Player(PlayerEvent::ItemDidFail {
            item: ItemRole::Leading(TrackRef::new(
                second,
                SlotId::new(0),
                Arc::from("https://example.com/repeated.mp3"),
            )),
            fault,
        }));

        let Some(TrackStatus::Failed(reason)) = queue.track(second).map(|entry| entry.status)
        else {
            panic!("the entry named by the player event must be failed");
        };
        let published = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|envelope| match envelope.event {
                QueueEvent::TrackLoadFailed { id, reason, auto_skipped } => {
                    Some((id, reason, auto_skipped))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(published, [(second, reason.clone(), false)]);
        assert_eq!(
            reason,
            fault.to_string(),
            "status and event must report the real cause without fabricating an engine failure"
        );
        assert!(
            !matches!(queue.track(first).map(|entry| entry.status), Some(TrackStatus::Failed(_))),
            "a repeated URI does not make the first entry the failed item"
        );
    }

    #[kithara::test(tokio)]
    #[case::stale(false)]
    #[case::paused(true)]
    async fn a_stale_or_paused_failure_cannot_change_status_or_publish_a_failure(
        #[case] paused: bool,
    ) {
        let queue = make_queue();
        let (first, second) = selected_second(&queue);
        let reported = if paused { second } else { first };
        if paused {
            queue.player.play();
            assert!(
                queue.player.playback_snapshot().is_some(),
                "setup must allocate a player slot"
            );
            queue.pause();
            assert!(queue.player.is_paused(), "setup must pause the active player");
            assert_eq!(queue.current().map(|entry| entry.id), Some(second));
        }
        let before = queue.track(reported).expect("the reported entry exists").status;
        let mut events = queue.subscribe::<QueueEvent>();
        queue.process_player_event(&PlayerBusEvent::Player(PlayerEvent::ItemDidFail {
            item: ItemRole::Leading(TrackRef::new(
                reported,
                SlotId::new(0),
                Arc::from("https://example.com/repeated.mp3"),
            )),
            fault: PlaybackFault::Source(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData,
            }),
        }));
        assert_eq!(queue.current().map(|entry| entry.id), Some(second));
        assert_eq!(queue.track(reported).expect("the entry survives").status, before);
        assert!(
            std::iter::from_fn(|| events.try_recv().ok())
                .all(|envelope| !matches!(envelope.event, QueueEvent::TrackLoadFailed { .. })),
            "an ignored item failure must not publish a queue failure"
        );
    }

    #[kithara::test(tokio)]
    async fn background_end_and_failure_leave_the_current_entry_untouched() {
        let queue = make_queue();
        let (background, current) = selected_second(&queue);
        let item = ItemRole::Background(TrackRef::new(
            background,
            SlotId::new(1),
            Arc::from("https://example.com/repeated.mp3"),
        ));

        queue.handle_item_did_play_to_end(&item);
        queue.handle_item_did_fail(
            &item,
            PlaybackFault::Source(TrackFailureKind::Decode {
                kind: DecodeErrorKind::InvalidData,
            }),
        );

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
            let queue = make_queue();
            let first = TrackId::allocate();
            let second = TrackId::allocate();
            queue.tracks.lock().extend([
                TrackRecord::new(first, "first".into(), TrackSource::from("first")),
                TrackRecord::new(second, "second".into(), TrackSource::from("second")),
            ]);
            *queue.lock_pending_select_mut() = SelectPhase::Idle;
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
            assert!(matches!(
                *queue.lock_pending_select_mut(),
                SelectPhase::Idle
            ));
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

    #[kithara::test(tokio)]
    async fn lagged_player_events_resynchronize_current_track() {
        let queue = make_queue();
        let id = queue
            .append("https://example.com/lagged-events.mp3")
            .expect("open queue accepts a track");

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
