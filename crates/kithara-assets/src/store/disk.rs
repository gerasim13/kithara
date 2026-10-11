#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use kithara_bufpool::{ByteBuffer, HasPool, PoolRegion};
use kithara_events::EventBus;
use kithara_platform::{CancelScope, CancelToken, sync::Arc};

use super::{AssetStore, builder::AssetStoreConfig, handle::StoreBackendInner};
use crate::{
    backend::{AssetDeleter, DiskAssetDeleter, DiskAssetStore, indexed_path},
    decorator::{EvictDeps, EvictionEvents},
    index::{AvailabilityIndex, FlushHub, FlushPolicy},
};

/// Everything [`AssetStore::open`] resolved before it picked the disk
/// branch. Mirrors [`crate::backend::MemStoreSetup`] on the memory side: one bundle so the
/// branch is a function instead of another sixty lines in the builder.
pub(super) struct DiskStoreSetup<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) config: Arc<AssetStoreConfig<S>>,
    pub(super) availability: AvailabilityIndex,
    pub(super) cancel: Option<CancelToken>,
    pub(super) event_bus: Option<EventBus>,
    pub(super) flush_hub: Option<Arc<FlushHub>>,
    pub(super) root_dir: PathBuf,
    pub(super) pools: PoolRegion<S>,
}

/// Assemble the disk decorator chain: evict over the disk store, processing
/// over that, the memory cache over that, leases on top.
///
/// Disk bytes survive LRU displacement, so the disk store needs no invalidation hook, unlike memory
/// bytes.
pub(super) fn open_disk_backend<S>(setup: DiskStoreSetup<S>) -> StoreBackendInner<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    let DiskStoreSetup {
        config,
        root_dir,
        cancel,
        flush_hub,
        pools,
        event_bus,
        availability,
    } = setup;
    let cancel = CancelScope::new(cancel).token();
    let hub = flush_hub.unwrap_or_else(|| FlushHub::new(cancel.child(), FlushPolicy::default()));

    let pins = open_disk_pins_index(&root_dir, pools.get::<u8>());
    let lru = open_disk_lru_index(&root_dir, pools.get::<u8>());
    pins.attach_to(&hub);
    lru.attach_to(&hub);

    let deleter: Arc<dyn AssetDeleter> = Arc::new(DiskAssetDeleter::new(
        root_dir.clone(),
        availability.clone(),
        pins.clone(),
        lru.clone(),
    ));

    if let Some(path) = lazy_index_path(&root_dir, "availability.bin") {
        availability.enable_persistence(path, pools.get::<u8>());
        availability
            .retain(|root, rel| indexed_path(&root_dir, root, rel).is_some_and(|p| p.exists()));
    }
    availability.attach_to(&hub);

    let disk = Arc::new(DiskAssetStore::with_config(
        root_dir,
        cancel.clone(),
        availability,
        Arc::clone(&deleter),
        Arc::clone(&config),
    ));
    let base = Arc::clone(&disk);
    let store = AssetStore::<S>::decorate_backend(
        disk,
        EvictDeps {
            lru,
            deleter,
            config,
            cancel,
            events: EvictionEvents::new(event_bus),
            pins,
        },
        None,
        false,
    );

    StoreBackendInner::Disk {
        store,
        base: Some(base),
    }
}

/// Unique throwaway disk root used when the builder gets no backend.
pub(super) fn fresh_temp_root() -> PathBuf {
    tempfile::tempdir()
        .expect("BUG: failed to create AssetStore temp dir")
        .keep()
}

/// Open `_index/pins.bin` as a disk-backed [`crate::index::PinsIndex`]; on path
/// failure falls back to an ephemeral index (best-effort, lazily materialised).
fn open_disk_pins_index(root_dir: &Path, buffer: ByteBuffer) -> crate::index::PinsIndex {
    let Some(path) = lazy_index_path(root_dir, "pins.bin") else {
        return crate::index::PinsIndex::ephemeral();
    };
    crate::index::PinsIndex::with_persist_at(path, buffer)
}

/// Open `_index/lru.bin` as a disk-backed [`crate::index::LruIndex`].
/// Same fallback policy and lazy-materialisation contract as
/// [`open_disk_pins_index`].
fn open_disk_lru_index(root_dir: &Path, buffer: ByteBuffer) -> crate::index::LruIndex {
    let Some(path) = lazy_index_path(root_dir, "lru.bin") else {
        return crate::index::LruIndex::ephemeral();
    };
    crate::index::LruIndex::with_persist_at(path, buffer)
}

/// Build the `root_dir/_index/<name>` path; `None` if the parent dir can't be
/// created (caller falls back to an ephemeral index).
fn lazy_index_path(root_dir: &Path, name: &str) -> Option<PathBuf> {
    let path = root_dir.join("_index").join(name);
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::debug!("create _index dir failed: {e}");
        return None;
    }
    Some(path)
}
