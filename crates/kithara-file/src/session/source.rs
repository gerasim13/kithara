use std::{num::NonZeroUsize, ops::Range};

use kithara_assets::{AssetReader, AssetStore, ReadSide, ResourceKey};
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc, time::Duration};
use kithara_storage::{ResourceStatus, StorageError, WaitOutcome};
use kithara_stream::{
    Activity, AudioCodec, MediaInfo, NotReadyCause, PendingReason, PlayheadRead, PlayheadWrite,
    ReadOutcome, SeekControl, SeekObserve, SegmentDescriptor, SourceError as StreamSourceError,
    SourcePhase, StreamError, WorkerWake, dl::PeerHandle,
};
use tracing::trace;

use super::{inner::FileSourceCtx, segments::FileSegmentIndex};
use crate::{
    coord::FileCoord,
    engine::{EngineIdentity, FilePhase, LifecycleSink, ResourceEngine},
    error::SourceError as FileSourceError,
};

/// Sync `Source` impl over a shared [`ResourceEngine`].
///
/// All async work — HTTP fetch, body streaming, finalization — is owned
/// by the Downloader through [`FilePeer`](super::FilePeer); `FileSource`
/// just exposes the cached bytes synchronously to the audio worker.
#[derive(Clone)]
pub struct FileSource {
    /// Shared coordination — held next to the engine so hot read paths
    /// do not have to dereference the lifecycle sink.
    coord: Arc<FileCoord>,
    engine: Arc<ResourceEngine>,
    source: Arc<FileSourceCtx>,
    /// Peer registration handle returned by `Downloader::register`.
    /// Held here (mirroring `HlsSource::set_peer_handle`) so the peer
    /// stays registered for the source's lifetime — dropping the last
    /// handle would trigger `PeerInner::drop` and cancel in-flight
    /// fetches. `None` on the `local()` fast path (no Downloader at all).
    peer_handle: Option<PeerHandle>,
}

impl FileSource {
    fn known_len(&self) -> Option<u64> {
        self.coord
            .total_bytes()
            .or_else(|| self.engine.reader.len())
    }

    /// Create a source for a local/cached file (no downloads needed).
    ///
    /// `cancel` is a child of the file config master so a track drop
    /// pulse interrupts any in-flight reads — see
    /// `kithara-play/CONTEXT.md` "Cancel Hierarchy".
    pub(crate) fn local(
        reader: AssetReader,
        coord: Arc<FileCoord>,
        bus: EventBus,
        backend: AssetStore,
        key: ResourceKey,
        cancel: CancelToken,
        cached_codec: Option<AudioCodec>,
    ) -> Self {
        let source = Arc::new(FileSourceCtx::new(
            Arc::clone(&coord),
            bus,
            reader.clone(),
            true,
            Some(kithara_events::TotalBytesSource::CommittedLen),
        ));
        let sink = Arc::clone(&source) as Arc<dyn LifecycleSink>;
        let engine = Arc::new(
            ResourceEngine::builder()
                .identity(EngineIdentity {
                    store: backend,
                    key,
                    process: None,
                    cancel,
                })
                .reader(reader)
                .sink(sink)
                .initial_phase(FilePhase::Complete)
                .build(),
        );
        source.try_build_segment_index();
        source.metadata_resolved(cached_codec.map(MediaInfo::from));
        Self {
            coord,
            engine,
            source,
            peer_handle: None,
        }
    }

    fn readable_part(&self, range: Range<u64>) -> Option<Range<u64>> {
        let Some(total) = self.known_len() else {
            return Some(range);
        };
        if total > 0 && range.start >= total {
            return None;
        }
        Some(range.start..range.end.min(total))
    }

    /// Pin the Downloader peer registration to this source's lifetime.
    /// Called once after `Downloader::register`; mirrors
    /// `HlsSource::set_peer_handle`. Without this the handle returned by
    /// `register` drops immediately and `PeerInner::Drop` cancels every
    /// in-flight fetch.
    pub(crate) fn set_peer_handle(&mut self, handle: PeerHandle) {
        self.peer_handle = Some(handle);
    }

    fn update_read_demand(&self, read_pos: u64) {
        if read_pos > self.coord.read_pos() {
            self.coord.set_read_pos(read_pos);
            if let Some(lease) = self.engine.demand_lease.as_ref() {
                lease.note_progress();
            }
        }
    }

    /// Build a `FileSource` over a pre-constructed [`ResourceEngine`]. The
    /// engine is created up in `stream.rs::Stream<File>::open` and shared
    /// with [`FilePeer`](super::FilePeer); the Downloader owns the fetch
    /// loop, so this constructor does nothing async.
    pub(crate) fn with_engine(
        engine: Arc<ResourceEngine>,
        source: Arc<FileSourceCtx>,
        coord: Arc<FileCoord>,
    ) -> Self {
        Self {
            coord,
            engine,
            source,
            peer_handle: None,
        }
    }

    fn zero_read_outcome(&self, offset: u64) -> kithara_stream::StreamResult<ReadOutcome> {
        match self.engine.reader.status() {
            ResourceStatus::Active => Ok(ReadOutcome::Pending(PendingReason::NotReady(
                NotReadyCause::SourcePending,
            ))),
            ResourceStatus::Committed {
                final_len: Some(len),
            } if offset < len => Ok(ReadOutcome::Pending(PendingReason::NotReady(
                NotReadyCause::SourcePending,
            ))),
            ResourceStatus::Committed { .. } => Ok(ReadOutcome::Eof),
            ResourceStatus::Failed(reason) => Err(StreamError::Source(
                FileSourceError::Storage(StorageError::Failed(reason)).into(),
            )),
            ResourceStatus::Cancelled => Err(StreamError::Source(
                FileSourceError::Storage(StorageError::Cancelled).into(),
            )),
        }
    }
}

