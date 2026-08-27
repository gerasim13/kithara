use kithara_events::{AdvanceReason, TrackId};

use super::{Queue, Transition};
use crate::{QueueError, TrackSource};

impl Queue {
    /// Append a track while this concrete queue remains owned by its caller.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append<S: Into<TrackSource>>(&self, source: S) -> Result<TrackId, QueueError> {
        self.control.append(source)
    }

    /// Append a track with a caller-owned stable id.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError::Play`] after the resident player is closed.
    pub fn append_with_id<S: Into<TrackSource>>(
        &self,
        id: TrackId,
        source: S,
    ) -> Result<TrackId, QueueError> {
        self.control.append_with_id(id, source)
    }

    /// Insert a track after `after`, or at the head when it is absent.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when `after` is not present.
    pub fn insert<S: Into<TrackSource>>(
        &self,
        source: S,
        after: Option<TrackId>,
    ) -> Result<TrackId, QueueError> {
        self.control.insert(source, after)
    }

    /// Insert a track with a caller-owned stable id.
    ///
    /// # Errors
    ///
    /// Returns [`QueueError`] when `after` is not present.
    pub fn insert_with_id<S: Into<TrackSource>>(
        &self,
        id: TrackId,
        source: S,
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
