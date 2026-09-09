#![forbid(unsafe_code)]

use std::{
    fs,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use kithara_bufpool::ByteBuffer;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
};
use kithara_storage::{Atomic, MmapDriver, StorageError};
use rkyv::rancor::Error;

use super::core::{LruIndex, LruInner, LruState};
use crate::{
    error::{AssetsError, AssetsResult},
    index::persistence::{init_atomic, open_existing, schema::LruIndexFile},
};

pub(super) struct LruPersist {
    cancel: CancelToken,
    /// One writer at a time for `lru.bin`: the snapshot and the atomic
    /// rename that publishes it are one step.
    writing: Mutex<()>,
    res: OnceLock<Atomic<MmapDriver>>,
    path: PathBuf,
}

impl LruIndex {
    /// Construct a disk-backed index rooted at `path`.
    ///
    /// If the file already exists and is non-empty it is opened and
    /// hydrated synchronously. Otherwise the disk file is **not**
    /// materialised — it appears on the first [`LruIndex::touch`]
    /// or [`LruIndex::remove`].
    pub(crate) fn with_persist_at(path: PathBuf, cancel: CancelToken, buffer: ByteBuffer) -> Self {
        let (initial, opened) = hydrate_existing(&path, &cancel, buffer);
        Self {
            inner: Arc::new(LruInner {
                state: Mutex::new(initial),
                persist: Some(LruPersist {
                    path,
                    cancel,
                    writing: Mutex::new(()),
                    res: opened.map_or_else(OnceLock::new, |a| {
                        let cell = OnceLock::new();
                        cell.set(a)
                            .unwrap_or_else(|_| unreachable!("freshly created cell"));
                        cell
                    }),
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
        let atomic = init_atomic(&persist.res, &persist.path, &persist.cancel)?;
        write_state(atomic, &snapshot, durable)?;
        self.dirty.store(false, Ordering::Release);
        Ok(())
    }
}

fn hydrate_existing(
    path: &std::path::Path,
    cancel: &CancelToken,
    mut buffer: ByteBuffer,
) -> (LruState, Option<Atomic<MmapDriver>>) {
    let nonempty = fs::metadata(path).is_ok_and(|m| m.len() > 0);
    if !nonempty {
        return (LruState::default(), None);
    }
    match open_existing(path, cancel) {
        Ok(res) => {
            let atomic = Atomic::new(res);
            let initial = read_state(&atomic, &mut buffer).unwrap_or_default();
            (initial, Some(atomic))
        }
        Err(e) => {
            tracing::debug!("open existing lru.bin failed: {e}");
            (LruState::default(), None)
        }
    }
}

fn read_state(res: &Atomic<MmapDriver>, buf: &mut ByteBuffer) -> AssetsResult<LruState> {
    let Some(len) = res.len() else {
        buf.clear();
        return Ok(LruState::default());
    };
    let len = usize::try_from(len).map_err(|error| {
        AssetsError::Storage(StorageError::Failed(format!(
            "LRU index len does not fit usize: {error}"
        )))
    })?;
    buf.ensure_len(len)?;
    let n = res.read_at(0, &mut buf[..len])?;
    buf.truncate(n);

    if n == 0 {
        return Ok(LruState::default());
    }

    let file = match rkyv::access::<crate::index::schema::ArchivedLruIndexFile, Error>(&buf[..n]) {
        Ok(archived) => rkyv::deserialize::<LruIndexFile, Error>(archived)
            .expect("BUG: LRU archived → owned deserialize"),
        Err(e) => {
            tracing::debug!("Failed to deserialize lru index: {}", e);
            return Ok(LruState::default());
        }
    };

    Ok(LruState::from(file))
}

fn write_state(res: &Atomic<MmapDriver>, state: &LruState, durable: bool) -> AssetsResult<()> {
    let file = LruIndexFile::from(state);
    let bytes = rkyv::to_bytes::<Error>(&file)
        .map_err(|e| AssetsError::Storage(StorageError::Failed(e.to_string())))?;
    if durable {
        res.write_all_durable(&bytes)?;
    } else {
        res.write_all(&bytes)?;
    }
    Ok(())
}
