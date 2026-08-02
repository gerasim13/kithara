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

    delegate::delegate! {
        to self.root {
            #[call(cancel)]
            pub(super) fn abort(&self);
            pub(super) fn is_cancelled(&self) -> bool;
        }
    }

    pub(super) fn handle(&self) -> CancelToken {
        self.fetch.read().clone()
    }

    pub(super) fn rearm(&self) {
        self.fetch.read().cancel();
        *self.fetch.write() = self.root.child();
    }
}
