use kithara_platform::sync::Arc;
use kithara_stream::SeekObserve;

/// Seek-observe state consulted by one variant for flushing gates.
pub(super) struct ReaderRuntime {
    seek_obs: Arc<dyn SeekObserve>,
}

impl ReaderRuntime {
    pub(super) fn new(seek_obs: Arc<dyn SeekObserve>) -> Self {
        Self { seek_obs }
    }

    pub(super) fn is_flushing(&self) -> bool {
        self.seek_obs.is_flushing()
    }

    pub(super) fn is_seek_active(&self) -> bool {
        self.seek_obs.is_flushing() || self.seek_obs.is_pending()
    }
}
