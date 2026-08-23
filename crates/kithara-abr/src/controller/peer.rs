use std::sync::{
    Weak,
    atomic::{AtomicBool, AtomicU64},
};

use kithara_events::EventBus;
use kithara_platform::{
    CancelGroup, CancelToken,
    sync::{Arc, Mutex, RwLock},
    time::Instant,
};

use super::throttle::EventThrottleCache;
use crate::{abr::Abr, state::AbrState};

/// Per-peer bookkeeping shared by tick orchestration and throttling.
pub(crate) struct PeerEntry {
    pub(crate) peer_weak: Weak<dyn Abr>,
    pub(super) bus: Arc<RwLock<Option<EventBus>>>,
    pub(super) variants_registered_published: AtomicBool,
    pub(super) bytes_downloaded: AtomicU64,
    pub(super) cancel: CancelGroup,
    pub(super) registration_cancel: CancelToken,
    pub(super) deferred_tick_at: Mutex<Option<Instant>>,
    pub(super) throttle: Mutex<EventThrottleCache>,
    pub(super) state: Option<Arc<AbrState>>,
}

impl PeerEntry {
    pub(super) fn arm_deferred_tick(&self, deadline: Instant) -> bool {
        let mut armed = self.deferred_tick_at.lock();
        if armed.is_some_and(|current| current <= deadline) {
            return false;
        }
        *armed = Some(deadline);
        true
    }

    pub(super) fn bus(&self) -> Option<EventBus> {
        self.bus.read().clone()
    }

    pub(super) fn clear_deferred_tick(&self) {
        *self.deferred_tick_at.lock() = None;
    }

    pub(super) fn take_deferred_tick(&self, deadline: Instant) -> bool {
        let mut armed = self.deferred_tick_at.lock();
        if *armed != Some(deadline) {
            return false;
        }
        *armed = None;
        true
    }
}