impl kithara_stream::Source for FileSource {
    delegate::delegate! {
        to self.coord {
            #[call(activity_handle)]
            fn activity(&self) -> Arc<dyn Activity>;
            #[call(advance_position)]
            fn advance(&self, n: u64);
            fn playhead_read(&self) -> Arc<dyn PlayheadRead>;
            fn playhead_write(&self) -> Arc<dyn PlayheadWrite>;
            fn position(&self) -> u64;
            fn seek_control(&self) -> Arc<dyn SeekControl>;
            fn seek_observe(&self) -> Arc<dyn SeekObserve>;
            fn set_position(&self, pos: u64);
        }
    }

    fn byte_map(&self) -> Option<Arc<dyn kithara_stream::ByteMap>> {
        self.source.segment_index.get()?;
        Some(Arc::new(FileByteMap {
            engine: Arc::clone(&self.engine),
            source: Arc::clone(&self.source),
        }))
    }

    fn len(&self) -> Option<u64> {
        self.known_len()
    }

    fn media_info(&self) -> Option<MediaInfo> {
        self.source.content_type_info.get().cloned()
    }

    fn phase(&self) -> SourcePhase {
        let pos = self.coord.position();
        self.phase_at(pos..pos.saturating_add(1))
    }

    fn phase_at(&self, range: Range<u64>) -> SourcePhase {
        let Some(readable) = self.readable_part(range) else {
            return match self.engine.reader.status() {
                ResourceStatus::Committed { .. } => SourcePhase::Eof,
                ResourceStatus::Active | ResourceStatus::Failed(_) | ResourceStatus::Cancelled => {
                    SourcePhase::Waiting
                }
            };
        };
        let contains = readable.is_empty() || self.engine.reader.contains_range(readable);
        if contains {
            return SourcePhase::Ready;
        }

        if self.coord.seek_obs().is_flushing() {
            return SourcePhase::Seeking;
        }
        SourcePhase::Waiting
    }

    #[cfg_attr(feature = "perf", hotpath::measure)]
    fn read_at(
        &mut self,
        offset: u64,
        buf: &mut [u8],
    ) -> kithara_stream::StreamResult<ReadOutcome> {
        let n = self
            .engine
            .reader
            .read_at(offset, buf)
            .map_err(|e| StreamError::Source(FileSourceError::Storage(e).into()))?;

        let Some(count) = NonZeroUsize::new(n) else {
            return self.zero_read_outcome(offset);
        };

        trace!(offset, bytes = n, "FileSource read complete");

        Ok(ReadOutcome::Bytes(count))
    }

    fn set_worker_wake(&self, wake: Arc<dyn WorkerWake>) {
        self.engine.set_worker_wake(wake);
    }

    fn take_reader_event_sink(&mut self) -> Option<kithara_stream::BoxedEventSink> {
        let hooks = super::reader::FileReaderEventSink::new(
            self.source.bus.clone(),
            Arc::clone(&self.coord),
            self.coord.seek_epoch_handle(),
        );
        Some(Box::new(hooks))
    }

    #[cfg_attr(feature = "perf", hotpath::measure)]
    fn wait_range(
        &mut self,
        range: Range<u64>,
        timeout: Option<Duration>,
    ) -> kithara_stream::StreamResult<WaitOutcome> {
        match self.phase_at(range.clone()) {
            SourcePhase::Seeking => return Ok(WaitOutcome::Interrupted),
            SourcePhase::Eof => return Ok(WaitOutcome::Eof),
            SourcePhase::Ready => return Ok(WaitOutcome::Ready),
            _ => {}
        }

        self.update_read_demand(range.start);

        if timeout.is_some() {
            return Err(StreamError::Source(StreamSourceError::WaitBudgetExceeded));
        }

        self.engine
            .reader
            .wait_range(range)
            .map_err(|e| StreamError::Source(FileSourceError::Storage(e).into()))
    }
}

/// Byte-map handle for a fully cached fragmented-mp4 file.
///
/// Holds the engine and File shell so the layout survives independently of
/// the original `FileSource` cursor; segment queries hit the lazy
/// `OnceLock<FileSegmentIndex>` populated on first call.
struct FileByteMap {
    engine: Arc<ResourceEngine>,
    source: Arc<FileSourceCtx>,
}

impl FileByteMap {
    fn segment_index(&self) -> Option<&FileSegmentIndex> {
        self.source.segment_index.get()
    }
}

impl kithara_stream::ByteMap for FileByteMap {
    fn init_segment_range(&self) -> Range<u64> {
        self.segment_index()
            .map_or(0..0, FileSegmentIndex::init_range)
    }

    fn len(&self) -> Option<u64> {
        self.engine.reader.len()
    }

    fn segment_after_byte(&self, byte_offset: u64) -> Option<SegmentDescriptor> {
        self.segment_index()?.segment_after_byte(byte_offset)
    }

    fn segment_at_time(&self, t: Duration) -> Option<SegmentDescriptor> {
        self.segment_index()?.segment_at_time(t)
    }

    fn segment_count(&self) -> Option<u32> {
        Some(self.segment_index()?.segment_count())
    }
}
