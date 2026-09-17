#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use dashmap::DashMap;
use kithara_bufpool::ByteBuffer;
use kithara_platform::sync::{Arc, Mutex};
use kithara_storage::StorageError;
use rkyv::rancor::Error;

use super::core::{PinCounts, PinsIndex, PinsInner};
use crate::{
    error::{AssetsError, AssetsResult},
    index::persistence::{IndexFile, schema::PinsIndexFile},
};

pub(super) struct PinsPersist {
    /// One writer at a time for `pins.bin`: the snapshot and the atomic
    /// rename that publishes it are one step.
    writing: Mutex<()>,
    file: IndexFile,
}

impl PinsIndex {
    /// Construct a disk-backed index rooted at `path`.
    ///
    /// An existing file is read and hydrated synchronously. Otherwise the
    /// disk file is **not** materialised — it appears the first time
    /// [`PinsIndex::add`] or [`PinsIndex::remove`] flush a real change.
    #[must_use]
    pub fn with_persist_at(path: PathBuf, mut buffer: ByteBuffer) -> Self {
        let file = IndexFile::new(path);
        let initial = read_pins(&file, &mut buffer).unwrap_or_else(|e| {
            tracing::debug!("read existing pins.bin failed: {e}");
            DashMap::new()
        });
        Self {
            inner: Arc::new(PinsInner {
                pins: initial,
                persist: Some(PinsPersist {
                    writing: Mutex::new(()),
                    file,
                }),
                hub: OnceLock::new(),
                dirty: AtomicBool::new(false),
            }),
        }
    }
}

impl PinsInner {
    /// Roots held by at least one durable pin — the set that belongs on disk.
    fn durable_roots(&self) -> Vec<String> {
        self.pins
            .iter()
            .filter(|r| r.value().durable > 0)
            .map(|r| r.key().clone())
            .collect()
    }

    pub(super) fn flush_with_durability(&self, durable: bool) -> AssetsResult<()> {
        let Some(persist) = self.persist.as_ref() else {
            self.dirty.store(false, Ordering::Release);
            return Ok(());
        };
        let _writing = persist.writing.lock();
        let snapshot = self.durable_roots();
        write_pins(&persist.file, &snapshot, durable)?;
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }
}

/// Counts for a root read back from `pins.bin`: whoever set that pin is gone,
/// so the surviving claim is a single durable one.
const fn hydrated_counts() -> PinCounts {
    PinCounts {
        durable: 1,
        local: 0,
    }
}

fn read_pins(file: &IndexFile, buf: &mut ByteBuffer) -> AssetsResult<DashMap<String, PinCounts>> {
    file.read_into(buf)?;
    if buf.is_empty() {
        return Ok(DashMap::new());
    }

    let archived = match rkyv::access::<crate::index::schema::ArchivedPinsIndexFile, Error>(buf) {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!("Failed to validate pins index: {}", e);
            return Ok(DashMap::new());
        }
    };

    let pinned = archived
        .pinned
        .iter()
        .filter(|(_, v)| **v)
        .map(|(k, _)| (k.as_str().to_string(), hydrated_counts()))
        .collect();
    Ok(pinned)
}

fn write_pins(file: &IndexFile, pins: &[String], durable: bool) -> AssetsResult<()> {
    let mut map = BTreeMap::new();
    for pin in pins {
        map.insert(pin.clone(), true);
    }
    let index = PinsIndexFile {
        version: 1,
        pinned: map,
    };

    let bytes = rkyv::to_bytes::<Error>(&index)
        .map_err(|e| AssetsError::Storage(StorageError::Failed(e.to_string())))?;
    file.write(&bytes, durable)
}
