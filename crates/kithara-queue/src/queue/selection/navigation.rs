use kithara_bufpool::HasPool;
use kithara_events::{AdvanceReason, QueueEvent, TrackId, TrackStatus};
use tracing::debug;

use crate::{
    error::QueueError,
    navigation::RepeatMode,
    queue::{QueueControl, types::Transition},
    track::TrackEntry,
};

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Advance to the next track per navigation rules. Returns the newly
    /// selected id, or `None` when the queue has ended (and
    /// [`RepeatMode::Off`](crate::navigation::RepeatMode::Off) is active).
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the successor cannot be selected.
    pub fn advance_to_next(
        &self,
        transition: Transition,
        reason: AdvanceReason,
    ) -> Result<Option<TrackId>, QueueError> {
        self.with_open_result(|queue| queue.advance_to_next_inner(transition, reason))
    }

    pub(in crate::queue) fn advance_to_next_inner(
        &self,
        transition: Transition,
        reason: AdvanceReason,
    ) -> Result<Option<TrackId>, QueueError> {
        let Some(next) = self.next_selectable_entry() else {
            self.lock_navigation_mut().finish();
            self.bus.publish(QueueEvent::QueueEnded);
            return Ok(None);
        };
        let id = next.id;
        self.select_with_reason(id, transition, reason)?;
        Ok(Some(id))
    }

    /// Read the next selectable entry without mutating navigation. Selection
    /// commits navigation only when the player selection actually commits.
    pub(in crate::queue) fn next_selectable_entry(&self) -> Option<TrackEntry> {
        let len = self.len();
        if len == 0 {
            return None;
        }
        let (current, repeat) = {
            let nav = self.lock_navigation();
            (nav.current_index(), nav.repeat_mode())
        };
        let tracks = self.lock_tracks();
        let selectable = |idx: usize| {
            let record = tracks.get(idx)?;
            if matches!(record.status, TrackStatus::Cancelled) {
                debug!(
                    id = record.id.as_u64(),
                    "advance_to_next: skipping cancelled track"
                );
                None
            } else {
                Some(record.entry())
            }
        };
        match (current, repeat) {
            (None, _) => (0..len).find_map(selectable),
            (Some(idx), RepeatMode::One) => std::iter::once(idx).find_map(selectable),
            (Some(idx), RepeatMode::Off) => (idx.saturating_add(1)..len).find_map(selectable),
            (Some(idx), RepeatMode::All) => (idx.saturating_add(1)..len)
                .chain(0..=idx)
                .find_map(selectable),
        }
    }

    /// Go back to the previous track. Returns the newly selected id, or
    /// `None` at index 0.
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the predecessor cannot be selected.
    pub fn return_to_previous(
        &self,
        transition: Transition,
    ) -> Result<Option<TrackId>, QueueError> {
        self.with_open_result(|queue| queue.return_to_previous_inner(transition))
    }

    fn return_to_previous_inner(
        &self,
        transition: Transition,
    ) -> Result<Option<TrackId>, QueueError> {
        let Some(prev_idx) = self.lock_navigation_mut().prev() else {
            return Ok(None);
        };
        let Some(id) = self.lock_tracks().get(prev_idx).map(|entry| entry.id) else {
            return Ok(None);
        };
        self.select_with_reason(id, transition, AdvanceReason::UserPrev)?;
        Ok(Some(id))
    }
}
