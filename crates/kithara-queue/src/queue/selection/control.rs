use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::{PlayError, SelectTransition, SelectionPlayback};

use super::super::{
    Queue,
    types::{PendingSelect, Transition},
};
use crate::{
    error::QueueError,
    event::{AdvanceReason, QueueEvent, TrackStatus},
    loading::LoadClass,
};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Admission is the cursor's commit: the queue stands on the successor from
    /// the moment its selection is accepted, even while the track is still
    /// loading. Reading a successor never moves the cursor — a read that
    /// committed would strand it on a track that never started, and the next
    /// end-of-item would name a track the cursor had already left.
    pub(in crate::queue) fn commit_navigation_to(&mut self, id: TrackId) {
        let ids = self.track_ids();
        self.navigation.select(id, &ids);
    }

    pub(crate) fn select(&mut self, id: TrackId, transition: Transition) -> Result<(), QueueError> {
        self.with_open_result(|queue| {
            queue.select_with(
                id,
                transition,
                AdvanceReason::UserSelect,
                SelectionPlayback::Play,
            )
        })
    }

    /// Hand the loaded track `id` to the deck: its resource when the queue
    /// still holds it, or nothing when the deck already does (an armed
    /// successor, or the track it plays). A refused select has spent the
    /// resource, so the track is consumed and reloads once it is wanted again.
    pub(in crate::queue) fn select_loaded_item(
        &mut self,
        id: TrackId,
        crossfade: kithara_play::CrossfadeSettings,
        reason: AdvanceReason,
        playback: SelectionPlayback,
    ) -> Result<(), QueueError> {
        let was_playing = self.player.is_playing();
        if was_playing && crossfade.duration > 0.0 {
            self.announce(QueueEvent::CrossfadeStarted {
                settings: crossfade,
            });
        }
        let resource = self.tracks.take_resource(id);
        if let Err(error) = self.player.select_with_crossfade(
            id,
            resource,
            SelectTransition {
                playback,
                crossfade,
            },
        ) {
            self.tracks.set_status(id, TrackStatus::Consumed);
            return Err(error.into());
        }
        self.commit_navigation_to(id);
        self.announce(QueueEvent::CurrentTrackAdvance {
            reason,
            id: Some(id),
        });
        self.tracks.set_status(id, TrackStatus::Consumed);
        Ok(())
    }

    pub(in crate::queue) fn select_with(
        &mut self,
        id: TrackId,
        transition: Transition,
        reason: AdvanceReason,
        playback: SelectionPlayback,
    ) -> Result<(), QueueError> {
        if matches!(
            reason,
            AdvanceReason::UserSelect
                | AdvanceReason::UserNext
                | AdvanceReason::UserPrev
                | AdvanceReason::RemovedCurrent
        ) {
            self.autoplay_target = None;
        }
        let default = self.config.crossfade_settings();
        let settings = transition
            .settings(default)
            .validate()
            .map_err(PlayError::from)?;
        self.select_with_reason_locked(id, settings, reason, playback)
    }

    pub(in crate::queue) fn select_with_reason(
        &mut self,
        id: TrackId,
        transition: Transition,
        reason: AdvanceReason,
    ) -> Result<(), QueueError> {
        let playback = if matches!(
            reason,
            AdvanceReason::NaturalEof
                | AdvanceReason::TrackFailed
                | AdvanceReason::CrossfadePreArm
                | AdvanceReason::Repeat
        ) || self.player.is_playing()
        {
            SelectionPlayback::Play
        } else {
            SelectionPlayback::Pause
        };
        self.select_with(id, transition, reason, playback)
    }

    /// `is_playing` is a session flag, not a verdict on the current item: the render thread queues
    /// the natural end but clears the flag only at the next `process`, so a repeat-one advance must
    /// re-select the item that just ended despite the flag.
    pub(in crate::queue) fn select_with_reason_locked(
        &mut self,
        id: TrackId,
        settings: kithara_play::CrossfadeSettings,
        reason: AdvanceReason,
        playback: SelectionPlayback,
    ) -> Result<(), QueueError> {
        let status = self
            .track(id)
            .map(|entry| entry.status)
            .ok_or(QueueError::UnknownTrackId(id))?;
        if self.player.current_item() == Some(id)
            && matches!(status, TrackStatus::Consumed)
            && matches!(
                reason,
                AdvanceReason::UserSelect
                    | AdvanceReason::UserNext
                    | AdvanceReason::UserPrev
                    | AdvanceReason::RemovedCurrent
                    | AdvanceReason::NaturalEof
            )
        {
            self.cancel_stale_pending(id);
            if self.player.is_playing() && reason != AdvanceReason::NaturalEof {
                return Ok(());
            }
            let finished = self.player.playback_snapshot().is_some_and(|snapshot| {
                snapshot.duration() > 0.0 && snapshot.position() >= snapshot.duration()
            });
            if reason == AdvanceReason::NaturalEof || finished {
                self.player.seek_seconds(0.0)?;
            }
            if playback == SelectionPlayback::Play {
                self.player.play();
            } else {
                self.player.pause();
            }
            self.commit_navigation_to(id);
            self.announce(QueueEvent::CurrentTrackAdvance {
                reason,
                id: Some(id),
            });
            return Ok(());
        }

        if let Some(armed) = self.player.armed_next()
            && armed != id
        {
            self.disarm_successor(armed);
        }

        match status {
            TrackStatus::Loaded => {
                self.cancel_stale_pending(id);
                self.select_loaded_item(id, settings, reason, playback)?;
                Ok(())
            }
            TrackStatus::Pending | TrackStatus::Loading | TrackStatus::Slow => {
                self.override_pending_select(PendingSelect {
                    reason,
                    settings,
                    playback,
                    id,
                });
                self.promote_pending_load(id);
                Ok(())
            }
            TrackStatus::Cancelled | TrackStatus::Consumed | TrackStatus::Failed(_) => {
                let source = self.tracks.source(id).ok_or(QueueError::NotReady(id))?;
                self.override_pending_select(PendingSelect {
                    reason,
                    settings,
                    playback,
                    id,
                });
                self.tracks.set_status(id, TrackStatus::Pending);
                self.spawn_apply_after_load(id, source, LoadClass::Interactive);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{super::super::types::SelectPhase, *};
    use crate::{
        event::QueueEvent,
        queue::state::tests::{make_queue, wait_for_queue_event},
    };

    fn append(queue: &mut Queue<crate::test_pools::TestPools>, source: &str) -> TrackId {
        queue
            .append(source)
            .expect("BUG: open queue must accept a track")
    }

    #[kithara::test(tokio)]
    async fn select_unknown_id_errors() {
        let (mut queue, _audio_thread) = make_queue();
        let err = queue
            .select(TrackId(999), Transition::None)
            .expect_err("unknown id should error");
        assert!(matches!(err, QueueError::UnknownTrackId(_)));
    }

    #[kithara::test(tokio)]
    async fn select_pending_track_stashes_pending_select() {
        let (mut queue, _audio_thread) = make_queue();
        let id = append(&mut queue, "https://example.com/a.mp3");
        let _ = queue.select(id, Transition::None);
        let phase = queue.pending_select;
        match phase {
            SelectPhase::Pending(pending) => {
                assert_eq!(pending.id, id);
                assert_eq!(pending.settings.duration, 0.0);
            }
            SelectPhase::Idle => panic!("BUG: select stashes pending entry"),
        }
    }

    #[kithara::test(tokio)]
    async fn advance_to_next_on_empty_emits_queue_ended() {
        let (mut queue, _audio_thread) = make_queue();
        let mut rx = queue.subscribe();
        assert!(
            queue
                .advance_to_next_inner(Transition::Crossfade, AdvanceReason::NaturalEof)
                .expect("BUG: open queue advance must be admitted")
                .is_none()
        );
        queue.publish();
        let saw_ended =
            wait_for_queue_event(&mut rx, |ev| matches!(ev, QueueEvent::QueueEnded), 200).await;
        assert!(saw_ended);
    }

    #[kithara::test(tokio)]
    async fn manual_next_at_exhaustion_does_not_emit_queue_ended() {
        let (mut queue, _audio_thread) = make_queue();
        let mut rx = queue.subscribe();
        assert_eq!(queue.next(Transition::None).expect("manual next"), None);
        assert!(
            !wait_for_queue_event(&mut rx, |ev| matches!(ev, QueueEvent::QueueEnded), 50).await
        );
    }

    #[kithara::test(tokio)]
    async fn advance_to_next_cycles_then_emits_queue_ended() {
        let (mut queue, _audio_thread) = make_queue();
        let a = append(&mut queue, "https://example.com/a.mp3");
        let b = append(&mut queue, "https://example.com/b.mp3");
        queue.navigation.select(b, &[a, b]);
        let mut rx = queue.subscribe();

        assert!(
            queue
                .advance_to_next_inner(Transition::Crossfade, AdvanceReason::NaturalEof)
                .expect("BUG: open queue advance must be admitted")
                .is_none()
        );
        queue.publish();

        let saw_ended =
            wait_for_queue_event(&mut rx, |ev| matches!(ev, QueueEvent::QueueEnded), 400).await;
        assert!(saw_ended, "QueueEnded should be broadcast at end-of-queue");
    }

    #[kithara::test(tokio)]
    async fn admitted_pending_successor_becomes_navigation_authority() {
        let (mut queue, _audio_thread) = make_queue();
        let first = append(&mut queue, "https://example.com/a.mp3");
        let second = append(&mut queue, "https://example.com/b.mp3");
        queue.navigation.select(first, &[first, second]);
        queue.tracks.set_status(first, TrackStatus::Consumed);
        queue.tracks.set_status(second, TrackStatus::Pending);

        assert_eq!(
            queue
                .advance_to_next_inner(Transition::Crossfade, AdvanceReason::NaturalEof)
                .expect("BUG: open queue advance must be admitted"),
            Some(second)
        );
        assert_eq!(
            queue.navigation.current(),
            Some(second),
            "admitted automatic successor must remain authoritative while loading"
        );
        let SelectPhase::Pending(pending) = queue.pending_select else {
            panic!("successor selection must remain pending")
        };
        assert_eq!(pending.playback, SelectionPlayback::Play);
        assert_eq!(pending.reason, AdvanceReason::NaturalEof);
    }

    #[kithara::test(tokio)]
    async fn pending_override_latches_profile_without_mutating_default() {
        let (mut queue, _audio_thread) = make_queue();
        let id = append(&mut queue, "https://example.com/a.mp3");
        let configured = kithara_play::CrossfadeSettings::new(
            2.0,
            kithara_play::CrossfadeCurve::EqualPower,
            1.0,
            0.5,
        )
        .expect("valid settings");
        let override_settings = kithara_play::CrossfadeSettings::new(
            4.0,
            kithara_play::CrossfadeCurve::Linear,
            0.25,
            0.3,
        )
        .expect("valid settings");
        queue
            .set_crossfade_settings(configured)
            .expect("valid settings");
        queue
            .select(
                id,
                Transition::CrossfadeWith {
                    settings: override_settings,
                },
            )
            .expect("pending selection admitted");
        queue
            .set_crossfade_settings(kithara_play::CrossfadeSettings::default())
            .expect("valid settings");

        let SelectPhase::Pending(pending) = queue.pending_select else {
            panic!("selection must remain pending")
        };
        assert_eq!(pending.settings, override_settings);
        assert_eq!(
            queue.crossfade_settings(),
            kithara_play::CrossfadeSettings::default()
        );
    }
}
