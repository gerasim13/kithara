use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use smallvec::SmallVec;
use tracing::debug;

use crate::{
    error::QueueError,
    event::{AdvanceReason, QueueEvent, TrackStatus},
    navigation::RepeatMode,
    queue::{QueueControl, types::Transition},
    track::TrackEntry,
};

impl<S> QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(in crate::queue) fn advance_to_next_inner(
        &self,
        transition: Transition,
        reason: AdvanceReason,
    ) -> Result<Option<TrackId>, QueueError> {
        let Some(next) = self.next_selectable_entry(reason) else {
            if matches!(
                reason,
                AdvanceReason::NaturalEof
                    | AdvanceReason::TrackFailed
                    | AdvanceReason::CrossfadePreArm
            ) {
                self.lock_navigation_mut().finish();
                self.bus.publish(QueueEvent::QueueEnded);
            }
            return Ok(None);
        };
        let id = next.id;
        self.select_with_reason(id, transition, reason)?;
        Ok(Some(id))
    }

    /// Advance to the next track per navigation rules. Returns the newly
    /// selected id, or `None` when the queue has ended (and
    /// [`RepeatMode::Off`](crate::navigation::RepeatMode::Off) is active).
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the successor cannot be selected.
    pub fn next(&self, transition: Transition) -> Result<Option<TrackId>, QueueError> {
        self.with_open_result(|queue| {
            queue.advance_to_next_inner(transition, AdvanceReason::UserNext)
        })
    }

    /// Read the next selectable entry without mutating navigation. Selection
    /// commits navigation only when the player selection actually commits.
    pub(in crate::queue) fn next_selectable_entry(
        &self,
        reason: AdvanceReason,
    ) -> Option<TrackEntry> {
        let tracks = self.lock_tracks();
        let selectable = tracks
            .iter()
            .filter(|record| {
                let available = !matches!(
                    record.status,
                    TrackStatus::Cancelled | TrackStatus::Failed(_)
                );
                if !available {
                    debug!(
                        id = record.id.as_u64(),
                        "navigation skipped unavailable track"
                    );
                }
                available
            })
            .map(crate::track::TrackRecord::entry)
            .collect::<Vec<TrackEntry>>();
        drop(tracks);
        let ids = selectable
            .iter()
            .map(|entry| entry.id)
            .collect::<SmallVec<[_; 16]>>();
        let mut navigation = self.lock_navigation_mut();
        let automatic = matches!(
            reason,
            AdvanceReason::NaturalEof | AdvanceReason::TrackFailed | AdvanceReason::CrossfadePreArm
        );
        let allow_repeat_one = matches!(
            reason,
            AdvanceReason::NaturalEof | AdvanceReason::CrossfadePreArm
        );
        let allow_wrap = automatic && navigation.repeat_mode() == RepeatMode::All;
        let id = navigation.next(&ids, allow_repeat_one, allow_wrap)?;
        drop(navigation);
        selectable.into_iter().find(|entry| entry.id == id)
    }

    pub(in crate::queue) fn peek_selectable_entry(&self) -> Option<TrackEntry> {
        let selectable = self
            .lock_tracks()
            .iter()
            .filter(|record| {
                !matches!(
                    record.status,
                    TrackStatus::Cancelled | TrackStatus::Failed(_)
                )
            })
            .map(crate::track::TrackRecord::entry)
            .collect::<Vec<TrackEntry>>();
        let ids = selectable
            .iter()
            .map(|entry| entry.id)
            .collect::<SmallVec<[_; 16]>>();
        let id = self.lock_navigation().peek_next(&ids)?;
        selectable.into_iter().find(|entry| entry.id == id)
    }

    /// Go back to the previous track. Returns the newly selected id, or
    /// `None` at index 0.
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the predecessor cannot be selected.
    pub fn previous(&self, transition: Transition) -> Result<Option<TrackId>, QueueError> {
        self.with_open_result(|queue| queue.return_to_previous_inner(transition))
    }

    fn return_to_previous_inner(
        &self,
        transition: Transition,
    ) -> Result<Option<TrackId>, QueueError> {
        let tracks = self.tracks();
        let ids = tracks
            .iter()
            .map(|entry| entry.id)
            .collect::<SmallVec<[_; 16]>>();
        let Some(id) = self.lock_navigation_mut().prev(&ids) else {
            return Ok(None);
        };
        self.select_with_reason(id, transition, AdvanceReason::UserPrev)?;
        Ok(Some(id))
    }
}
