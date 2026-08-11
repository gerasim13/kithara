#![forbid(unsafe_code)]

use std::{error::Error as StdError, io::Error as IoError};

use kithara_platform::sync::Arc;
use kithara_storage::StorageError;

use super::{PendingResource, SessionPhase};
use crate::{
    error::{AssetsError, AssetsResult},
    layout::ResourceKey,
};

pub(crate) type RemoveResource = Arc<dyn Fn(&ResourceKey) -> AssetsResult<()> + Send + Sync>;

/// Typed source retained when a pending acquisition cannot remove its resource.
#[derive(Clone, Debug, derive_more::Display)]
#[doc(hidden)]
#[display("pending resource cleanup failed for {key:?}: {source}")]
pub struct PendingResourceCleanupError {
    source: Arc<AssetsError>,
    key: ResourceKey,
}

impl PendingResourceCleanupError {
    pub(super) fn new(key: ResourceKey, source: AssetsError) -> Self {
        Self {
            key,
            source: Arc::new(source),
        }
    }

    /// Resource whose terminal cleanup failed.
    #[must_use]
    pub const fn key(&self) -> &ResourceKey {
        &self.key
    }
}

impl From<&PendingResourceCleanupError> for AssetsError {
    fn from(error: &PendingResourceCleanupError) -> Self {
        Self::Storage(StorageError::Io(IoError::other(error.clone())))
    }
}

impl StdError for PendingResourceCleanupError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

impl PendingResource {
    pub(super) fn record_cleanup_failure(
        &self,
        key: &ResourceKey,
        source: AssetsError,
    ) -> AssetsError {
        let failure = PendingResourceCleanupError::new(key.clone(), source);
        let public = AssetsError::from(&failure);
        self.state.lock().phase = SessionPhase::CleanupFailed(failure);
        public
    }
}
