#![forbid(unsafe_code)]

use std::{
    fmt,
    sync::atomic::{AtomicU64, Ordering},
    task::Waker,
};

use dashmap::{DashMap, mapref::entry::Entry};
use kithara_bufpool::HasPool;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
};

use super::pending_resource::{
    PendingResource, PendingResourceSession, RemoveResource, ResourceAttachment, ResourceLease,
    SessionPhase,
};
use crate::{
    error::AssetsResult,
    layout::ResourceKey,
    resource::AcquisitionResult,
    store::{AssetReader, AssetStore, ResourceAcquisition},
};

#[cfg(test)]
type AttachProbe = Arc<dyn Fn() + Send + Sync>;

/// One consumer's contribution to the aggregate demand. `read_pos` is
/// shared with the consumer (advances seen without an update call);
/// `look_ahead = None` means "whole file" and collapses the watermark to
/// `u64::MAX`.
pub(crate) struct DemandEntry {
    read_pos: Arc<AtomicU64>,
    requested_end: AtomicU64,
    peer_waker: Mutex<Option<Waker>>,
    reader_waker: Mutex<Option<Waker>>,
    look_ahead: Option<u64>,
}

impl DemandEntry {
    pub(crate) fn new(read_pos: Arc<AtomicU64>, look_ahead: Option<u64>) -> Self {
        Self {
            read_pos,
            look_ahead,
            requested_end: AtomicU64::new(0),
            peer_waker: Mutex::new(None),
            reader_waker: Mutex::new(None),
        }
    }

    pub(super) fn clear_peer_waker(&self, waker: &Waker) {
        let old = {
            let mut current = self.peer_waker.lock();
            if current
                .as_ref()
                .is_some_and(|registered| registered.will_wake(waker))
            {
                current.take()
            } else {
                None
            }
        };
        drop(old);
    }

    pub(super) fn register_peer_waker(&self, waker: &Waker) {
        register_waker(&self.peer_waker, waker);
    }

    pub(super) fn register_reader_waker(&self, waker: &Waker) {
        register_waker(&self.reader_waker, waker);
    }

    pub(super) fn request_until(&self, end: u64) -> bool {
        self.requested_end.fetch_max(end, Ordering::AcqRel) < end
    }

    pub(super) fn take_peer_waker(&self) -> Option<Waker> {
        self.peer_waker.lock().take()
    }

    pub(super) fn take_reader_waker(&self) -> Option<Waker> {
        self.reader_waker.lock().take()
    }

    /// Per-entry watermark: how far this consumer wants bytes fetched.
    pub(super) fn watermark(&self) -> u64 {
        let prefetch = self.look_ahead.map_or(u64::MAX, |la| {
            self.read_pos.load(Ordering::Acquire).saturating_add(la)
        });
        prefetch.max(self.requested_end.load(Ordering::Acquire))
    }
}

fn register_waker(slot: &Mutex<Option<Waker>>, waker: &Waker) {
    let replacement = waker.clone();
    let (old, unused) = {
        let mut current = slot.lock();
        if current
            .as_ref()
            .is_none_or(|registered| !registered.will_wake(waker))
        {
            (current.replace(replacement), None)
        } else {
            (None, Some(replacement))
        }
    };
    drop(old);
    drop(unused);
}

pub(super) struct PendingResourceInner<S> {
    /// Parent of every slot's `writer_cancel` (the store cancel).
    pub(super) cancel: CancelToken,
    pub(super) slots: DashMap<ResourceKey, Arc<PendingResource<S>>>,
    #[cfg(test)]
    attach_probe: Mutex<Option<AttachProbe>>,
}

/// Opaque index of resources that are not yet ready in this store.
///
/// Cheap to [`Clone`] (one `Arc` bump); all clones share the same slot
/// map, so consumer demand aggregates across `AssetStore` clones automatically.
pub(crate) struct PendingResourceIndex<S> {
    inner: Arc<PendingResourceInner<S>>,
}

