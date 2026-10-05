#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, HashMap},
    fs::OpenOptions,
    io::ErrorKind,
    path::PathBuf,
};

use arc_swap::ArcSwap;
use kithara_bufpool::ByteBuffer;
use kithara_platform::sync::{Arc, Mutex};
use kithara_storage::StorageError;
use rkyv::rancor::Error;

use super::core::{AssetTree, Availability, AvailabilityIndex, Entry, InnerIndex};
use crate::{
    error::{AssetsError, AssetsResult},
    index::persistence::{
        IndexFile,
        schema::{AssetAvailabilityFile, AvailabilityFile, ResourceAvailabilityFile},
    },
};

pub(super) struct AvailabilityPersist {
    file: IndexFile,
    /// One writer at a time for `availability.bin`: the snapshot and the
    /// atomic rename that publishes it are one step.
    writing: Mutex<()>,
}

impl AvailabilityIndex {
    /// Enable disk persistence rooted at `path`. Hydrates the in-memory
    /// aggregate from the existing on-disk snapshot (if any). Idempotent.
    ///
    /// A failed load collapses silently — the aggregate stays empty and the
    /// file is replaced on the first flush.
    pub(crate) fn enable_persistence(&self, path: PathBuf, mut buffer: ByteBuffer) {
        let file = IndexFile::new(path);
        if let Err(e) = self.load_from(&file, &mut buffer) {
            tracing::debug!("read existing availability.bin failed: {e}");
        }
        let _ = self.inner.persist.set(AvailabilityPersist {
            file,
            writing: Mutex::new(()),
        });
    }

    /// Load the availability index from a persistent resource.
    pub(crate) fn load_from(&self, file: &IndexFile, buf: &mut ByteBuffer) -> AssetsResult<()> {
        file.read_into(buf)?;
        if buf.is_empty() {
            return Ok(());
        }

        let archived =
            match rkyv::access::<crate::index::schema::ArchivedAvailabilityFile, Error>(buf) {
                Ok(archived) => archived,
                Err(e) => {
                    tracing::debug!("Failed to validate availability index: {}", e);
                    return Ok(());
                }
            };

        let mut loaded = AssetTree::new();
        for (root, asset_record) in archived.assets.iter() {
            let mut asset_map = HashMap::new();

            for (path, res_record) in asset_record.resources.iter() {
                let mut avail = Availability::default();
                res_record.ranges.iter().for_each(|r| {
                    avail.insert(r.0.to_native()..r.1.to_native());
                });

                let final_len: Option<u64> = res_record.final_len.as_ref().map(|l| l.to_native());

                match final_len {
                    Some(flen) => {
                        avail.mark_committed(flen);
                    }
                    None => avail.committed = res_record.is_committed,
                }

                asset_map.insert(
                    path.as_str().to_string(),
                    Entry::new(ArcSwap::from_pointee(avail)),
                );
            }

            loaded.insert(root.as_str().to_string(), Arc::new(asset_map));
        }
        self.edit_tree(|tree| tree.extend(loaded.clone()));
        Ok(())
    }

    /// Persist the aggregate index to a caller-supplied index file. Used by the cross-instance roundtrip tests; the
    /// production flush path goes through [`super::Flushable::flush`].
    #[cfg(test)]
    pub(crate) fn persist_to(&self, file: &IndexFile) -> AssetsResult<()> {
        write_aggregate(&snapshot_aggregate(&self.inner), file, false)
    }
}

impl InnerIndex {
    /// Force the detached cohort before publishing its selected availability.
    /// Only a missing file whose key has already left both the selected and
    /// live aggregate is a completed deletion; every other barrier error
    /// preserves the obligations and prevents manifest publication.
    fn barrier_pending_files(&self, index: &AvailabilityFile) -> AssetsResult<()> {
        let paths: Vec<PathBuf> = self
            .pending_durability
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        let queued: Vec<_> = paths
            .into_iter()
            .filter_map(|path| self.pending_durability.remove(&path))
            .collect();
        for (path, key) in &queued {
            let synced = OpenOptions::new()
                .write(true)
                .open(path)
                .and_then(|file| file.sync_data());
            if let Err(error) = synced {
                let (root, relative) = AvailabilityIndex::resolve_refs(key);
                let selected = index
                    .assets
                    .get(root)
                    .is_some_and(|asset| asset.resources.contains_key(relative));
                let present = self
                    .assets
                    .load()
                    .get(root)
                    .is_some_and(|asset| asset.contains_key(relative));
                if error.kind() == ErrorKind::NotFound && !selected && !present {
                    continue;
                }
                for (path, key) in queued {
                    self.pending_durability.entry(path).or_insert(key);
                }
                return Err(error.into());
            }
        }
        Ok(())
    }

