#![forbid(unsafe_code)]

use std::{
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use kithara_bufpool::ByteBuffer;
use kithara_platform::sync::{Arc, Mutex};
use kithara_storage::StorageError;
use rkyv::rancor::Error;

use super::core::{LruIndex, LruInner, LruState};
use crate::{
    error::{AssetsError, AssetsResult},
    index::persistence::{IndexFile, schema::LruIndexFile},
};

pub(super) struct LruPersist {
    /// One writer at a time for `lru.bin`: the snapshot and the atomic
    /// rename that publishes it are one step.
    writing: Mutex<()>,
    file: IndexFile,
}

impl LruIndex {
    /// Construct a disk-backed index rooted at `path`.
    ///
    /// An existing file is read and hydrated synchronously. Otherwise the
    /// disk file is **not** materialised — it appears on the first
    /// [`LruIndex::touch`] or [`LruIndex::remove`].
    pub(crate) fn with_persist_at(path: PathBuf, mut buffer: ByteBuffer) -> Self {
        let file = IndexFile::new(path);
        let initial = read_state(&file, &mut buffer).unwrap_or_else(|e| {
            tracing::debug!("read existing lru.bin failed: {e}");
            LruState::default()
        });
        Self {
            inner: Arc::new(LruInner {
                state: Mutex::new(initial),
                persist: Some(LruPersist {
                    writing: Mutex::new(()),
                    file,
                }),
                hub: OnceLock::new(),
                dirty: AtomicBool::new(false),
            }),
        }
    }
}

impl LruInner {
    pub(super) fn flush_with_durability(&self, durable: bool) -> AssetsResult<()> {
        let Some(persist) = self.persist.as_ref() else {
            self.dirty.store(false, Ordering::Release);
            return Ok(());
        };
        let _writing = persist.writing.lock();
        let snapshot = self.state.lock().clone();
        write_state(&persist.file, &snapshot, durable)?;
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }
}

fn read_state(file: &IndexFile, buf: &mut ByteBuffer) -> AssetsResult<LruState> {
    file.read_into(buf)?;
    if buf.is_empty() {
        return Ok(LruState::default());
    }

    let index = match rkyv::access::<crate::index::schema::ArchivedLruIndexFile, Error>(buf) {
        Ok(archived) => rkyv::deserialize::<LruIndexFile, Error>(archived)
            .expect("BUG: LRU archived → owned deserialize"),
        Err(e) => {
            tracing::debug!("Failed to deserialize lru index: {}", e);
            return Ok(LruState::default());
        }
    };

    Ok(LruState::from(index))
}

fn write_state(file: &IndexFile, state: &LruState, durable: bool) -> AssetsResult<()> {
    let index = LruIndexFile::from(state);
    let bytes = rkyv::to_bytes::<Error>(&index)
        .map_err(|e| AssetsError::Storage(StorageError::Failed(e.to_string())))?;
    file.write(&bytes, durable)
}
