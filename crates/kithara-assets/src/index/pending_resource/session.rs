#![forbid(unsafe_code)]

use kithara_platform::sync::Arc;

use super::{PendingResource, WriterClaim};
use crate::{index::pending::PendingResourceInner, layout::ResourceKey};

/// Immutable wiring for one canonical pending resource.
pub(in crate::index) struct PendingResourceSession<S> {
    pub(super) inner: Arc<PendingResourceInner<S>>,
    pub(super) slot: Arc<PendingResource<S>>,
    pub(super) key: ResourceKey,
}

impl<S> PendingResourceSession<S> {
    pub(in crate::index) fn new(
        inner: &Arc<PendingResourceInner<S>>,
        key: &ResourceKey,
        slot: &Arc<PendingResource<S>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::clone(inner),
            key: key.clone(),
            slot: Arc::clone(slot),
        })
    }
}

pub(super) struct WriterIdentity<S> {
    pub(super) claim: Arc<WriterClaim>,
    pub(super) session: Arc<PendingResourceSession<S>>,
}

impl<S> WriterIdentity<S> {
    pub(super) fn new(
        claim: Arc<WriterClaim>,
        session: &Arc<PendingResourceSession<S>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            claim,
            session: Arc::clone(session),
        })
    }
}
