use std::{ops::Deref, sync::atomic::Ordering};

use kithara_platform::sync::Arc;

use super::super::core::PlayerRuntime;
use crate::{
    api::{EngineEvent, PlayerEvent, SlotId},
    bridge::{PlayerNotification, TrackPlaybackStopReason},
};

struct Notifier<'a> {
    player: &'a PlayerRuntime,
}

impl<'a> Notifier<'a> {
    const fn new(player: &'a PlayerRuntime) -> Self {
        Self { player }
    }
}

impl Deref for Notifier<'_> {
    type Target = PlayerRuntime;

    fn deref(&self) -> &Self::Target {
        self.player
    }
}

impl Notifier<'_> {
    fn dispatch_notification(&self, slot_id: SlotId, notification: PlayerNotification) {
        if self.natural_end_outranked_by_seek(slot_id, &notification) {
            tracing::debug!(
                src = ?notification.src(),
                "dropping a natural end outranked by a published seek"
            );
            return;
        }
        let from_current_item = self.slot() == Some(slot_id);
        let emitted = player_events_from_notification(self, &notification);
        let emitted_any = !emitted.is_empty();
        for event in emitted {
            self.core.engine.bus().publish(event);
        }

        match notification.clone() {
            PlayerNotification::Requested => {
                self.handle_track_requested();
            }
            PlayerNotification::HandoverRequested => {
                self.handle_handover_requested();
            }
            PlayerNotification::PlaybackStopped {
                reason: TrackPlaybackStopReason::Eof,
                ..
            } => {
                self.handle_track_playback_stopped(from_current_item, notification);
            }
            _ => {
                if let Some(event) =
                    player_event_from_notification(notification.clone(), from_current_item)
                {
                    self.core.engine.bus().publish(event);
                } else if !emitted_any {
                    tracing::trace!(
                        src = ?notification.src(),
                        ?notification,
                        "unhandled player notification"
                    );
                }
            }
        }
    }

    /// Bug #5's dispatch-side sibling: a natural end minted at an older
    /// epoch describes a position the user has already left — a newer
    /// published seek revives the track (`apply_seek`), and delivering the
    /// stale end would hand the queue an `ItemDidPlayToEnd` it answers with
    /// an auto-advance out from under the accepted seek. Compared with `!=`,
    /// not `<`: epochs wrap, and `withdraw_seek_epoch` legally steps the
    /// published value back. `Stop` and `Failed` are never fenced — a broken
    /// source stays broken across a seek, and a slot without playback state
    /// has nothing to outrank the end.
    fn natural_end_outranked_by_seek(
        &self,
        slot_id: SlotId,
        notification: &PlayerNotification,
    ) -> bool {
        let PlayerNotification::PlaybackStopped {
            reason: TrackPlaybackStopReason::Eof,
            seek_epoch,
            ..
        } = notification
        else {
            return false;
        };
        self.core
            .engine
            .slot_playback(slot_id)
            .is_some_and(|playback| playback.seek_epoch.load(Ordering::SeqCst) != *seek_epoch)
    }

    fn finalize_handover_if_armed(&self) {
        let pending = self.phase.lock().pending_mut().and_then(Option::take);
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
        self.publish_current_track_snapshot(pending.duration_seconds);
        self.core.items.set_current(index);
        self.announce_current_item(index);
    }

    fn handle_handover_requested(&self) {
        if self.crossfade_duration() <= 0.0 {
            return;
        }
        self.core
            .engine
            .bus()
            .publish(PlayerEvent::HandoverRequested);
        if self.auto_advance_enabled()
            && let Some(idx) = self.armed_next()
        {
            let _ = self.commit_next(idx);
        }
    }

    /// `from_current_item` is read before `finalize_handover_if_armed`
    /// runs, so the phase still names the slot that ended. With a
    /// crossfade the incoming slot was already promoted by `commit_next`,
    /// so an outgoing end reports `false` — the queue advanced on the
    /// pre-arm instead.
    fn handle_track_playback_stopped(
        &self,
        from_current_item: bool,
        notification: PlayerNotification,
    ) {
        if let Some(event) = player_event_from_notification(notification, from_current_item) {
            self.core.engine.bus().publish(event);
        }

        self.finalize_handover_if_armed();
    }

    fn handle_track_requested(&self) {
        self.core
            .engine
            .bus()
            .publish(PlayerEvent::PrefetchRequested);
        if self.auto_advance_enabled() {
            let next_index = self.current_index() + 1;
            if next_index < self.item_count() {
                let _ = self.arm_next(next_index);
            }
        }
    }

    /// Process audio-thread notifications, emitting `ItemDidPlayToEnd`
    /// only when a track finishes via natural EOF.
    fn process_notifications(&self) {
        for slot_id in self.core.engine.active_slots() {
            let mut saw_slot = false;
            while let Some(notification) = self.core.engine.pop_slot_notification(slot_id) {
                saw_slot = true;
                tracing::debug!(?notification, "process_notifications: handle");
                if let PlayerNotification::Unloaded { src } = &notification {
                    self.core.engine.unbind_slot_seek(slot_id, src);
                }
                self.dispatch_notification(slot_id, notification);
            }
            if !self.core.engine.drain_slot_trash(slot_id) && !saw_slot {
                tracing::warn!(?slot_id, "process_notifications: slot has no control state");
            }
        }
    }

    fn publish_current_track_snapshot(&self, duration_seconds: f64) {
        let Some(slot_id) = self.slot() else {
            return;
        };
        let Some(playback) = self.core.engine.slot_playback(slot_id) else {
            return;
        };
        playback.position.store(0.0, Ordering::Relaxed);
        playback
            .duration
            .store(duration_seconds.max(0.0), Ordering::Relaxed);
    }
}

