#![forbid(unsafe_code)]

use kithara_platform::sync::Arc;

use super::{PendingResource, WriterClaim};
use crate::{index::pending::PendingResourceInner, layout::ResourceKey};

/// Immutable wiring for one canonical pending resource.
pub(in crate::index) struct PendingResourceSession {
    pub(super) inner: Arc<PendingResourceInner>,
    pub(super) slot: Arc<PendingResource>,
    pub(super) key: ResourceKey,
}

impl PendingResourceSession {
    pub(in crate::index) fn new(
        inner: &Arc<PendingResourceInner>,
        key: &ResourceKey,
        slot: &Arc<PendingResource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::clone(inner),
            key: key.clone(),
            slot: Arc::clone(slot),
        })
    }
}

pub(super) struct WriterIdentity {
    pub(super) claim: Arc<WriterClaim>,
    pub(super) session: Arc<PendingResourceSession>,
}

impl WriterIdentity {
    pub(super) fn new(claim: Arc<WriterClaim>, session: &Arc<PendingResourceSession>) -> Arc<Self> {
        Arc::new(Self {
            claim,
            session: Arc::clone(session),
        })
    }
}
