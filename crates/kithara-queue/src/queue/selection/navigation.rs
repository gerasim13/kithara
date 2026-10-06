use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use smallvec::SmallVec;
use tracing::debug;

use crate::{
    error::QueueError,
    event::{AdvanceReason, QueueEvent, TrackStatus},
    navigation::RepeatMode,
    queue::{Queue, types::Transition},
    track::TrackEntry,
};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub(in crate::queue) fn advance_to_next_inner(
        &mut self,
        transition: Transition,
        reason: AdvanceReason,
    ) -> Result<Option<TrackId>, QueueError> {
        let Some(next) = self.next_selectable_entry(reason) else {
            let current = self.current().map(|entry| entry.id);
            let deck_item = self.player.current_item();
            if matches!(
                reason,
                AdvanceReason::NaturalEof
                    | AdvanceReason::TrackFailed
                    | AdvanceReason::CrossfadePreArm
            ) {
                debug!(
                    ?reason,
                    ?current,
                    ?deck_item,
                    "navigation has no successor: the queue ends here"
                );
                self.lock_navigation_mut().finish();
                self.bus.publish(QueueEvent::QueueEnded);
            } else {
                debug!(
                    ?reason,
                    ?current,
                    ?deck_item,
                    "navigation has no successor: staying on the current track"
                );
            }
            return Ok(None);
        };
        let id = next.id;
        self.select_with_reason(id, transition, reason)?;
        self.commit_navigation_to(id);
        Ok(Some(id))
    }

    /// Advance to the next navigation-owned track, as
    /// [`QueueControl::next`](crate::QueueControl::next) does, while the
    /// caller still owns this queue.
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the successor cannot be selected.
    pub fn next(&mut self, transition: Transition) -> Result<Option<TrackId>, QueueError> {
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

    /// Return to the previous navigation-owned track, as
    /// [`QueueControl::previous`](crate::QueueControl::previous) does, while
    /// the caller still owns this queue.
    ///
    /// # Errors
    ///
    /// Returns a queue or player error when the predecessor cannot be selected.
    pub fn previous(&mut self, transition: Transition) -> Result<Option<TrackId>, QueueError> {
        self.with_open_result(|queue| queue.return_to_previous_inner(transition))
    }

    fn return_to_previous_inner(
        &mut self,
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