    /// Committed files are forced onto the medium before the manifest names them; reversed, a crash
    /// could leave the manifest vouching for bytes that never landed.
    pub(super) fn flush_with_durability(&self, durable: bool) -> AssetsResult<()> {
        let Some(p) = self.persist.get() else {
            return Ok(());
        };
        let _writing = p.writing.lock();
        let index = snapshot_aggregate(self);
        self.barrier_pending_files(&index)?;
        write_aggregate(&index, &p.file, durable)
    }
}

/// Freeze committed values before taking their durability cohort. Tree
/// guards alone are insufficient: each entry still holds a live `ArcSwap`.
fn snapshot_aggregate(inner: &InnerIndex) -> AvailabilityFile {
    let tree = inner.assets.load();
    let assets = tree
        .iter()
        .filter_map(|(root, asset)| {
            let resources: BTreeMap<String, ResourceAvailabilityFile> = asset
                .iter()
                .filter_map(|(path, entry)| {
                    let avail = entry.load();
                    if !avail.committed {
                        return None;
                    }
                    let record = ResourceAvailabilityFile {
                        ranges: avail.ranges.iter().map(|r| (r.start, r.end)).collect(),
                        final_len: avail.final_len,
                        is_committed: avail.committed,
                    };
                    Some((path.clone(), record))
                })
                .collect();
            if resources.is_empty() {
                return None;
            }
            Some((root.clone(), AssetAvailabilityFile { resources }))
        })
        .collect();
    AvailabilityFile { assets, version: 1 }
}

fn write_aggregate(index: &AvailabilityFile, file: &IndexFile, durable: bool) -> AssetsResult<()> {
    let bytes = rkyv::to_bytes::<Error>(index)
        .map_err(|e| AssetsError::Storage(StorageError::Failed(e.to_string())))?;
    file.write(&bytes, durable)
}

#[cfg(test)]
mod tests {
    use std::{fs, ops::Range};

    use kithara_platform::{CancelToken, time::Duration};
    use kithara_storage::{
        AtomicChunked, AvailabilityObserver, MmapDriver, MmapOptions, OpenIntent, OpenMode,
        Resource,
    };
    use kithara_test_utils::kithara;
    use tempfile::TempDir;

    use super::*;
    use crate::{index::ScopedAvailabilityObserver, layout::ResourceKey};

    struct CheckpointOnCommit {
        observer: Arc<ScopedAvailabilityObserver>,
        index: AvailabilityIndex,
        canonical: PathBuf,
    }

    impl AvailabilityObserver for CheckpointOnCommit {
        fn on_commit(&self, final_len: u64) {
            self.observer.on_commit(final_len);
            let flushed = self.index.flush();
            assert!(
                self.canonical.is_file(),
                "a commit observer may checkpoint only after the canonical file exists"
            );
            flushed.unwrap();
        }

        fn on_write(&self, range: Range<u64>) {
            self.observer.on_write(range);
        }
    }

    #[kithara::test(timeout(Duration::from_secs(5)))]
    fn atomic_commit_publishes_canonical_file_before_manifest() {
        let dir = TempDir::new().unwrap();
        let pools = crate::test_pools::pools();
        let manifest = dir.path().join("availability.bin");
        let canonical = dir.path().join("asset/segment.bin");
        let key = ResourceKey::relative("asset", "segment.bin");
        let index = AvailabilityIndex::new();
        index.enable_persistence(manifest.clone(), crate::test_pools::byte_buffer(&pools));
        let observer: Arc<dyn AvailabilityObserver> = Arc::new(CheckpointOnCommit {
            observer: ScopedAvailabilityObserver::for_file(
                key.clone(),
                index.clone(),
                canonical.clone(),
            ),
            index: index.clone(),
            canonical: canonical.clone(),
        });
        let resource =
            AtomicChunked::<MmapDriver>::open_deferred(canonical.clone(), move |target, intent| {
                let mode = match intent {
                    OpenIntent::Fresh => OpenMode::ReadWrite,
                    OpenIntent::Reopen => OpenMode::ReadOnly,
                };
                Resource::open_with_observer(
                    CancelToken::never(),
                    MmapOptions::for_path(target.to_path_buf())
                        .mode(mode)
                        .build(),
                    Some(Arc::clone(&observer)),
                )
            })
            .unwrap();
        resource.write_at(0, b"payload").unwrap();
        assert!(!canonical.exists());

        resource.commit(Some(7)).unwrap();

        assert_eq!(fs::read(&canonical).unwrap(), b"payload");
        let restored = AvailabilityIndex::new();
        restored
            .load_from(
                &IndexFile::new(manifest),
                &mut crate::test_pools::byte_buffer(&pools),
            )
            .unwrap();
        assert_eq!(restored.final_len(&key), Some(7));
        assert!(restored.contains_range(&key, 0..7));
    }

