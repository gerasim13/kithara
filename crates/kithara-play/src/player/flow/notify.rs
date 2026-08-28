use std::{ops::Deref, sync::atomic::Ordering};

use kithara_platform::sync::Arc;

use super::super::core::PlayerImpl;
use crate::{
    api::{EngineEvent, ItemRole, PlayerEvent, SlotId},
    bridge::{PlayerNotification, TrackPlaybackStopReason},
};

struct Notifier<'a> {
    player: &'a PlayerImpl,
}

impl<'a> Notifier<'a> {
    const fn new(player: &'a PlayerImpl) -> Self {
        Self { player }
    }
}

impl Deref for Notifier<'_> {
    type Target = PlayerImpl;

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
        let item = self.item_role(slot_id, &notification);
        let emitted = player_events_from_notification(self, &notification, &item);
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
                self.handle_track_playback_stopped(item, notification);
            }
            _ => {
                if let Some(event) = player_event_from_notification(notification.clone(), item) {
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

    /// Name the item a start or stop notification is about, together with
    /// its role in the arena.
    ///
    /// Slot identity answers only half of the role: a slot is a processor
    /// holding an arena, and `commit_next` promotes the successor *inside*
    /// the current slot, leaving it in the phase as the activated
    /// `PendingNext`. So a stop from the held slot naming a src other than
    /// the promoted one is the item promoted over — it ends inside the
    /// leading slot, but it is not what the listener is hearing.
    fn item_role(&self, slot_id: SlotId, notification: &PlayerNotification) -> ItemRole {
        let id = notification.item_id().map(Arc::clone);
        if self.slot() != Some(slot_id) {
            return ItemRole::Background { id };
        }
        let promoted = self
            .phase
            .lock()
            .pending()
            .filter(|pending| pending.state.activated())
            .map(|pending| Arc::clone(&pending.src));
        match (promoted, notification.src()) {
            (Some(promoted), Some(src)) if promoted != *src => ItemRole::Outgoing { id },
            _ => ItemRole::Leading { id },
        }
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

    /// The role is read before `finalize_handover_if_armed` runs, so the
    /// phase still describes the arena as it was when the track stopped.
    fn handle_track_playback_stopped(&self, item: ItemRole, notification: PlayerNotification) {
        if let Some(event) = player_event_from_notification(notification, item) {
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

impl PlayerImpl {
    pub fn process_notifications(&self) {
        Notifier::new(self).process_notifications();
    }

    pub(crate) fn publish_current_track_snapshot(&self, duration_seconds: f64) {
        Notifier::new(self).publish_current_track_snapshot(duration_seconds);
    }
}

pub(crate) fn player_event_from_notification(
    notification: PlayerNotification,
    item: ItemRole,
) -> Option<PlayerEvent> {
    match notification {
        PlayerNotification::PlaybackStopped {
            reason: TrackPlaybackStopReason::Eof,
            src,
            ..
        } => Some(PlayerEvent::ItemDidPlayToEnd { src, item }),
        PlayerNotification::PlaybackStopped {
            reason: TrackPlaybackStopReason::Failed,
            src,
            ..
        } => Some(PlayerEvent::ItemDidFail { src, item }),
        _ => None,
    }
}

fn player_events_from_notification(
    player: &PlayerImpl,
    notification: &PlayerNotification,
    item: &ItemRole,
) -> Vec<kithara_events::Event> {
    let mut events = Vec::new();
    match notification {
        PlayerNotification::PlaybackStarted { src, .. } => {
            events.push(
                PlayerEvent::PlaybackStarted {
                    src: Arc::clone(src),
                    item: item.clone(),
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
    use kithara_events::{Envelope, Event, EventReceiver};
    use kithara_platform::sync::Arc;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        player::{
            PlayerConfig,
            state::{PendingNext, PendingNextState},
        },
        session::testing,
    };

    /// A started player holding one slot — the slot the phase calls current.
    fn player_with_slot() -> (PlayerImpl, SlotId) {
        let player = PlayerImpl::new(
            PlayerConfig::test_builder()
                .session(testing::test_session())
                .build(),
        );
        player
            .ensure_engine_started()
            .expect("engine start must succeed");
        let slot = player.ensure_slot().expect("slot allocation must succeed");
        (player, slot)
    }

    /// Put the slot mid-crossfade: the successor is loaded into this same
    /// slot's arena and already promoted over the outgoing track, exactly
    /// as `commit_next` leaves it.
    fn activate_pending(player: &PlayerImpl, src: &str) {
        let mut phase = player.phase.lock();
        let Some(pending) = phase.pending_mut() else {
            panic!("BUG: an active phase must carry a pending slot");
        };
        *pending = Some(PendingNext {
            src: Arc::from(src),
            state: PendingNextState::ActivatedReady,
            index: 1,
            duration_seconds: 60.0,
        });
    }

    fn stop_notification(src: &str, reason: TrackPlaybackStopReason) -> PlayerNotification {
        PlayerNotification::PlaybackStopped {
            src: Arc::from(src),
            item_id: None,
            reason,
            seek_epoch: 0,
        }
    }

    fn leading() -> ItemRole {
        ItemRole::Leading { id: None }
    }

    fn eof_notification() -> PlayerNotification {
        stop_notification("leading.mp3", TrackPlaybackStopReason::Eof)
    }

    fn published_start_role(rx: &mut EventReceiver) -> Option<ItemRole> {
        while let Ok(Envelope { event, .. }) = rx.try_recv() {
            if let Event::Player(PlayerEvent::PlaybackStarted { item, .. }) = event {
                return Some(item);
            }
        }
        None
    }

    fn published_end_role(rx: &mut EventReceiver) -> Option<ItemRole> {
        while let Ok(Envelope { event, .. }) = rx.try_recv() {
            if let Event::Player(PlayerEvent::ItemDidPlayToEnd { item, .. }) = event {
                return Some(item);
            }
        }
        None
    }

    #[kithara::test]
    fn eof_playback_stopped_notification_maps_to_item_end_event() {
        let event = player_event_from_notification(eof_notification(), leading());
        assert!(matches!(
            event,
            Some(PlayerEvent::ItemDidPlayToEnd {
                item: ItemRole::Leading { .. },
                ..
            })
        ));
    }

    #[kithara::test]
    fn failed_playback_stopped_notification_carries_the_role() {
        let event = player_event_from_notification(
            stop_notification("leading.mp3", TrackPlaybackStopReason::Failed),
            ItemRole::Background { id: None },
        );
        assert!(matches!(
            event,
            Some(PlayerEvent::ItemDidFail {
                item: ItemRole::Background { .. },
                ..
            })
        ));
    }

    #[kithara::test]
    fn playback_stopped_notification_does_not_map_to_item_end_event() {
        let event = player_event_from_notification(
            stop_notification("leading.mp3", TrackPlaybackStopReason::Stop),
            leading(),
        );
        assert!(event.is_none());
    }

    #[kithara::test]
    fn end_of_the_held_slot_is_the_leading_track() {
        let (player, slot) = player_with_slot();
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(slot, eof_notification());

        assert_eq!(
            published_end_role(&mut rx),
            Some(leading()),
            "with nothing promoted over it, the held slot's track is the one being heard"
        );
    }

    /// `process_notifications` drains every active slot. A slot the phase
    /// no longer holds is an orphan still emptying its notification ring
    /// while a different track plays; acting on its end cuts that track.
    #[kithara::test]
    fn end_of_a_slot_the_phase_does_not_hold_is_a_background_track() {
        let (player, slot) = player_with_slot();
        let background = SlotId::new(slot.value() + 1);
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(background, eof_notification());

        assert_eq!(
            published_end_role(&mut rx),
            Some(ItemRole::Background { id: None }),
            "an end from a slot the phase does not hold is not the track being heard"
        );
    }

    /// A slot is a processor holding an arena, not one track: `commit_next`
    /// promotes the successor *inside* the current slot. So the outgoing
    /// half of a crossfade ends while its own slot is still the held one —
    /// slot identity alone calls it leading, and the queue would advance a
    /// second time off an end that was already accounted for.
    #[kithara::test]
    fn the_outgoing_half_of_a_crossfade_is_not_the_leading_track() {
        let (player, slot) = player_with_slot();
        activate_pending(&player, "leading.mp3");
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(
            slot,
            stop_notification("outgoing.mp3", TrackPlaybackStopReason::Eof),
        );

        assert_eq!(
            published_end_role(&mut rx),
            Some(ItemRole::Outgoing { id: None }),
            "the track promoted over is not the one the listener is hearing"
        );
    }

    /// The other half of the same moment: the promoted track is the one
    /// being heard, so its own end must still drive the advance.
    #[kithara::test]
    fn the_promoted_half_of_a_crossfade_is_the_leading_track() {
        let (player, slot) = player_with_slot();
        activate_pending(&player, "leading.mp3");
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(slot, eof_notification());

        assert_eq!(
            published_end_role(&mut rx),
            Some(leading()),
            "the promoted track is the one being heard"
        );
    }

    /// A start is drained from every active slot exactly as a stop is, so
    /// it needs the same answer: an orphan whose item reaches `Playing`
    /// would otherwise announce itself as the item being heard.
    #[kithara::test]
    fn start_of_a_slot_the_phase_does_not_hold_is_a_background_item() {
        let (player, slot) = player_with_slot();
        let background = SlotId::new(slot.value() + 1);
        let mut rx = player.subscribe();

        Notifier::new(&player).dispatch_notification(
            background,
            PlayerNotification::PlaybackStarted {
                src: Arc::from("background.mp3"),
                item_id: Some(Arc::from("bg-7")),
            },
        );

        assert_eq!(
            published_start_role(&mut rx),
            Some(ItemRole::Background {
                id: Some(Arc::from("bg-7"))
            }),
            "a start from a slot the phase does not hold is not the item being heard"
        );
    }
}
