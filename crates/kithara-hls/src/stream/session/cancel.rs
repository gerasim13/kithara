use kithara_platform::{CancelToken, sync::RwLock};

pub(super) struct SessionCancel {
    pub(super) root: CancelToken,
    fetch: RwLock<CancelToken>,
}

impl SessionCancel {
    pub(super) fn new(root: CancelToken) -> Self {
        let fetch = RwLock::new(root.child());
        Self { root, fetch }
    }

    pub(super) fn abort(&self) {
        self.root.cancel();
    }

    pub(super) fn handle(&self) -> CancelToken {
        self.fetch.read().clone()
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.root.is_cancelled()
    }

    pub(super) fn rearm(&self) {
        self.fetch.read().cancel();
        *self.fetch.write() = self.root.child();
    }
}
