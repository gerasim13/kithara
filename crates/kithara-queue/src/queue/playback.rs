use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::{
    InterruptionKind, PlayError, SeekOutcome, SelectionPlayback, SessionDuckingMode, SuccessorLink,
};
use smallvec::SmallVec;
use tracing::debug;

use super::{
    QueueControl,
    types::{CachedPosition, PendingSelect, PlaybackView, SelectPhase, Transition},
};
use crate::{
    ActionAtItemEnd,
    attempts::LoadClass,
    error::QueueError,
    event::{AdvanceReason, TrackStatus},
    track::TrackEntry,
};

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn freeze_cached_position(&self) {
        if let Some(t) = self.player.position_seconds() {
            self.write_cached_position(CachedPosition::known(t));
        }
    }

    /// Whether the user has paused playback.
    ///
    /// Reads the Player's explicit paused phase, not its effective rate or
    /// live output state: both become inactive at natural EOF without turning
    /// that EOF into a user pause.
    pub(super) fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    /// Where the current track is, in media seconds, and the rate it plays
    /// at; `None` while paused or before it has a position and a duration.
    fn playback_time(&self) -> Option<super::types::PlaybackTime> {
        if self.is_paused() {
            return None;
        }
        let view = self.playback_view();
        Some(super::types::PlaybackTime {
            dur: view.duration?,
            pos: view.position?,
            rate: f64::from(self.player.rate()),
        })
    }

    /// The successor the queue wants armed behind the current track: the one
    /// navigation offers at its natural end, while the queue advances, no
    /// selection waits for its load, and the cursor stands on the track the
    /// deck plays. Repeating the current track needs no successor.
    fn wanted_successor(&self) -> Option<(usize, TrackEntry)> {
        if self.action_at_item_end() != ActionAtItemEnd::Advance
            || matches!(*self.lock_pending_select_mut(), SelectPhase::Pending(_))
        {
            return None;
        }
        let current = self.current()?;
        let playing = self
            .lock_tracks()
            .get(self.player.current_index())
            .map(|entry| entry.id);
        if playing != Some(current.id) {
            return None;
        }
        let next = self.next_selectable_entry(AdvanceReason::NaturalEof)?;
        if next.id == current.id {
            return None;
        }
        let index = self
            .lock_tracks()
            .iter()
            .position(|entry| entry.id == next.id)?;
        Some((index, next))
    }

    /// Keep the deck's armed successor the one the queue wants. Once the
    /// current track ends within the longer of the prefetch lead and the
    /// crossfade, in session time, a consumed successor reloads and a loaded
    /// one is armed; a crossfade successor is committed once the track ends
    /// within the fade, so the two overlap. A gapless one needs no commit:
    /// the processor plays it on the frame after the current track's last.
    pub(super) fn reconcile_successor(&self) {
        let settings = self.crossfade_settings();
        let lead = self.config.prefetch_duration.max(settings.duration);
        let time = self.playback_time().filter(|time| time.ends_within(lead));
        let armed = self.player.armed_next();
        if armed.is_none() && time.is_none() {
            return;
        }
        let wanted = self.wanted_successor();
        if let Some(armed) = armed
            && wanted.as_ref().is_none_or(|(index, _)| *index != armed)
        {
            self.disarm_successor(armed);
        }
        let (Some(time), Some((index, next))) = (time, wanted) else {
            return;
        };
        match next.status {
            TrackStatus::Consumed => {
                let Some(source) = self.tracks.source(next.id) else {
                    return;
                };
                self.set_status(next.id, TrackStatus::Pending);
                self.spawn_apply_after_load(next.id, source, LoadClass::Prefetch);
            }
            TrackStatus::Loaded => {
                if armed != Some(index) && !self.arm_successor(index, next.id, settings.link()) {
                    return;
                }
                if settings.link() == SuccessorLink::Fade && time.ends_within(settings.duration) {
                    self.cross_fade_into(next.id);
                }
            }
            TrackStatus::Pending
            | TrackStatus::Loading
            | TrackStatus::Slow
            | TrackStatus::Cancelled
            | TrackStatus::Failed(_) => {}
        }
    }

    /// Arm the loaded successor at `index`; `false` when the deck did not.
    fn arm_successor(&self, index: usize, id: TrackId, link: SuccessorLink) -> bool {
        match self.player.arm_next(index, link) {
            Ok(Some(_)) => true,
            Ok(None) => {
                debug!(
                    id = id.as_u64(),
                    index, "the successor has no resource on the deck to arm"
                );
                false
            }
            Err(error) => {
                debug!(%error, id = id.as_u64(), index, "the successor would not arm");
                false
            }
        }
    }

    fn cross_fade_into(&self, id: TrackId) {
        if let Err(error) =
            self.select_with_reason(id, Transition::Crossfade, AdvanceReason::CrossfadePreArm)
        {
            debug!(%error, id = id.as_u64(), "the armed successor would not cross-fade in");
        }
    }

    /// Take the armed successor at `index` off the deck. Arming moved its
    /// resource onto the deck, so the entry is consumed and reloads once it
    /// is wanted again.
    pub(super) fn disarm_successor(&self, index: usize) {
        self.player.unarm_next();
        let id = self.lock_tracks().get(index).map(|entry| entry.id);
        if let Some(id) = id {
            self.set_status(id, TrackStatus::Consumed);
        }
    }

    /// Platform audio-route changed while playback may be active.
    ///
    /// Recreates the native output stream below the queue without
    /// changing queue state, current item, or track loading.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when the underlying player cannot restart
    /// the active audio route.
    pub fn notify_audio_route_changed(&self, reason: &str) -> Result<(), QueueError> {
        self.with_open_result(|queue| queue.player.invalidate_audio_route(reason))?;
        Ok(())
    }

    /// The platform interrupted, or released, the audio output.
    ///
    /// Recording the fact is all this does: an interruption leaves the native
    /// output unscheduled, and restoring it is the route-invalidation path.
    pub fn notify_interruption(&self, kind: InterruptionKind) {
        self.command(|queue| queue.player.notify_interruption(kind));
    }

    /// Pause playback and freeze the queue-visible head position.
    pub fn pause(&self) {
        self.command(Self::pause_inner);
    }

    /// [`Self::pause`] inside the admission its caller holds, as the tick
    /// does when a drained end pauses the queue.
    pub(super) fn pause_inner(&self) {
        self.player.pause();
        let mut phase = self.lock_pending_select_mut();
        if let SelectPhase::Pending(mut pending) = *phase {
            pending.playback = SelectionPlayback::Pause;
            *phase = SelectPhase::Pending(pending);
        }
        drop(phase);
        self.freeze_cached_position();
    }

    /// Starts playback, marking a consumed slot or retaining the selection until loading finishes.
    /// Reconciliation is serialized with load completion.
    pub fn play(&self) {
        self.command(Self::play_inner);
    }

    fn play_inner(&self) {
        let mut phase = self.lock_pending_select_mut();
        if let SelectPhase::Pending(mut pending) = *phase {
            pending.playback = SelectionPlayback::Play;
            *phase = SelectPhase::Pending(pending);
        }
        drop(phase);
        self.player.play();

        let _apply = self.lock_select_apply();
        let pending = match *self.lock_pending_select_mut() {
            SelectPhase::Pending(pending) => Some(pending),
            SelectPhase::Idle => None,
        };
        let index = self.player.current_index();
        if pending.is_none() && self.player.item_has_resource(index) {
            return;
        }
        let current = {
            let guard = self.lock_tracks();
            pending
                .map_or_else(
                    || guard.get(index),
                    |pending| guard.iter().find(|entry| entry.id == pending.id),
                )
                .map(|entry| (entry.id, entry.status.clone()))
        };
        let Some((id, status)) = current else {
            return;
        };
        match status {
            TrackStatus::Loaded => self.set_status(id, TrackStatus::Consumed),
            TrackStatus::Pending | TrackStatus::Loading | TrackStatus::Slow => {
                self.override_pending_select(pending.unwrap_or_else(|| PendingSelect {
                    id,
                    settings: Transition::None.settings(self.crossfade_settings()),
                    playback: SelectionPlayback::Play,
                    reason: AdvanceReason::UserSelect,
                }));
                self.promote_pending_load(id);
            }
            TrackStatus::Failed(_) => {
                let Some(source) = self.tracks.source(id) else {
                    return;
                };
                self.override_pending_select(PendingSelect {
                    id,
                    settings: Transition::None.settings(self.crossfade_settings()),
                    playback: SelectionPlayback::Play,
                    reason: AdvanceReason::UserSelect,
                });
                self.set_status(id, TrackStatus::Pending);
                self.spawn_apply_after_load(id, source, LoadClass::Interactive);
            }
            TrackStatus::Consumed | TrackStatus::Cancelled => {}
        }
    }

    /// Single coherent read of the player's live playback state.
    ///
    /// Pollers (the FFI time thread, `snapshot`) get position, duration,
    /// decoded frontier, and the playing flag from one call instead of
    /// several separate accessors. The player-sourced fields come from one
    /// [`PlaybackSnapshot`](kithara_play::PlaybackSnapshot) via its `From`
    /// conversion; `position` is then replaced with this queue's cached,
    /// 0.0-smoothed value.
    #[must_use]
    pub fn playback_view(&self) -> PlaybackView {
        let mut view = self
            .player
            .playback_snapshot()
            .map(PlaybackView::from)
            .unwrap_or_default();
        view.position = self.position_seconds();
        view
    }

    pub(super) fn seek_player(&self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        self.with_open_result(|queue| queue.seek_player_inner(seconds))
    }

    /// Resumes seeking after the last track plays to natural EOF and the navigation cursor runs off
    /// the end, leaving `current()` at `None`.
    fn seek_player_inner(&self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        if self.current().is_none() {
            let id = { self.lock_navigation().last_selected() };
            if let Some(id) = id {
                let ids = self
                    .tracks()
                    .into_iter()
                    .map(|track| track.id)
                    .collect::<SmallVec<[_; 16]>>();
                self.lock_navigation_mut().select(id, &ids);
                self.handle_current_item_changed();
            }
        }
        let outcome = self.player.seek_seconds(seconds)?;
        if let SeekOutcome::Landed { landed_at, .. } = outcome {
            self.write_cached_position(CachedPosition::known(landed_at.as_secs_f64()));
        }
        Ok(outcome)
    }

    /// Lower or restore the whole session output under a competing sound,
    /// such as a call or a navigation prompt.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when the session rejects the change.
    pub fn set_session_ducking(&self, mode: SessionDuckingMode) -> Result<(), QueueError> {
        self.with_open_result(|queue| queue.player.set_session_ducking(mode))?;
        Ok(())
    }

    pub(super) fn tick_player(&self) -> Result<(), PlayError> {
        self.with_open_result(Self::tick_player_inner)
    }

    fn tick_player_inner(&self) -> Result<(), PlayError> {
        self.player.tick()?;
        self.player.process_notifications();
        self.drain_player_events();
        self.update_cached_position();
        self.reconcile_successor();
        Ok(())
    }

    fn update_cached_position(&self) {
        /// Minimum position threshold used to suppress spurious 0.0 reports
        /// on pause/resume. Values above this are considered a valid
        /// non-zero position.
        const MIN_STABLE_POSITION_SECS: f64 = 0.5;

        if self.is_paused() {
            return;
        }

        let Some(t) = self.player.position_seconds() else {
            return;
        };
        let prev = Option::<f64>::from(self.read_cached_position());
        if t == 0.0 && prev.is_some_and(|p| p > MIN_STABLE_POSITION_SECS) {
            return;
        }
        self.write_cached_position(CachedPosition::known(t));
    }

    delegate::delegate! {
        to self {
            /// Latest monotonic playback position for the current track in
            /// seconds. Updated on every [`Self::tick`]; skips transient 0.0
            /// samples the engine produces on pause/resume so downstream UIs
            /// see stable values.
            #[must_use]
            #[into]
            #[call(read_cached_position)]
            pub fn position_seconds(&self) -> Option<f64>;

            /// Seek within the currently-playing track.
            ///
            /// Seek-hang detection is not handled here: the audio pipeline's
            /// own `#[hang_watchdog]` instrumentation (e.g. `Audio::read`,
            /// `Stream::read`, `decode_next_chunk`) already panics with a
            /// stacktrace and context dump when no progress is observed. Adding
            /// a second Queue-level watchdog would just duplicate those panics.
            ///
            /// Returns the typed [`SeekOutcome`](kithara_play::SeekOutcome) — either
            /// `Landed` with the requested target (the actual landed position is
            /// reconciled by the worker after applying the seek; this call returns
            /// the optimistic outcome) or `PastEof` if the target is beyond the
            /// known track duration.
            ///
            /// # Errors
            /// Returns [`QueueError::Play`] if the player reports a seek failure.
            #[expr($.map_err(QueueError::from))]
            #[call(seek_player)]
            pub fn seek(&self, seconds: f64) -> Result<SeekOutcome, QueueError>;

            /// Periodic tick: drives `PlayerImpl::tick` and drains queued engine
            /// events to act on `ItemDidPlayToEnd` (filtered) and forward
            /// `CurrentItemChanged` as
            /// [`QueueEvent::CurrentTrackChanged`](crate::event::QueueEvent::CurrentTrackChanged).
            ///
            /// # Errors
            /// Forwards `PlayError` from `PlayerImpl::tick`.
            #[expr($.map_err(QueueError::from))]
            #[call(tick_player)]
            pub fn tick(&self) -> Result<(), QueueError>;
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_events::{SlotId, TrackId};
    use kithara_platform::sync::Arc;
    use kithara_play::{ItemRole, PlayerEvent, TrackRef};
    use kithara_test_utils::kithara;

    use crate::{
        event::{QueueEvent, TrackStatus},
        queue::{state::tests::make_queue, types::SelectPhase},
        track::{TrackRecord, TrackSource},
    };

    #[kithara::test(tokio)]
    async fn spurious_item_did_play_to_end_is_filtered() {
        let queue = make_queue();
        let _a = queue.append("https://example.com/a.mp3");
        let _b = queue.append("https://example.com/b.mp3");

        queue.player.bus().publish(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Leading(TrackRef::new(
                TrackId::allocate(),
                SlotId::new(0),
                Arc::from(""),
            )),
        });

        queue
            .tick()
            .expect("BUG: tick returned error in test setup");

        assert_eq!(
            queue.lock_navigation().current(),
            None,
            "navigation must not have advanced"
        );
    }

    #[kithara::test(tokio)]
    async fn eof_after_queue_end_does_not_restart_from_first_track() {
        let queue = make_queue();
        let a = TrackId::allocate();
        let b = TrackId::allocate();
        queue.tracks.lock().extend([
            TrackRecord::new(a, "a".into(), TrackSource::from("a")),
            TrackRecord::new(b, "b".into(), TrackSource::from("b")),
        ]);
        queue.lock_navigation_mut().select(b, &[a, b]);
        queue.lock_navigation_mut().finish();
        let mut rx = queue.subscribe();

        queue.player.bus().publish(PlayerEvent::ItemDidPlayToEnd {
            item: ItemRole::Leading(TrackRef::new(
                b,
                SlotId::new(0),
                Arc::from(format!("test://memory/{}", b.as_u64())),
            )),
        });

        queue
            .tick()
            .expect("BUG: tick returned error in test setup");

        assert_eq!(
            queue.lock_navigation().current(),
            None,
            "stale EOF must not restart the queue"
        );
        let saw_ended = crate::queue::state::tests::wait_for_queue_event(
            &mut rx,
            |ev| matches!(ev, QueueEvent::QueueEnded),
            200,
        )
        .await;
        assert!(!saw_ended, "stale EOF must not duplicate QueueEnded");
    }

    #[kithara::test(tokio)]
    async fn play_retries_the_current_track_after_its_prefetch_failed() {
        let queue = make_queue();
        let id = queue
            .append("https://example.com/a.mp3")
            .expect("open queue accepts a track");
        queue.set_status(id, TrackStatus::Failed("network offline".into()));

        queue.play_inner();

        let SelectPhase::Pending(pending) = *queue.lock_pending_select_mut() else {
            panic!("play must retain selection while retrying the failed track")
        };
        assert_eq!(pending.id, id);
        assert_eq!(pending.playback, kithara_play::SelectionPlayback::Play);
    }

    #[kithara::test(tokio)]
    #[case::append(false)]
    #[case::insert(true)]
    async fn play_promotes_the_initial_pending_prefetch(#[case] insert: bool) {
        let queue = make_queue();
        let id = if insert {
            queue.insert("https://example.com/a.mp3", None)
        } else {
            queue.append("https://example.com/a.mp3")
        }
        .expect("open queue accepts a track");
        assert!(!queue.tracks.attempt_selected(id));

        queue.play();

        assert!(queue.tracks.attempt_selected(id));
    }
}
