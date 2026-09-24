use kithara::platform::sync::Arc;

use super::AudioPlayer;
use crate::{Inner, types::FfiError};

#[cfg_attr(feature = "uniffi-web", uniffi::export)]
impl AudioPlayer {
    /// Create a Web player attached to the initialized host.
    ///
    /// # Errors
    /// Returns a lifecycle error if the host is not ready.
    #[cfg_attr(feature = "uniffi-web", uniffi::constructor)]
    pub fn new_web() -> Result<Arc<Self>, FfiError> {
        crate::web::bridge::require_initialized_domain()?;
        Ok(Arc::new(Self {
            inner: Inner::default(),
        }))
    }

    /// Submit a new default playback rate to the Web worker.
    ///
    /// # Errors
    /// Returns an error if the worker command cannot be accepted.
    pub fn set_playing_rate(&self, rate: f32) -> Result<(), FfiError> {
        self.inner.try_set_playing_rate(rate)
    }
}
