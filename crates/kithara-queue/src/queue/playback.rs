use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::{PlayError, SeekOutcome, SelectionPlayback, SuccessorLink};
use tracing::debug;

use super::{
    Queue, QueueControl, QueueRuntime,
    types::{CachedPosition, PendingSelect, PlaybackView, SelectPhase, Transition},
};
use crate::{
    ActionAtItemEnd,
    error::QueueError,
    event::{AdvanceReason, TrackStatus},
    loading::LoadClass,
    track::TrackEntry,
};

impl<S> QueueRuntime<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Whether the user has paused playback.
    ///
    /// Reads the Player's explicit paused phase, not its effective rate or
    /// live output state: both become inactive at natural EOF without turning
    /// that EOF into a user pause.
    pub(super) fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    /// The player's live playback state with `position` replaced by the
    /// queue's cached, 0.0-smoothed one.
    fn playback_view_at(&self, position: Option<f64>) -> PlaybackView {
        let mut view = self
            .player
            .playback_snapshot()
            .map(PlaybackView::from)
            .unwrap_or_default();
        view.position = position;
        view
    }
}

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Single coherent read of the player's live playback state; see
    /// [`Queue::playback_view`].
    #[must_use]
    pub fn playback_view(&self) -> PlaybackView {
        self.playback_view_at(self.position_seconds())
    }

    /// Latest monotonic playback position for the current track in seconds;
    /// see [`Queue::position_seconds`].
    #[must_use]
    pub fn position_seconds(&self) -> Option<f64> {
        self.view.position_seconds()
    }
}

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
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
        self.playback_view_at(self.position_seconds())
    }

    /// Latest monotonic playback position for the current track in seconds.
    /// Updated on every tick; skips transient 0.0 samples the engine
    /// produces on pause/resume so downstream UIs see stable values.
    #[must_use]
    pub fn position_seconds(&self) -> Option<f64> {
        self.position.into()
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

    fn freeze_cached_position(&mut self) {
        if let Some(t) = self.player.position_seconds() {
            self.position = CachedPosition::known(t);
        }
    }

    /// The successor the queue wants armed behind the current track: the one
    /// navigation offers at its natural end, while the queue advances, no
    /// selection waits for its load, and the cursor stands on the track the
    /// deck plays. Repeating the current track needs no successor.
    fn wanted_successor(&mut self) -> Option<TrackEntry> {
        if self.action_at_item_end() != ActionAtItemEnd::Advance
            || matches!(self.pending_select, SelectPhase::Pending(_))
        {
            return None;
        }
        let current = self.current()?;
        if self.player.current_item() != Some(current.id) {
            return None;
        }
        self.next_selectable_entry(AdvanceReason::NaturalEof)
            .filter(|next| next.id != current.id)
    }

    /// Keep the deck's armed successor the one the queue wants. Once the
    /// current track ends within the longer of the prefetch lead and the
    /// crossfade, in session time, a consumed successor reloads and a loaded
    /// one is armed; a crossfade successor is committed once the track ends
    /// within the fade, so the two overlap. A gapless one needs no commit:
    /// the processor plays it on the frame after the current track's last.
    pub(super) fn reconcile_successor(&mut self) {
        let settings = self.crossfade_settings();
        let lead = self.config.prefetch_duration.max(settings.duration);
        let time = self.playback_time().filter(|time| time.ends_within(lead));
        let armed = self.player.armed_next();
        if armed.is_none() && time.is_none() {
            return;
        }
        let wanted = self.wanted_successor();
        if let Some(armed) = armed
            && wanted.as_ref().is_none_or(|next| next.id != armed)
        {
            self.disarm_successor(armed);
        }
        let (Some(time), Some(next)) = (time, wanted) else {
            return;
        };
        match next.status {
            TrackStatus::Consumed => {
                let Some(source) = self.tracks.source(next.id) else {
                    return;
                };
                self.tracks.set_status(next.id, TrackStatus::Pending);
                self.spawn_apply_after_load(next.id, source, LoadClass::Prefetch);
            }
            TrackStatus::Loaded => {
                let link = SuccessorLink::from(settings);
                if armed != Some(next.id) && !self.arm_successor(next.id, link) {
                    return;
                }
                if link == SuccessorLink::Fade && time.ends_within(settings.duration) {
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

    /// Hand the loaded successor `id` to the deck to arm; `false` when the
    /// deck did not arm it. An arm that fails spends the resource, so the
    /// track is consumed and reloads once it is wanted again.
    fn arm_successor(&mut self, id: TrackId, link: SuccessorLink) -> bool {
        let Some(resource) = self.tracks.take_resource(id) else {
            debug!(id = id.as_u64(), "the successor has no resource to arm");
            return false;
        };
        match self.player.arm_next(id, resource, link) {
            Ok(()) => true,
            Err(error) => {
                debug!(%error, id = id.as_u64(), "the successor would not arm");
                self.tracks.set_status(id, TrackStatus::Consumed);
                false
            }
        }
    }

    fn cross_fade_into(&mut self, id: TrackId) {
        if let Err(error) =
            self.select_with_reason(id, Transition::Crossfade, AdvanceReason::CrossfadePreArm)
        {
            debug!(%error, id = id.as_u64(), "the armed successor would not cross-fade in");
        }
    }

    /// Take the armed successor `id` off the deck. Arming moved its resource
    /// onto the deck, so the track is consumed and reloads once it is wanted
    /// again.
    pub(super) fn disarm_successor(&mut self, id: TrackId) {
        self.player.unarm_next();
        self.tracks.set_status(id, TrackStatus::Consumed);
    }

    pub(crate) fn pause(&mut self) {
        self.command(Self::pause_inner);
    }

    /// [`Self::pause`] on an open queue, as the tick does when a drained end
    /// pauses the queue.
    pub(super) fn pause_inner(&mut self) {
        self.player.pause();
        if let SelectPhase::Pending(pending) = &mut self.pending_select {
            pending.playback = SelectionPlayback::Pause;
        }
        self.freeze_cached_position();
    }

    pub(crate) fn play(&mut self) {
        self.command(Self::play_inner);
    }

    /// The track play is about: the pending selection, else the one the deck
    /// holds, else the one the cursor stands on while the deck holds none.
    fn play_inner(&mut self) {
        if let SelectPhase::Pending(pending) = &mut self.pending_select {
            pending.playback = SelectionPlayback::Play;
        }
        self.player.play();

        let pending = match self.pending_select {
            SelectPhase::Pending(pending) => Some(pending),
            SelectPhase::Idle => None,
        };
        let target = pending.map(|pending| pending.id).or_else(|| {
            self.player
                .current_item()
                .or_else(|| self.current().map(|entry| entry.id))
        });
        let Some((id, status)) =
            target.and_then(|id| self.track(id).map(|entry| (id, entry.status)))
        else {
            return;
        };
        match status {
            TrackStatus::Loaded => {
                let resource = self.tracks.take_resource(id);
                if let Err(error) = self.player.select(id, resource, SelectionPlayback::Play) {
                    debug!(%error, id = id.as_u64(), "play could not start the loaded track");
                }
                self.tracks.set_status(id, TrackStatus::Consumed);
            }
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
                self.tracks.set_status(id, TrackStatus::Pending);
                self.spawn_apply_after_load(id, source, LoadClass::Interactive);
            }
            TrackStatus::Consumed | TrackStatus::Cancelled => {}
        }
    }

    pub(crate) fn seek(&mut self, seconds: f64) -> Result<SeekOutcome, QueueError> {
        Ok(self.with_open_result(|queue| queue.seek_player(seconds))?)
    }

    /// Resumes seeking after the last track plays to natural EOF and the navigation cursor runs off
    /// the end, leaving `current()` at `None`.
    fn seek_player(&mut self, seconds: f64) -> Result<SeekOutcome, PlayError> {
        if self.current().is_none()
            && let Some(id) = self.navigation.last_selected()
        {
            let ids = self.track_ids();
            self.navigation.select(id, &ids);
            self.handle_current_item_changed();
        }
        let outcome = self.player.seek_seconds(seconds)?;
        if let SeekOutcome::Landed { landed_at, .. } = outcome {
            self.position = CachedPosition::known(landed_at.as_secs_f64());
        }
        Ok(outcome)
    }

    pub(crate) fn tick(&mut self) -> Result<(), QueueError> {
        Ok(self.tick_player()?)
    }

    pub(super) fn tick_player(&mut self) -> Result<(), PlayError> {
        self.with_open(Self::tick_player_inner)
    }

    fn tick_player_inner(&mut self) {
        self.player.process_notifications();
        self.drain_player_events();
        self.update_cached_position();
        self.reconcile_successor();
    }

    fn update_cached_position(&mut self) {
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
        let prev = self.position_seconds();
        if t == 0.0 && prev.is_some_and(|p| p > MIN_STABLE_POSITION_SECS) {
            return;
        }
        self.position = CachedPosition::known(t);
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
        queue::{Queue, state::tests::make_queue, types::SelectPhase},
        test_pools::TestPools,
        track::{TrackRecord, TrackSource},
    };

    #[kithara::test(tokio)]
    async fn spurious_item_did_play_to_end_is_filtered() {
        let mut queue = make_queue();
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
            queue.navigation.current(),
            None,
            "navigation must not have advanced"
        );
    }

    #[kithara::test(tokio)]
    async fn eof_after_queue_end_does_not_restart_from_first_track() {
        let mut queue = make_queue();
        let a = TrackId::allocate();
        let b = TrackId::allocate();
        queue.tracks.records_mut().extend([
            TrackRecord::new(a, "a".into(), TrackSource::from("a")),
            TrackRecord::new(b, "b".into(), TrackSource::from("b")),
        ]);
        queue.navigation.select(b, &[a, b]);
        queue.navigation.finish();
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
            queue.navigation.current(),
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
        let mut queue = make_queue();
        let id = queue
            .append("https://example.com/a.mp3")
            .expect("open queue accepts a track");
        queue
            .tracks
            .set_status(id, TrackStatus::Failed("network offline".into()));

        queue.play_inner();

        let SelectPhase::Pending(pending) = queue.pending_select else {
            panic!("play must retain selection while retrying the failed track")
        };
        assert_eq!(pending.id, id);
        assert_eq!(pending.playback, kithara_play::SelectionPlayback::Play);
    }

    #[kithara::test(tokio)]
    #[case::append(false)]
    #[case::insert(true)]
    async fn play_promotes_the_initial_pending_prefetch(#[case] insert: bool) {
        let mut queue = make_queue();
        let id = if insert {
            queue.insert("https://example.com/a.mp3", None)
        } else {
            queue.append("https://example.com/a.mp3")
        }
        .expect("open queue accepts a track");
        assert!(!attempt_selected(&queue, id));

        queue.play();

        assert!(attempt_selected(&queue, id));
    }

    /// Whether the selection wants `id`'s live load attempt.
    fn attempt_selected(queue: &Queue<TestPools>, id: TrackId) -> bool {
        queue.tracks.records().iter().any(|record| {
            record.id == id && record.load.as_ref().is_some_and(|attempt| attempt.selected)
        })
    }
}
