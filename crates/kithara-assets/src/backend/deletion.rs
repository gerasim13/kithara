#![forbid(unsafe_code)]

use crate::{error::AssetsResult, layout::ResourceKey};

/// Trait implemented by the disk and mem backends to expose a single
/// canonical removal channel. See module docs for the contract.
pub(crate) trait AssetDeleter: Send + Sync + std::fmt::Debug {
    /// Remove an asset's resources and invalidate its aggregate availability.
    /// Disk removes the root directory; memory removes only its own resources and
    /// invalidates foreign-root index entries without reaching another backend's handles.
    fn delete_asset(&self, asset_root: &str) -> AssetsResult<()>;

    /// Remove a single resource identified by `key`.
    ///
    /// Disk impl: `fs::remove_file` of the resolved path, then
    /// `availability.remove(key)`.
    ///
    /// Mem impl: drop the matching `active_resources` entry, then
    /// `availability.remove(key)`.
    fn remove_resource(&self, key: &ResourceKey) -> AssetsResult<()>;
}
