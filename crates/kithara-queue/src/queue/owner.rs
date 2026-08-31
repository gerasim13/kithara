use kithara_bufpool::HasPool;
use kithara_events::{AdvanceReason, TrackId};

use super::{Queue, Transition};
use crate::{QueueError, TrackSource};

impl<S> Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Append a track while this concrete queue remains owned by its caller.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append<T: Into<TrackSource<S>>>(&self, source: T) -> Result<TrackId, QueueError> {
        self.control.append(source)
    }

    /// Append a track with a caller-owned stable id.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append_with_id<T: Into<TrackSource<S>>>(
        &self,
        id: TrackId,
        source: T,
    ) -> Result<TrackId, QueueError> {
        self.control.append_with_id(id, source)
    }

    /// Insert a track after `after`, or at the head when it is absent.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when `after` is not present.
    pub fn insert<T: Into<TrackSource<S>>>(
        &self,
        source: T,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        self.control.insert(source, after)
    }

    /// Insert a track with a caller-owned stable id.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when `after` is not present.
    pub fn insert_with_id<T: Into<TrackSource<S>>>(
        &self,
        id: TrackId,
        source: T,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        self.control.insert_with_id(id, source, after)
    }

    delegate::delegate! {
        to self.control {
            /// Advance to the next navigation-owned track.
            ///
            /// # Errors
            ///
            /// Returns a queue or player error when the successor cannot be selected.
            pub fn advance_to_next(
                &self,
                transition: Transition,
                reason: AdvanceReason,
            ) -> Result<Option<TrackId>, QueueError>;

            /// Return to the previous navigation-owned track.
            ///
            /// # Errors
            ///
            /// Returns a queue or player error when the predecessor cannot be selected.
            pub fn return_to_previous(
                &self,
                transition: Transition,
            ) -> Result<Option<TrackId>, QueueError>;
        }
    }
}