impl PlayerRuntime {
    pub fn process_notifications(&self) {
        Notifier::new(self).process_notifications();
    }

    pub(crate) fn publish_current_track_snapshot(&self, duration_seconds: f64) {
        Notifier::new(self).publish_current_track_snapshot(duration_seconds);
    }
}

/// `from_current_item` answers whether the stopping slot is the one the
/// phase currently holds.
pub(crate) fn player_event_from_notification(
    notification: PlayerNotification,
    from_current_item: bool,
) -> Option<PlayerEvent> {
    match notification {
        PlayerNotification::PlaybackStopped {
            reason: TrackPlaybackStopReason::Eof,
            src,
            item_id,
            ..
        } => Some(PlayerEvent::ItemDidPlayToEnd {
            src,
            item_id,
            from_current_item,
        }),
        PlayerNotification::PlaybackStopped {
            reason: TrackPlaybackStopReason::Failed,
            src,
            item_id,
            ..
        } => Some(PlayerEvent::ItemDidFail {
            src,
            item_id,
            from_current_item,
        }),
        PlayerNotification::RateChanged { rate } => Some(PlayerEvent::RateChanged { rate }),
        _ => None,
    }
}

fn player_events_from_notification(
    player: &PlayerRuntime,
    notification: &PlayerNotification,
) -> Vec<kithara_events::Event> {
    let mut events = Vec::new();
    match notification {
        PlayerNotification::PlaybackStarted { src, item_id } => {
            events.push(
                PlayerEvent::PlaybackStarted {
                    src: Arc::clone(src),
                    item_id: item_id.clone(),
                }
                .into(),
            );
        }
        PlayerNotification::PlaybackStopped {
            reason: TrackPlaybackStopReason::Stop,
            ..
        } => {
            let phase = player.phase.lock();
            if phase
                .pending()
                .is_some_and(|pending| pending.state.activated())
                && let Some(slot) = phase.slot()
            {
                events.push(
                    EngineEvent::CrossfadeCompleted {
                        from: slot,
                        to: slot,
                    }
                    .into(),
                );
            }
        }
        _ => {}
    }
    events
}

#[cfg(test)]
mod tests {
    use kithara_bufpool::{BytePool, PcmPool};
    use kithara_events::{Envelope, Event, EventReceiver};
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig,
        player::{PlayerConfig, PlayerImpl},
        session::testing,
    };

    /// A started player holding one slot — the slot the phase calls current.
    fn player_with_slot() -> (PlayerImpl, SlotId) {
        let worker = PlayWorker::new(
            PlayWorkerConfig::for_pools(BytePool::default(), PcmPool::default()).build(),
        );
        let player = PlayerImpl::new(
            PlayerConfig::builder()
                .worker(worker)
                .session(testing::test_session())
                .build(),
        );
        player
            .ensure_engine_started()
            .expect("engine start must succeed");
        let slot = player.ensure_slot().expect("slot allocation must succeed");
        (player, slot)
    }

    fn eof_notification() -> PlayerNotification {
        PlayerNotification::PlaybackStopped {
            src: Arc::from("track.mp3"),
            item_id: None,
            reason: TrackPlaybackStopReason::Eof,
            seek_epoch: 0,
        }
    }

    fn published_end_flag(rx: &mut EventReceiver) -> Option<bool> {
        while let Ok(Envelope { event, .. }) = rx.try_recv() {
            if let Event::Player(PlayerEvent::ItemDidPlayToEnd {
                from_current_item, ..
            }) = event
            {
                return Some(from_current_item);
            }
        }
        None
    }

    #[kithara::test]
    fn eof_playback_stopped_notification_maps_to_item_end_event() {
        let event = player_event_from_notification(eof_notification(), true);
        assert!(matches!(
            event,
            Some(PlayerEvent::ItemDidPlayToEnd {
                from_current_item: true,
                ..
            })
        ));
    }

    #[kithara::test]
    fn failed_playback_stopped_notification_carries_the_current_item_answer() {
        let event = player_event_from_notification(
            PlayerNotification::PlaybackStopped {
                src: Arc::from("track.mp3"),
                item_id: None,
                reason: TrackPlaybackStopReason::Failed,
                seek_epoch: 0,
            },
            false,
        );
        assert!(matches!(
            event,
            Some(PlayerEvent::ItemDidFail {
                from_current_item: false,
                ..
            })
        ));
    }

    #[kithara::test]
    fn playback_stopped_notification_does_not_map_to_item_end_event() {
        let event = player_event_from_notification(
            PlayerNotification::PlaybackStopped {
                src: Arc::from("track.mp3"),
                item_id: None,
                reason: TrackPlaybackStopReason::Stop,
                seek_epoch: 0,
            },
            true,
        );
        assert!(event.is_none());
    }

    #[kithara::test]
    fn end_of_the_held_slot_is_reported_as_the_current_item() {
        let (player, slot) = player_with_slot();
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(slot, eof_notification());

        assert_eq!(
            published_end_flag(&mut rx),
            Some(true),
            "the slot the phase holds is the item the listener hears"
        );
    }

    /// `process_notifications` drains every active slot. An end minted by a
    /// slot the phase does not hold — a preloaded successor, or a
    /// predecessor still decoding after a switch — must not be reported as
    /// the current item, or the queue advances off a track still playing.
    #[kithara::test]
    fn end_of_a_background_slot_is_not_reported_as_the_current_item() {
        let (player, slot) = player_with_slot();
        let background = SlotId::new(slot.value() + 1);
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(background, eof_notification());

        assert_eq!(
            published_end_flag(&mut rx),
            Some(false),
            "an end from a slot the phase does not hold is not the current item"
        );
    }
}
