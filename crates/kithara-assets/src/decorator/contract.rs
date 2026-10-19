#![forbid(unsafe_code)]

use std::{fmt::Debug, path::Path};

use crate::{
    error::AssetsResult,
    layout::ResourceKey,
    resource::{AcquisitionResult, AssetResourceState, ReadSide, RequestIdentity, WriteSide},
};

bitflags::bitflags! {
    /// Decorator capabilities advertised by a base store.
    ///
    /// `EvictAssets` and `LeaseAssets` check their bit before activating their
    /// logic; when the bit is absent they pass through to the inner layer. The
    /// handle cache and processing have no bit: they run on every store.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Capabilities: u8 {
        /// LRU eviction with per-asset byte accounting (`EvictAssets`).
        const EVICT = 0b0001;
        /// RAII pin/lease semantics (`LeaseAssets`).
        const LEASE = 0b0010;
    }
}

/// Explicit public contract for the assets abstraction.
///
/// Acquisition is **phase-typed**: `acquire_resource*` hands back an
/// [`AcquisitionResult`] — a `Pending` [`WriteSide`] writer that must `commit` before
/// any read, or a `Ready` [`ReadSide`] reader when the resource is already committed.
/// `open_resource*` always returns a `Ready` reader. The decrypt-readiness gate is
/// carried in these types, not behind a runtime `is_readable()` probe; storage
/// lifecycle `status()` stays a runtime facade on the reader.
pub trait Assets: Clone + Send + Sync + 'static {
    /// Writer (Pending) phase returned by `acquire_resource*`.
    type ActiveRes: WriteSide<Reader = Self::ReadyRes>;
    /// Context type for resource processing. Use `()` for no context.
    type Context: Clone + Send + Sync + Debug + 'static;
    /// Reader (Ready) phase returned by `open_resource*` and by the `Ready`
    /// arm of `acquire_resource*`.
    type ReadyRes: ReadSide<Writer = Self::ActiveRes>;

    /// Acquire a resource for mutation (no identity, no context).
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    fn acquire_resource(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
    ) -> AssetsResult<AcquisitionResult<Self::ActiveRes, Self::ReadyRes>> {
        self.acquire_resource_with_ctx(key, identity, None)
    }

    /// Acquire a resource for mutation with optional processing context.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    fn acquire_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<Self::Context>,
    ) -> AssetsResult<AcquisitionResult<Self::ActiveRes, Self::ReadyRes>>;

    /// Decorator capabilities supported by this backend.
    #[must_use]
    fn capabilities(&self) -> Capabilities {
        Capabilities::all()
    }

    /// Delete the entire asset (all resources under `asset_root`).
    ///
    /// # Errors
    /// Returns `AssetsError` if the asset directory cannot be removed.
    fn delete_asset(&self, asset_root: &str) -> AssetsResult<()>;

    /// Open a resource for read (no context).
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    fn open_resource(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
    ) -> AssetsResult<Self::ReadyRes> {
        self.open_resource_with_ctx(key, identity, None)
    }

    /// Open a resource for read with optional processing context.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be opened.
    fn open_resource_with_ctx(
        &self,
        key: &ResourceKey,
        identity: Option<&RequestIdentity>,
        ctx: Option<Self::Context>,
    ) -> AssetsResult<Self::ReadyRes>;

    /// Remove a single resource by `key`.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be removed.
    fn remove_resource(&self, _key: &ResourceKey) -> AssetsResult<()> {
        Ok(())
    }

    /// Inspect the current resource state without creating or mutating it.
    ///
    /// # Errors
    /// Returns `AssetsError` if the resource cannot be inspected.
    fn resource_state(&self, key: &ResourceKey) -> AssetsResult<AssetResourceState>;

    /// Return the root directory for disk-backed implementations.
    fn root_dir(&self) -> &Path;
}
