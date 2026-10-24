use std::collections::BTreeMap;

use kithara_platform::{CancelToken, sync::Mutex};

use crate::variant::DispatchTokens;

/// Cancel hierarchy of one session: `root` -> `fetch` -> per-segment lookahead.
///
/// Exact size discovery rides `root`; `fetch` keeps immutable body fetches live
/// across seeks. Lookahead children can be retired against the reader's current
/// owed window, including fetches that became owed after their dispatch.
pub(super) struct SessionCancel {
    pub(super) root: CancelToken,
    fetch: CancelToken,
    lookahead: Mutex<BTreeMap<u32, CancelToken>>,
}

impl SessionCancel {
    pub(super) fn new(root: CancelToken) -> Self {
        let fetch = root.child();
        Self {
            root,
            fetch,
            lookahead: Mutex::default(),
        }
    }

    pub(super) fn dispatch_tokens(&self) -> DispatchTokens<'_> {
        DispatchTokens {
            metadata: self.root.clone(),
            fetch: self.fetch.clone(),
            lookahead: &self.lookahead,
        }
    }

    pub(super) fn take_retired_lookahead(&self, owed_end: Option<u32>) -> Vec<CancelToken> {
        let mut retired: Vec<CancelToken> = Vec::new();
        self.lookahead.lock().retain(|segment, token| {
            let owed = owed_end.is_some_and(|last| *segment <= last);
            if !owed {
                retired.push(token.clone());
            }
            owed
        });
        retired
    }

    delegate::delegate! {
        to self.root {
            #[call(cancel)]
            pub(super) fn abort(&self);
            pub(super) fn is_cancelled(&self) -> bool;
        }
    }
}