    #[kithara::test(timeout(Duration::from_secs(5)))]
    #[case::missing(false)]
    #[case::directory(true)]
    fn a_failed_file_barrier_preserves_the_previous_manifest(#[case] directory: bool) {
        let dir = TempDir::new().unwrap();
        let pools = crate::test_pools::pools();
        let manifest = dir.path().join("availability.bin");
        let canonical = dir.path().join("segment.bin");
        let key = ResourceKey::relative("asset", "segment.bin");
        let index = AvailabilityIndex::new();
        index.enable_persistence(manifest.clone(), crate::test_pools::byte_buffer(&pools));
        index.record_commit(&ResourceKey::relative("asset", "already-durable.bin"), 3);
        index.flush().unwrap();
        let previous = fs::read(&manifest).unwrap();
        if directory {
            fs::create_dir(&canonical).unwrap();
        }
        let observer =
            ScopedAvailabilityObserver::for_file(key.clone(), index.clone(), canonical.clone());
        observer.on_commit(7);

        assert!(matches!(index.flush(), Err(AssetsError::Io(_))));
        assert_eq!(fs::read(&manifest).unwrap(), previous);
        assert!(matches!(index.flush(), Err(AssetsError::Io(_))));
        if directory {
            fs::remove_dir(&canonical).unwrap();
        }
        fs::write(&canonical, b"payload").unwrap();
        index.flush().unwrap();

        let restored = AvailabilityIndex::new();
        restored
            .load_from(
                &IndexFile::new(manifest),
                &mut crate::test_pools::byte_buffer(&pools),
            )
            .unwrap();
        assert_eq!(restored.final_len(&key), Some(7));
    }

    #[kithara::test(timeout(Duration::from_secs(5)))]
    fn a_deleted_resource_has_no_manifest_barrier_obligation() {
        let dir = TempDir::new().unwrap();
        let pools = crate::test_pools::pools();
        let manifest = dir.path().join("availability.bin");
        let canonical = dir.path().join("segment.bin");
        let key = ResourceKey::relative("asset", "segment.bin");
        let index = AvailabilityIndex::new();
        index.enable_persistence(manifest.clone(), crate::test_pools::byte_buffer(&pools));
        fs::write(&canonical, b"payload").unwrap();
        ScopedAvailabilityObserver::for_file(key.clone(), index.clone(), canonical.clone())
            .on_commit(7);
        fs::remove_file(&canonical).unwrap();
        index.remove(&key);

        index.flush().unwrap();

        let restored = AvailabilityIndex::new();
        restored
            .load_from(
                &IndexFile::new(manifest),
                &mut crate::test_pools::byte_buffer(&pools),
            )
            .unwrap();
        assert_eq!(restored.final_len(&key), None);
    }

    #[kithara::test(timeout(Duration::from_secs(5)))]
    fn every_same_size_file_commit_requires_a_durability_barrier() {
        let dir = TempDir::new().unwrap();
        let pools = crate::test_pools::pools();
        let canonical = dir.path().join("segment.bin");
        let key = ResourceKey::relative("asset", "segment.bin");
        let index = AvailabilityIndex::new();
        index.enable_persistence(
            dir.path().join("availability.bin"),
            crate::test_pools::byte_buffer(&pools),
        );
        let observer = ScopedAvailabilityObserver::for_file(key, index.clone(), canonical.clone());
        fs::write(&canonical, b"first").unwrap();
        observer.on_commit(5);
        index.flush().unwrap();

        fs::write(&canonical, b"later").unwrap();
        observer.on_commit(5);
        fs::remove_file(&canonical).unwrap();

        assert!(matches!(index.flush(), Err(AssetsError::Io(_))));
    }
}
