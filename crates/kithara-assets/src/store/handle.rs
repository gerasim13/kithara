#![forbid(unsafe_code)]

use std::{
    fmt, future::Future, num::NonZeroUsize, ops::Range, path::Path, sync::atomic::AtomicU64,
};

use kithara_bufpool::HasPool;
use kithara_platform::{sync::Arc, tokio::sync::mpsc};
use kithara_storage::StorageError;
use rangemap::RangeSet;

#[cfg(not(target_arch = "wasm32"))]
use super::DiskStore;
use super::{AssetReader, MemStore, ResourceAcquisition};
#[cfg(not(target_arch = "wasm32"))]
use crate::backend::DiskAssetStore;
#[cfg(test)]
use crate::decorator::Capabilities;
use crate::{
    decorator::{Assets, EvictionRouter, EvictionSubscription, ProcessCtx},
    error::{AssetsError, AssetsResult},
    index::{
        AvailabilityIndex, DemandEntry, PendingResourceIndex, RemoveResource,
        ResourceTransactionIndex, pending_resource::ResourceAttachment,
    },
    layout::{AssetLayoutRegistry, AssetScope, AssetSource, ResourceKey},
    resource::{AcquisitionResult, AssetResourceState, RequestIdentity},
};

/// Forward a method call to the active store variant. Keeps the
/// `#[cfg(not(target_arch = "wasm32"))]` gate on `Disk` in one place so
/// the enum arms don't repeat it across a dozen trivial wrappers.
macro_rules! delegate_to_store {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match &$self.inner.backend {
            #[cfg(not(target_arch = "wasm32"))]
            StoreBackendInner::Disk { store, .. } => store.$method($($arg),*),
            StoreBackendInner::Memory { store } => store.$method($($arg),*),
        }
    };
}

/// Cheap shared handle for one asset-store identity.
pub struct AssetStore<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    inner: Arc<AssetStoreInner<S>>,
}

pub(super) struct AssetStoreInner<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) layouts: AssetLayoutRegistry,
    pub(super) availability: AvailabilityIndex,
    pub(super) eviction: EvictionRouter,
    pub(super) pending_resources: PendingResourceIndex<S>,
    pub(super) transactions: ResourceTransactionIndex,
    pub(super) backend: StoreBackendInner<S>,
}

pub(super) enum StoreBackendInner<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    #[cfg(not(target_arch = "wasm32"))]
    Disk {
        store: DiskStore<S>,
        base: Option<Arc<DiskAssetStore>>,
    },
    Memory {
        store: MemStore<S>,
    },
}

impl<S> Clone for AssetStore<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> fmt::Debug for AssetStore<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssetStore").finish_non_exhaustive()
    }
}

