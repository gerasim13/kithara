use kithara_bufpool::HasPool;
use kithara_events::TrackId;

use crate::{
    attempts::LoadClass,
    event::TrackStatus,
    queue::{
        Queue,
        types::{PendingSelect, SelectPhase},
    },
    track::TrackSource,
};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Synchronous-select counterpart to [`Self::override_pending_select`]:
    /// the user picked a `Loaded` track, so any other in-flight load is
    /// stale. Drop pending and mark the stale track [`TrackStatus::Cancelled`],
    /// which drops its live attempt and a resource it already loaded, so
    /// neither its load's finish nor the successor arming barges in on top
    /// of the just-selected track.
    pub(in crate::queue) fn cancel_stale_pending(&mut self, applying_id: TrackId) {
        let stale = match self.pending_select {
            SelectPhase::Pending(prev) if prev.id != applying_id => Some(prev.id),
            _ => None,
        };
        self.pending_select = SelectPhase::Idle;
        if let Some(stale_id) = stale {
            self.set_status(stale_id, TrackStatus::Cancelled);
        }
    }

    /// Replace `pending_select` with a new selection. If the previous
    /// pending track is different from `new`, mark it
    /// [`TrackStatus::Cancelled`] so the in-flight load — when it
    /// finishes — does not silently plant its resource into the queue
    /// and "barge in" via auto-advance. `TrackStatus::Cancelled` is the
    /// single source of truth for this: setting it drops the track's live
    /// attempt, so that load's finish lands stale, and `advance_to_next`
    /// reads it when iterating.
    /// See Bug B reproducer (`tests/.../track_switch_race.rs`).
    pub(in crate::queue) fn override_pending_select(&mut self, new: PendingSelect) {
        let prev_id = match self.pending_select {
            SelectPhase::Pending(prev) if prev.id != new.id => Some(prev.id),
            _ => None,
        };
        self.pending_select = SelectPhase::Pending(new);
        if let Some(prev_id) = prev_id {
            self.set_status(prev_id, TrackStatus::Cancelled);
        }
    }

    pub(in crate::queue) fn promote_pending_load(&self, id: TrackId) {
        if let Some(source) = self.tracks.source(id) {
            self.loader.promote_load(id, source);
        }
    }

    pub(in crate::queue) fn spawn_apply_after_load(
        &self,
        id: TrackId,
        source: TrackSource<S>,
        class: LoadClass,
    ) {
        self.loader.spawn_load(id, source, class);
    }
}