impl<S> Clone for PendingResourceIndex<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> PendingResourceIndex<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Create an empty index. `cancel` is the store cancel; each slot's
    /// `writer_cancel` is a child of it.
    pub(crate) fn new(cancel: CancelToken) -> Self {
        Self {
            inner: Arc::new(PendingResourceInner {
                cancel,
                slots: DashMap::new(),
                #[cfg(test)]
                attach_probe: Mutex::default(),
            }),
        }
    }

    pub(crate) fn attach_pending_resource(
        &self,
        key: &ResourceKey,
        entry: Arc<DemandEntry>,
        store: AssetStore<S>,
        remove: RemoveResource,
        acquire: impl FnOnce() -> AssetsResult<ResourceAcquisition<S>>,
    ) -> AssetsResult<AcquisitionResult<ResourceAttachment<S>, AssetReader<S>>> {
        let (slot, epoch, reader, peer_waker) = match self.inner.slots.entry(key.clone()) {
            Entry::Occupied(occupied) => {
                let slot = Arc::clone(occupied.get());
                let mut state = slot.state.lock();
                match &state.phase {
                    SessionPhase::CleanupFailed(failure) => {
                        return Err(failure.into());
                    }
                    SessionPhase::Committed => {
                        panic!("BUG: committed pending resource remained attachable");
                    }
                    SessionPhase::Active => {}
                    SessionPhase::Finishing => {
                        panic!("BUG: finishing pending resource remained attachable");
                    }
                }
                state.entries.push(Arc::clone(&entry));
                let epoch = slot.elect_writer(&mut state, &entry);
                #[cfg(test)]
                self.run_attach_probe_for_test();
                let Some(reader) = state.reader.as_ref().cloned() else {
                    panic!("BUG: active pending resource lost its reader");
                };
                let peer_waker = state.current_peer_waker();
                drop(state);
                drop(occupied);
                (slot, epoch, reader, peer_waker)
            }
            Entry::Vacant(vacant) => {
                let mut writer = match acquire()? {
                    AcquisitionResult::Pending(writer) => writer,
                    AcquisitionResult::Ready(reader) => {
                        return Ok(AcquisitionResult::Ready(reader));
                    }
                };
                writer.transfer_cleanup();
                let slot = Arc::new(PendingResource::new(
                    self.inner.cancel.child(),
                    Arc::clone(&entry),
                    writer,
                    remove,
                ));
                let (epoch, reader) = {
                    let mut state = slot.state.lock();
                    let epoch = slot.elect_writer(&mut state, &entry);
                    let Some(reader) = state.reader.as_ref().cloned() else {
                        panic!("BUG: new pending resource lost its reader");
                    };
                    drop(state);
                    (epoch, reader)
                };
                vacant.insert(Arc::clone(&slot));
                (slot, epoch, reader, None)
            }
        };

        if let Some(waker) = peer_waker {
            waker.wake();
        }
        let session = PendingResourceSession::new(&self.inner, key, &slot);
        let lease = ResourceLease::new(entry, session, store);
        let writer = epoch.map(|claim| lease.writer(claim));
        Ok(AcquisitionResult::Pending(ResourceAttachment {
            reader,
            lease,
            writer,
        }))
    }

    #[cfg(test)]
    pub(crate) fn has_slot_for_test(&self, key: &ResourceKey) -> bool {
        self.inner.slots.contains_key(key)
    }

    #[cfg(test)]
    fn run_attach_probe_for_test(&self) {
        let probe = self.inner.attach_probe.lock().take();
        if let Some(probe) = probe {
            probe();
        }
    }

    #[cfg(test)]
    fn set_attach_probe_for_test(&self, probe: impl Fn() + Send + Sync + 'static) {
        *self.inner.attach_probe.lock() = Some(Arc::new(probe));
    }

    #[cfg(test)]
    fn slot_locked_for_test(&self, key: &ResourceKey) -> bool {
        use dashmap::try_result::TryResult;

        matches!(self.inner.slots.try_get(key), TryResult::Locked)
    }
}

impl<S> fmt::Debug for PendingResourceIndex<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingResourceIndex")
            .field("tracked_resources", &self.inner.slots.len())
            .finish()
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests;
