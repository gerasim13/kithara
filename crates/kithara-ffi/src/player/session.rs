use super::AudioPlayer;
use crate::types::{FfiDuckingMode, FfiError, FfiInterruptionKind};

/// Platform audio-session signals: route changes and competing sounds.
#[cfg_attr(feature = "uniffi", uniffi::export)]
impl AudioPlayer {
    /// Notify the native player that the platform audio route changed.
    ///
    /// This does not change queue state. If playback is active, the
    /// native output stream is recreated so CoreAudio/CPAL cannot keep a
    /// stale route after headphones or Bluetooth devices are removed.
    ///
    /// # Errors
    ///
    /// Returns [`FfiError`] when the native player cannot schedule the
    /// route invalidation.
    pub fn notify_audio_route_changed(&self, reason: &str) -> Result<(), FfiError> {
        self.inner.notify_audio_route_changed(reason)
    }

    /// Notify the native player that the platform interrupted, or released,
    /// the audio output.
    ///
    /// An interruption stops the output below the engine: the audio callback
    /// is no longer invoked, so playback can neither observe the interruption
    /// nor report it, and every value the audio thread publishes freezes where
    /// it stood. Reporting it here is what keeps the observable playback state
    /// honest while nothing is audible. Getting the output back is
    /// [`notify_audio_route_changed`](Self::notify_audio_route_changed).
    pub fn notify_interruption(&self, kind: FfiInterruptionKind) {
        self.inner.notify_interruption(kind.into());
    }

    /// Lower or restore the whole session output under a competing sound,
    /// such as a call or a navigation prompt.
    ///
    /// # Errors
    ///
    /// Returns [`FfiError`] when the audio session rejects the change.
    pub fn set_ducking_mode(&self, mode: FfiDuckingMode) -> Result<(), FfiError> {
        self.inner.set_ducking_mode(mode)
    }
}
