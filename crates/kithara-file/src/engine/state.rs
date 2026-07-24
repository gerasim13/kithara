use std::sync::{
    OnceLock,
    atomic::{AtomicU8, Ordering},
};

use bon::bon;
use kithara_assets::{AssetReader, AssetWriter, DemandLease, RawWriteHandle, WriteSide};
use kithara_platform::sync::{Arc, Mutex};
use kithara_stream::WorkerWake;
use kithara_test_utils::{kithara, probe::IntoProbeArg};

use super::{EngineIdentity, LifecycleSink};

/// File-streaming FSM phases. Stored as `AtomicU8` for lock-free transitions
/// from the download driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FilePhase {
    /// Ready to start the initial full-file download.
    Init = 0,
    /// Download in progress.
    Downloading = 1,
    /// File fully downloaded (or local).
    Complete = 2,
}

impl IntoProbeArg for FilePhase {
    fn into_probe_arg(self) -> u64 {
        u64::from(self as u8)
    }
}

/// Per-resource state shared by the protocol shell and download driver.
pub(crate) struct ResourceEngine {
    pub(crate) identity: EngineIdentity,
    pub(crate) reader: AssetReader,
    pub(crate) writer: Mutex<Option<AssetWriter>>,
    pub(crate) raw: Option<RawWriteHandle>,
    pub(crate) sink: Arc<dyn LifecycleSink>,
    pub(crate) demand_lease: Option<DemandLease>,
    pub(crate) phase: AtomicU8,
    pub(crate) worker_wake: OnceLock<Arc<dyn WorkerWake>>,
}

#[bon]
impl ResourceEngine {
    #[builder]
    pub(crate) fn new(
        identity: EngineIdentity,
        reader: AssetReader,
        writer: Option<AssetWriter>,
        raw: Option<RawWriteHandle>,
        sink: Arc<dyn LifecycleSink>,
        initial_phase: FilePhase,
        demand_lease: Option<DemandLease>,
    ) -> Self {
        Self {
            identity,
            reader,
            writer: Mutex::new(writer),
            raw,
            sink,
            demand_lease,
            phase: AtomicU8::new(initial_phase as u8),
            worker_wake: OnceLock::new(),
        }
    }

    /// Mark the resource failed and evict the pre-allocated cache file.
    ///
    /// Call on any terminal download error so the file is gone from disk
    /// before the task returns — without this the reader held by the engine
    /// keeps the mmap parked in the cache directory for the source lifetime.
    pub(crate) fn fail_and_evict(&self, reason: &str) {
        if let Some(writer) = self.take_writer() {
            writer.fail(reason.to_string());
        }
        if let Err(error) = self.identity.store.remove_resource(&self.identity.key) {
            tracing::warn!(%error, reason, "kithara-file: failed to remove terminal resource");
        }
    }

    #[kithara::probe(phase)]
    pub(crate) fn set_phase(&self, phase: FilePhase, final_len: Option<u64>) {
        let previous = self.phase.load(Ordering::Acquire);
        self.phase.store(phase as u8, Ordering::Release);
        if matches!(phase, FilePhase::Complete) && previous != FilePhase::Complete as u8 {
            self.sink.complete(final_len);
        }
    }

    pub(crate) fn set_worker_wake(&self, wake: Arc<dyn WorkerWake>) {
        let _ = self.worker_wake.set(wake);
    }

    /// Take the single commit-owning writer, if this is a download path and it
    /// has not been consumed yet.
    pub(crate) fn take_writer(&self) -> Option<AssetWriter> {
        self.writer.lock().take()
    }

    pub(crate) fn wake_worker(&self) {
        if let Some(wake) = self.worker_wake.get() {
            wake.wake();
        }
    }
}
