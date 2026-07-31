use std::sync::{
    Weak,
    atomic::{AtomicBool, AtomicU64},
};

use kithara_events::EventBus;
use kithara_platform::sync::{Arc, Mutex, RwLock};

use super::throttle::EventThrottleCache;
use crate::{abr::Abr, state::AbrState};

/// Per-peer bookkeeping shared by tick orchestration and throttling.
pub(crate) struct PeerEntry {
    pub(crate) peer_weak: Weak<dyn Abr>,
    pub(super) bus: Arc<RwLock<Option<EventBus>>>,
    pub(super) variants_registered_published: AtomicBool,
    pub(super) bytes_downloaded: AtomicU64,
    pub(super) throttle: Mutex<EventThrottleCache>,
    pub(super) state: Option<Arc<AbrState>>,
}

impl PeerEntry {
    pub(super) fn bus(&self) -> Option<EventBus> {
        self.bus.read().clone()
    }
}
