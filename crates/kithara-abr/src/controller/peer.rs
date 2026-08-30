use std::sync::{
    Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
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
#[derive(fieldwork::Fieldwork)]
#[non_exhaustive]
#[fieldwork(opt_in, with, vis = "pub(super)")]
pub(crate) struct PeerEntry {
    pub(crate) peer_weak: Weak<dyn Abr>,
    pub(super) bus: Arc<RwLock<Option<EventBus>>>,
    pub(super) tick_requested: AtomicBool,
    pub(super) variants_registered_published: AtomicBool,
    pub(super) bytes_downloaded: AtomicU64,
    pub(super) cancel: CancelGroup,
    pub(super) registration_cancel: CancelToken,
    pub(super) throttle: Mutex<EventThrottleCache>,
    pub(super) tick_deadline: Mutex<Option<Instant>>,
    #[field(with)]
    pub(super) state: Option<Arc<AbrState>>,
}

impl PeerEntry {
    pub(super) fn new(
        peer_weak: Weak<dyn Abr>,
        bus: Arc<RwLock<Option<EventBus>>>,
        cancel: CancelGroup,
        registration_cancel: CancelToken,
    ) -> Self {
        Self {
            peer_weak,
            bus,
            cancel,
            registration_cancel,
            variants_registered_published: AtomicBool::default(),
            bytes_downloaded: AtomicU64::default(),
            tick_deadline: Mutex::default(),
            tick_requested: AtomicBool::default(),
            throttle: Mutex::default(),
            state: None,
        }
    }

    pub(super) fn bus(&self) -> Option<EventBus> {
        self.bus.read().clone()
    }

    pub(super) fn clear_tick_deadline(&self) {
        *self.tick_deadline.lock() = None;
    }

    pub(super) fn mark_tick_requested(&self) -> bool {
        !self.tick_requested.swap(true, Ordering::AcqRel)
    }

    pub(super) fn set_tick_deadline(&self, deadline: Instant) {
        *self.tick_deadline.lock() = Some(deadline);
    }

    pub(super) fn take_due_tick_deadline(&self, now: Instant) -> bool {
        let mut deadline = self.tick_deadline.lock();
        if !deadline.is_some_and(|deadline| deadline <= now) {
            return false;
        }
        *deadline = None;
        true
    }

    pub(super) fn take_tick_request(&self) -> bool {
        self.tick_requested.swap(false, Ordering::AcqRel)
    }

    pub(super) fn tick_deadline(&self) -> Option<Instant> {
        *self.tick_deadline.lock()
    }
}
