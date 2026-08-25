use std::{fmt::Write as _, io, path::PathBuf, sync::Arc};

use kithara::{
    assets::{
        AssetResourceState, AssetSource, AssetStore, AssetsError, ResourceKey, StorageBackend,
    },
    bufpool::{BudgetExhausted, BytePool, PcmPool},
    platform::tokio::task,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use thiserror::Error;

mod analysis_cache;
mod library;

pub use analysis_cache::{
    CachedTrackAnalysis, SyncAnalysisFixtures, analysis_map, unavailable_analysis_map,
};
pub use library::{
    RepositoryMp3, SyncFixturePair, SyncTrackFixture, library_pair_from_env, repository_mp3,
    repository_mp3_pair, silvercomet_hls,
};

const RESOURCE_ROOT_MAGIC: &[u8] = b"kithara-sync-fixture-resources\0";

/// Explicit store and buffer-pool owner shared by one sync fixture graph.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SyncFixtureResources {
    inner: Arc<SyncFixtureResourcesInner>,
}

#[derive(Debug)]
struct SyncFixtureResourcesInner {
    store: AssetStore,
    byte_pool: BytePool,
    pcm_pool: PcmPool,
    _root: TempDir,
}

impl SyncFixtureResources {
    /// Creates the shared resource owner for a stable fixture case.
    ///
    /// # Errors
    ///
    /// Returns an error when `case_key` is empty or a private store directory
    /// cannot be created.
    pub fn new(case_key: &str, byte_pool: BytePool, pcm_pool: PcmPool) -> SyncFixtureResult<Self> {
        if case_key.is_empty() {
            return Err(SyncFixtureError::InvalidConfig(
                "the sync fixture resource case key must not be empty".to_owned(),
            ));
        }

        let prefix = format!("kithara-sync-{}-", hex_digest(&resource_root_key(case_key)));
        let root = tempfile::Builder::new()
            .prefix(&prefix)
            .tempdir()
            .map_err(|error| SyncFixtureError::ResourceRoot {
                case_key: case_key.to_owned(),
                error,
            })?;
        let store = AssetStore::builder()
            .backend(StorageBackend::Disk {
                root: root.path().to_path_buf(),
            })
            .pool(byte_pool.clone())
            .build();

        Ok(Self {
            inner: Arc::new(SyncFixtureResourcesInner {
                store,
                byte_pool,
                pcm_pool,
                _root: root,
            }),
        })
    }

    /// Returns the asset store owned by this fixture graph.
    #[must_use]
    pub fn store(&self) -> &AssetStore {
        &self.inner.store
    }

    /// Returns the byte pool owned by this fixture graph.
    #[must_use]
    pub fn byte_pool(&self) -> &BytePool {
        &self.inner.byte_pool
    }

    /// Returns the PCM pool owned by this fixture graph.
    #[must_use]
    pub fn pcm_pool(&self) -> &PcmPool {
        &self.inner.pcm_pool
    }
}

pub type SyncFixtureResult<T> = Result<T, SyncFixtureError>;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SyncFixtureError {
    #[error("invalid sync fixture configuration: {0}")]
    InvalidConfig(String),
    #[error("{operation} '{path}' failed: {error}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        error: io::Error,
    },
    #[error("{operation} '{path}' through the asset store failed: {error}")]
    Asset {
        operation: &'static str,
        path: PathBuf,
        #[source]
        error: Box<AssetsError>,
    },
    #[error("{operation} for asset source {source:?} failed: {error}")]
    AssetScope {
        operation: &'static str,
        source: Box<AssetSource>,
        #[source]
        error: Box<AssetsError>,
    },
    #[error("sync analysis asset {operation} for {resource:?} failed: {error}")]
    AnalysisAsset {
        operation: &'static str,
        resource: ResourceKey,
        #[source]
        error: Box<AssetsError>,
    },
    #[error("sync analysis asset {resource:?} is in an invalid cache state: {state:?}")]
    AnalysisAssetState {
        resource: ResourceKey,
        state: AssetResourceState,
    },
    #[error("sync analysis asset {resource:?} has no committed payload length")]
    AnalysisAssetLengthUnknown { resource: ResourceKey },
    #[error("sync analysis asset {resource:?} payload has {len} bytes; maximum is {max}")]
    AnalysisAssetTooLarge {
        resource: ResourceKey,
        len: u64,
        max: usize,
    },
    #[error("sync analysis asset {resource:?} ended after {actual} bytes; expected {expected}")]
    AnalysisAssetTruncated {
        resource: ResourceKey,
        expected: usize,
        actual: usize,
    },
    #[error("sync analysis asset {resource:?} cannot reserve a {len}-byte pooled buffer")]
    AnalysisAssetBuffer {
        resource: ResourceKey,
        len: usize,
        #[source]
        error: BudgetExhausted,
    },
    #[error("create private sync fixture store for case '{case_key}' failed: {error}")]
    ResourceRoot {
        case_key: String,
        #[source]
        error: io::Error,
    },
    #[error("lock sync analysis cache key {key} failed: {error}")]
    AnalysisLock {
        key: String,
        #[source]
        error: io::Error,
    },
    #[error("sync fixture blocking task '{operation}' failed: {detail}")]
    Blocking {
        operation: &'static str,
        detail: String,
    },
    #[error("track analysis for '{media}' is incomplete: {detail}")]
    IncompleteAnalysis { media: String, detail: String },
    #[error("sync analysis cache is corrupt: {0}")]
    CorruptCache(String),
    #[error("sync analysis cache failed to persist key {0}")]
    CacheStore(String),
}

async fn blocking<T, F>(operation: &'static str, work: F) -> SyncFixtureResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> SyncFixtureResult<T> + Send + 'static,
{
    task::spawn_blocking(work)
        .await
        .map_err(|error| SyncFixtureError::Blocking {
            operation,
            detail: error.to_string(),
        })?
}

fn resource_root_key(case_key: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(RESOURCE_ROOT_MAGIC);
    hasher.update(case_key.as_bytes());
    hasher.finalize().into()
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
fn test_resources(case_key: &str) -> SyncFixtureResources {
    SyncFixtureResources::new(case_key, BytePool::new(8, 0), PcmPool::new(8, 0))
        .expect("create sync fixture resources")
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn empty_resource_case_key_is_rejected() {
        let error = SyncFixtureResources::new("", BytePool::new(8, 0), PcmPool::new(8, 0))
            .expect_err("empty case key must fail");

        assert!(matches!(error, SyncFixtureError::InvalidConfig(_)));
    }

    #[kithara::test]
    fn private_store_root_lives_until_the_last_owner_drops() {
        let resources = test_resources("sync-fixture-unit-private-root-lifetime");
        let root = resources.inner._root.path().to_path_buf();
        let shared = resources.clone();

        drop(resources);
        assert!(root.is_dir());
        drop(shared);
        assert!(!root.exists());
    }
}
