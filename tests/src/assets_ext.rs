#[cfg(not(target_arch = "wasm32"))]
use std::{io, path::PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use kithara::assets::{AcquisitionResult, AssetReader, AssetsError, AssetsResult, WriteSide};
use kithara::assets::{AssetResourceState, AssetStore, ResourceKey, StorageBackend};

#[cfg(not(target_arch = "wasm32"))]
pub fn disk_asset_store(root: impl Into<PathBuf>) -> AssetStore {
    AssetStore::builder()
        .backend(StorageBackend::Disk { root: root.into() })
        .build()
}

pub fn memory_asset_store() -> AssetStore {
    AssetStore::builder()
        .backend(StorageBackend::Memory)
        .build()
}

/// Publish one previously missing resource's complete immutable payload.
///
/// # Errors
/// Returns an asset-store error when acquisition, writing, commit, or checkpoint fails.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_new_resource(
    store: &AssetStore,
    key: &ResourceKey,
    bytes: &[u8],
) -> AssetsResult<AssetReader> {
    let AcquisitionResult::Pending(writer) = store.acquire_resource(key, None)? else {
        return Err(AssetsError::Io(io::Error::other(
            "immutable asset resource was not missing after its transaction recheck",
        )));
    };
    writer.write_at(0, bytes).map_err(AssetsError::from)?;
    let final_len = u64::try_from(bytes.len()).map_err(|_| {
        AssetsError::Io(io::Error::other(
            "asset resource payload length exceeds u64",
        ))
    })?;
    let reader = writer.commit(Some(final_len)).map_err(AssetsError::from)?;
    store.checkpoint()?;
    Ok(reader)
}

/// Test-only convenience over the public [`AssetStore::resource_state`]:
/// "is there a committed resource for this key?". Production code inspects
/// the full [`AssetResourceState`] directly, so this committed-only shortcut
/// lives in the test harness.
pub trait AssetStoreTestExt {
    /// `true` when the key resolves to a committed resource.
    fn has_resource(&self, key: &ResourceKey) -> bool;
}

impl AssetStoreTestExt for AssetStore {
    fn has_resource(&self, key: &ResourceKey) -> bool {
        matches!(
            self.resource_state(key),
            Ok(AssetResourceState::Committed { .. })
        )
    }
}