impl<S> AssetStore<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    /// Acquire a resource explicitly for mutation.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    pub fn acquire_resource(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
    ) -> AssetsResult<ResourceAcquisition<S>> {
        delegate_to_store!(self, acquire_resource, key, identity)
    }

    /// Acquire a resource with processing context for an explicit write path.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    pub fn acquire_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<ProcessCtx>,
    ) -> AssetsResult<ResourceAcquisition<S>> {
        delegate_to_store!(self, acquire_resource_with_ctx, key, identity, ctx)
    }

    /// Join or create the canonical pending resource acquisition for `key`.
    ///
    /// # Errors
    ///
    /// Returns an error when the resource cannot be acquired or a retired
    /// session could not finish removing its backing resource.
    #[doc(hidden)]
    pub fn attach_pending_resource(
        &self,
        key: &ResourceKey,
        read_pos: Arc<AtomicU64>,
        look_ahead: Option<u64>,
    ) -> AssetsResult<AcquisitionResult<ResourceAttachment<S>, AssetReader<S>>> {
        let entry = Arc::new(DemandEntry::new(read_pos, look_ahead));
        let weak = Arc::downgrade(&self.inner);
        let remove: RemoveResource = Arc::new(move |key| {
            let Some(inner) = weak.upgrade() else {
                return Err(AssetsError::Storage(StorageError::Failed(
                    "asset store closed before session cleanup".to_string(),
                )));
            };
            let store = Self { inner };
            store.remove_resource(key)
        });
        self.inner.pending_resources.attach_pending_resource(
            key,
            entry,
            self.clone(),
            remove,
            || self.acquire_resource(key, None),
        )
    }

    #[cfg(test)]
    pub(super) fn capabilities(&self) -> Capabilities {
        delegate_to_store!(self, capabilities)
    }

    /// Persist the in-memory byte-availability aggregate snapshot to
    /// disk. For an in-memory store this is a no-op.
    ///
    /// Callers can checkpoint at any point they want a consistent
    /// aggregate on disk; the store also checkpoints itself when the last
    /// handle drops, because the manifest is what makes a resource usable
    /// after a restart.
    ///
    /// # Errors
    ///
    /// Returns `AssetsError` if the persistent index resource cannot
    /// be opened or the atomic write fails.
    pub fn checkpoint(&self) -> AssetsResult<()> {
        match &self.inner.backend {
            #[cfg(not(target_arch = "wasm32"))]
            StoreBackendInner::Disk { base, .. } => {
                base.as_ref().map_or(Ok(()), |base| base.checkpoint())
            }
            StoreBackendInner::Memory { .. } => Ok(()),
        }
    }

    /// Delete the entire asset directory.
    ///
    /// # Errors
    /// Returns `AssetsError` if the directory cannot be removed.
    pub(crate) fn delete_asset(&self, asset_root: &str) -> AssetsResult<()> {
        delegate_to_store!(self, delete_asset, asset_root)
    }

    /// Return the fixed handle-cache capacity for an ephemeral memory store.
    /// Durable stores return `None` because handle displacement does not remove
    /// their committed bytes.
    #[must_use]
    pub fn ephemeral_cache_capacity(&self) -> Option<NonZeroUsize> {
        match &self.inner.backend {
            #[cfg(not(target_arch = "wasm32"))]
            StoreBackendInner::Disk { .. } => None,
            StoreBackendInner::Memory { store } => Some(store.cache_capacity()),
        }
    }

    /// Return whether both handles refer to the same store instance.
    #[must_use]
    pub fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub(super) fn new_handle(inner: AssetStoreInner<S>) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Open a resource by key (no processing context).
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    pub fn open_resource(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
    ) -> AssetsResult<AssetReader<S>> {
        delegate_to_store!(self, open_resource, key, identity)
    }

    /// Open a resource with processing context.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    pub fn open_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<ProcessCtx>,
    ) -> AssetsResult<AssetReader<S>> {
        delegate_to_store!(self, open_resource_with_ctx, key, identity, ctx)
    }

    /// Remove a single resource from the store. The concrete store
    /// dispatches through the canonical asset deleter
    /// channel, which atomically clears the matching
    /// [`AvailabilityIndex`](crate::index) entry — so this method
    /// must not invalidate the index again.
    ///
    /// # Errors
    /// Returns `AssetsError` if the backing resource cannot be removed.
    pub fn remove_resource(&self, key: &ResourceKey) -> AssetsResult<()> {
        if key.is_absolute() {
            return Err(AssetsError::InvalidKey);
        }
        delegate_to_store!(self, remove_resource, key)
    }

    /// Inspect the current resource state.
    ///
    /// # Errors
    /// Returns `AssetsError` if the key is invalid or the backend cannot inspect.
    pub fn resource_state(&self, key: &ResourceKey) -> AssetsResult<AssetResourceState> {
        delegate_to_store!(self, resource_state, key)
    }

    /// Return the root directory for the asset store.
    #[must_use]
    pub fn root_dir(&self) -> &Path {
        delegate_to_store!(self, root_dir)
    }

    /// Bind `source` to the layout registered for marker `T`.
    ///
    /// # Errors
    /// Returns an error when the source or layout-owned root is invalid.
    pub fn scope<T: 'static>(&self, source: &AssetSource) -> AssetsResult<AssetScope<S>> {
        let layout = Arc::clone(self.inner.layouts.layout::<T>());
        AssetScope::new(self.clone(), source, layout)
    }

    /// Subscribe to evictions under `asset_root`.
    ///
    /// When a [`ResourceKey`] under `asset_root` is invalidated, the evicted key is sent on `tx`.
    /// Every subscriber for that root receives the key. The returned
    /// [`EvictionSubscription`] guard deregisters only its own subscription on drop.
    pub fn subscribe_eviction(
        &self,
        asset_root: Arc<str>,
        tx: mpsc::UnboundedSender<ResourceKey>,
    ) -> EvictionSubscription {
        self.inner.eviction.subscribe(asset_root, tx)
    }

    /// Serialize a closure per key across clones of this store. The closure
    /// must re-read state inside; separate stores are not coordinated. Waiting
    /// and running operations release the transaction when cancelled.
    /// Transactions are not reentrant: an operation must not acquire the same
    /// key again through this store.
    pub async fn with_resource_transaction<T, F, Fut>(&self, key: &ResourceKey, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.inner.transactions.run(key, operation).await
    }

    delegate::delegate! {
        to self.inner.availability {
            /// Byte ranges known to the availability aggregate for this resource.
            #[must_use]
            pub fn available_ranges(&self, key: &ResourceKey) -> RangeSet<u64>;
            /// Return `true` when every byte in `range` is already present for
            /// the resource, or when the range is empty. Aggregate-only: no
            /// locks, no filesystem.
            #[must_use]
            pub fn contains_range(&self, key: &ResourceKey, range: Range<u64>) -> bool;
            /// Committed final length per the availability aggregate, if known.
            #[must_use]
            pub fn final_len(&self, key: &ResourceKey) -> Option<u64>;
        }
    }
}

/// The manifest decides what survives a restart, so the last handle writes
/// it before the indexes go away. Waiting for the flush hub's own teardown
/// is too late: every index holds the hub alive, so by the time the hub
/// drops there is nothing left to flush.
impl<S> Drop for AssetStoreInner<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let StoreBackendInner::Disk {
            base: Some(base), ..
        } = &self.backend
            && let Err(error) = base.checkpoint()
        {
            tracing::warn!(%error, "AssetStore: final checkpoint failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use crate::{AssetStore, StorageBackend};

    #[kithara::test]
    fn clone_shares_one_inner_identity() {
        let store = AssetStore::builder(crate::test_pools::pools())
            .backend(StorageBackend::Memory)
            .build();
        let clone = store.clone();
        let other = AssetStore::builder(crate::test_pools::pools())
            .backend(StorageBackend::Memory)
            .build();

        assert!(store.is_same(&clone));
        assert!(!store.is_same(&other));
    }
}
