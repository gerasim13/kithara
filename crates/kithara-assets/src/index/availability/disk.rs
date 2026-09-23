#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, HashMap},
    fs::OpenOptions,
    path::PathBuf,
};

use arc_swap::ArcSwap;
use kithara_bufpool::ByteBuffer;
use kithara_platform::sync::{Arc, Mutex};
use kithara_storage::StorageError;
use rkyv::rancor::Error;

use super::core::{Availability, AvailabilityIndex, Entry, InnerIndex};
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

        let mut tree = HashMap::clone(&self.inner.assets.load());
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

            tree.insert(root.as_str().to_string(), Arc::new(asset_map));
        }
        self.inner.assets.store(Arc::new(tree));
        Ok(())
    }

    /// Persist the aggregate index to a caller-supplied index file. Used by the cross-instance roundtrip tests; the
    /// production flush path goes through [`super::Flushable::flush`].
    #[cfg(test)]
    pub(crate) fn persist_to(&self, file: &IndexFile) -> AssetsResult<()> {
        write_aggregate(&self.inner, file, false)
    }
}

impl InnerIndex {
    /// `sync_data` every committed file queued since the last flush. A file
    /// that vanished (evicted, or its asset deleted) simply drops out — the
    /// manifest entry goes with it.
    fn barrier_pending_files(&self) {
        let queued: Vec<PathBuf> = self
            .pending_durability
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for path in queued {
            self.pending_durability.remove(&path);
            let synced = OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|file| file.sync_data());
            if let Err(error) = synced {
                tracing::debug!(?path, %error, "availability: durability barrier skipped");
            }
        }
    }

    pub(super) fn flush_with_durability(&self, durable: bool) -> AssetsResult<()> {
        let Some(p) = self.persist.get() else {
            return Ok(());
        };
        // WHY: Order is the whole point: force the committed files onto the medium first, then name them. Reversed, a crash could leave the
        // manifest vouching for bytes that never landed.
        let _writing = p.writing.lock();
        self.barrier_pending_files();
        write_aggregate(self, &p.file, durable)?;
        Ok(())
    }
}

/// Serialise the aggregate and publish it as `file`.
fn write_aggregate(inner: &InnerIndex, file: &IndexFile, durable: bool) -> AssetsResult<()> {
    let tree = inner.assets.load();
    let assets = tree
        .iter()
        .filter_map(|(root, asset)| {
            let resources: BTreeMap<String, ResourceAvailabilityFile> = asset
                .iter()
                .filter_map(|(path, entry)| {
                    let avail = entry.load();
                    // WHY: The crash-recovery snapshot is a COMMITTED-only contract: an uncommitted partial write (whose `.tmp` was never renamed) must
                    // be invisible after a rebuild, matching the aggregate probes' verdict.
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
    let index = AvailabilityFile { assets, version: 1 };
    let bytes = rkyv::to_bytes::<Error>(&index)
        .map_err(|e| AssetsError::Storage(StorageError::Failed(e.to_string())))?;
    file.write(&bytes, durable)
}
