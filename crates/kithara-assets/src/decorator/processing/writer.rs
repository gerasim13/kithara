use std::fmt;

use bon::bon;
use kithara_bufpool::BytePool;
use kithara_platform::{sync::Arc, time::Duration};
use kithara_storage::{StorageError, StorageResult};

use super::{contract::ProcessCtx, gate::ReadinessGate, guard::GateGuard, reader::ProcessedReader};
use crate::resource::{RawWriteHandle, ReadSide, WriteSide};

fn run_process<W>(
    pool: &BytePool,
    processor: &ProcessCtx,
    inner: &W,
    final_len: u64,
    chunk_size: usize,
) -> StorageResult<u64>
where
    W: WriteSide,
{
    let chunk_size_u64 = chunk_size as u64;
    let mut sink = processor.begin();
    let raw = inner.reader();

    let mut input_buf = pool.get_with(|buffer| buffer.resize(chunk_size, 0));
    let mut output_buf = pool.get_with(|buffer| buffer.resize(chunk_size, 0));

    let mut read_offset = 0u64;
    let mut write_offset = 0u64;

    while read_offset < final_len {
        let remaining_u64 = (final_len - read_offset).min(chunk_size_u64);
        let to_read = usize::try_from(remaining_u64).map_err(|err| {
            StorageError::Failed(format!(
                "process_and_write: chunk size {remaining_u64} does not fit usize: {err}"
            ))
        })?;
        let is_last = read_offset + remaining_u64 >= final_len;

        let read = raw.read_inflight_at(read_offset, &mut input_buf[..to_read])?;
        if read == 0 {
            break;
        }

        let written = sink
            .process(&input_buf[..read], &mut output_buf[..read], is_last)
            .map_err(StorageError::Failed)?;

        inner.write_at(write_offset, &output_buf[..written])?;

        read_offset += read as u64;
        write_offset += u64::try_from(written).map_err(|err| {
            StorageError::Failed(format!(
                "process_and_write: written {written} does not fit u64: {err}"
            ))
        })?;
    }

    Ok(write_offset)
}

/// Default [`ProcessedWriter`] transform pass size. One pass over a 64 `KiB`
/// window keeps the pooled input and output buffers page-sized while still
/// committing most resources in a handful of passes.
pub(super) const DEFAULT_CHUNK_SIZE: usize = 64 * 1024;

/// Default backstop between [`ReadinessGate`] wakeups. The gate is woken by
/// `notify_all` on every state change, so this only bounds how long a waiter
/// sleeps before rechecking an abort it was not signalled for.
pub(super) const DEFAULT_GATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Write handle that processes a resource atomically when it is committed.
pub struct ProcessedWriter<W> {
    pool: BytePool,
    gate_poll_interval: Duration,
    guard: GateGuard,
    processor: Option<ProcessCtx>,
    inner: W,
    chunk_size: usize,
}

impl<W: fmt::Debug> fmt::Debug for ProcessedWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessedWriter")
            .field("inner", &self.inner)
            .field("processor", &self.processor)
            .field("ready", &self.guard.is_ready())
            .finish_non_exhaustive()
    }
}

#[bon]
impl<W> ProcessedWriter<W>
where
    W: WriteSide,
{
    /// Creates a pending writer for a resource and its optional processor.
    /// `chunk_size` and `gate_poll_interval` carry the production defaults, so
    /// a caller states them only when it wants a different pass size or
    /// recheck cadence.
    #[builder]
    pub fn new(
        inner: W,
        processor: Option<ProcessCtx>,
        pool: BytePool,
        #[builder(default = DEFAULT_CHUNK_SIZE)] chunk_size: usize,
        #[builder(default = DEFAULT_GATE_POLL_INTERVAL)] gate_poll_interval: Duration,
    ) -> Self {
        let ready = processor.is_none();
        Self {
            inner,
            processor,
            pool,
            chunk_size,
            gate_poll_interval,
            guard: GateGuard::new(Arc::new(ReadinessGate::new(ready, gate_poll_interval))),
        }
    }

    fn build_reader(&self, inner: W::Reader) -> ProcessedReader<W::Reader> {
        ProcessedReader::with_readiness(
            inner,
            self.guard.shared(),
            self.processor.clone(),
            self.pool.clone(),
            self.chunk_size,
            self.gate_poll_interval,
        )
    }
}

impl<W> WriteSide for ProcessedWriter<W>
where
    W: WriteSide,
{
    type Reader = ProcessedReader<W::Reader>;

    fn abandon(mut self) {
        self.inner.abandon();
        // WHY: Failing this shared gate would poison readers during generation teardown.
        self.guard.disarm();
    }

    fn commit(mut self, final_len: Option<u64>) -> StorageResult<ProcessedReader<W::Reader>> {
        let needs_processing = self.processor.is_some() && !self.guard.is_ready();
        let actual_len = match (needs_processing, final_len, self.processor.as_ref()) {
            (true, Some(len), Some(processor)) if len > 0 => Some(run_process(
                &self.pool,
                processor,
                &self.inner,
                len,
                self.chunk_size,
            )?),
            _ => final_len,
        };

        let reader_inner = self.inner.commit(actual_len)?;
        if needs_processing {
            self.guard.mark_ready();
        }
        self.guard.disarm();
        Ok(ProcessedReader::with_readiness(
            reader_inner,
            self.guard.shared(),
            self.processor.clone(),
            self.pool.clone(),
            self.chunk_size,
            self.gate_poll_interval,
        ))
    }

    fn fail(mut self, reason: String) {
        self.inner.fail(reason);
        self.guard.fail();
        self.guard.disarm();
    }

    fn reader(&self) -> ProcessedReader<W::Reader> {
        self.build_reader(self.inner.reader())
    }

    delegate::delegate! {
        to self.inner {
            fn raw_write_handle(&self) -> RawWriteHandle;
            fn write_at(&self, offset: u64, data: &[u8]) -> StorageResult<()>;
        }
    }
}
