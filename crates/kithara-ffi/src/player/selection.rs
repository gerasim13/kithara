use crate::{item::AudioPlayerItem, player::AudioPlayer, types::FfiError};

#[cfg_attr(feature = "uniffi", uniffi::export)]
impl AudioPlayer {
    /// Select `item` in the queue with the given transition.
    ///
    /// `FfiTransition::None` performs an immediate cut (`AVQueuePlayer`
    /// user-initiated-selection idiom: tap a track in a list).
    /// `FfiTransition::Crossfade` uses the player's configured duration
    /// (typical for Next/Prev buttons). Play state is not changed here:
    /// the engine continues playing if it was, pauses if it was.
    ///
    /// # Errors
    ///
    /// Returns [`FfiError::InvalidArgument`] if `item` is not in the
    /// queue, [`FfiError::NotReady`] if its resource is not yet loaded,
    /// or [`FfiError::Internal`] if the underlying Queue fails to select.
    pub fn select(
        &self,
        item: &AudioPlayerItem,
        transition: crate::types::FfiTransition,
    ) -> Result<(), FfiError> {
        self.inner.select(item, transition)
    }
}
