use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    task::{Wake, Waker},
};

use kithara_assets::{AssetReader, ReadSide, ResourceLease, WriterEpoch};
use kithara_events::{
    AudioCodecKind, ContainerKind, EventBus, FileError, FileEvent, TotalBytesSource,
};
use kithara_net::Headers;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Weak},
};
use kithara_storage::ResourceStatus;
use kithara_stream::{AudioCodec, ContainerFormat, MediaInfo, WorkerWake};
use url::Url;

use super::segments::FileSegmentIndex;
use crate::coord::FileCoord;

const CODEC_SNIFF_BYTES: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileTerminalState {
    Active = 0,
    Committed = 1,
    Failed = 2,
    Cancelled = 3,
}

impl From<&ResourceStatus> for FileTerminalState {
    fn from(status: &ResourceStatus) -> Self {
        match status {
            ResourceStatus::Active => Self::Active,
            ResourceStatus::Committed { .. } => Self::Committed,
            ResourceStatus::Failed(_) => Self::Failed,
            ResourceStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// Control-plane handles shared between the source and the download driver.
pub(crate) struct FileSourceCtx {
    pub(crate) coord: Arc<FileCoord>,
    pub(crate) cancel: CancelToken,
    pub(crate) bus: EventBus,
    /// Mirrors `FileConfig::reader_event_capacity`: ring depth the reader
    /// sink wraps `bus` with. Sits next to the bus because that is the only
    /// pair it is ever used as.
    pub(crate) reader_event_capacity: usize,
}

/// Data-plane handles describing where the file lives and how to fetch it.
///
/// The session-owned writer never leaves `kithara-assets`; File keeps only the
/// synchronous reader and protocol metadata needed to drive HTTP callbacks.
pub(crate) struct FileAssetCtx {
    pub(crate) reader: AssetReader,
    pub(crate) headers: Option<Headers>,
    pub(crate) url: Url,
}

/// Shared inner state for a `FileSource`. All fields are either immutable
/// (set at construction) or self-synchronizing - there is no `Mutex`.
pub(crate) struct FileInner {
    pub(crate) asset: FileAssetCtx,
    pub(crate) source: FileSourceCtx,
    /// `MediaInfo` discovered from the HTTP `Content-Type` header on
    /// first connect (or sniffed from the cached bytes for local
    /// fast-path). Set at most once. Carries both codec and container
    /// so downstream Apple/Android dispatch can pick a backend without
    /// re-probing the bytes.
    pub(crate) content_type_info: OnceLock<MediaInfo>,
    /// Lazily-built fragmented-mp4 segment index. Populated on first
    /// segment-method call once the file is fully cached and parses
    /// as fragmented mp4. Stays empty for non-mp4 files, classic mp4
    /// (no `moof` chain), or while the file is still downloading.
    pub(crate) segment_index: OnceLock<FileSegmentIndex>,

    /// Consumer demand lease held for this source's lifetime. `Some` on
    /// the remote path: it keeps the pending resource alive and lets
    /// [`FilePeer`](super::FilePeer) take over the single-writer
    /// election if the original writer drops. `None` for local /
    /// already-cached sources that never download.
    pub(crate) resource_lease: Option<ResourceLease>,
    completion_started: AtomicBool,
    complete: AtomicBool,
    terminal_state: AtomicU8,

    opened_emitted: OnceLock<()>,

    /// Late-bound audio-worker wake. Remote file sources can underrun while
    /// HTTP bytes are still arriving, so each write wakes the worker that
    /// previously parked on a non-blocking readiness probe.
    worker_wake: OnceLock<Arc<dyn WorkerWake>>,
    reader_waker: OnceLock<Waker>,
}

impl FileInner {
    pub(crate) fn new(
        source: FileSourceCtx,
        asset: FileAssetCtx,
        complete: bool,
        resource_lease: Option<ResourceLease>,
    ) -> Self {
        let terminal_state = FileTerminalState::from(&asset.reader.status());
        let complete = complete && terminal_state == FileTerminalState::Committed;
        let inner = Self {
            source,
            asset,
            resource_lease,
            opened_emitted: OnceLock::new(),
            content_type_info: OnceLock::new(),
            segment_index: OnceLock::new(),
            worker_wake: OnceLock::new(),
            reader_waker: OnceLock::new(),
            completion_started: AtomicBool::new(complete),
            complete: AtomicBool::new(complete),
            terminal_state: AtomicU8::new(terminal_state as u8),
        };
        if complete {
            inner.try_build_segment_index();
        }
        inner
    }

    /// Read the fully cached file bytes and parse a fragmented-mp4 index,
    /// or `None` when the file is not yet complete or not fragmented mp4.
    fn build_segment_index_from_cache(&self) -> Option<FileSegmentIndex> {
        let total = self.asset.reader.len()?;
        if total == 0 || !self.asset.reader.contains_range(0..total) {
            return None;
        }
        let total_usize = usize::try_from(total).ok()?;
        let mut buf: Box<[u8]> = std::iter::repeat_n(0u8, total_usize).collect();
        self.asset.reader.read_at(0, &mut buf).ok()?;
        FileSegmentIndex::try_build(&buf)
    }

    pub(crate) fn publish_opened(
        &self,
        total_bytes: Option<u64>,
        cached: bool,
        source: Option<TotalBytesSource>,
    ) {
        if self.opened_emitted.set(()).is_err() {
            return;
        }
        let (codec, container) = self
            .content_type_info
            .get()
            .map_or((None, None), map_media_info);
        self.source.bus.publish(FileEvent::Opened {
            codec,
            container,
            total_bytes,
            cached,
        });
        if let (Some(total_bytes), Some(source)) = (total_bytes, source) {
            self.source.bus.publish(FileEvent::TotalBytesResolved {
                total_bytes,
                source,
            });
        }
    }

    pub(crate) fn publish_total_bytes_resolved(&self, total_bytes: u64, source: TotalBytesSource) {
        self.source.bus.publish(FileEvent::TotalBytesResolved {
            total_bytes,
            source,
        });
    }

    /// Mark the file complete once. The one-shot fragmented-mp4 parse runs
    /// on this edge so the hot-path `byte_map` audit
    /// can short-circuit on `segment_index.get()` without re-reading the
    /// file each tick.
    pub(crate) fn mark_complete(&self) {
        if self.completion_started.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(total_bytes) = self.source.coord.total_bytes() {
            self.source
                .bus
                .publish(FileEvent::CacheComplete { total_bytes });
        }
        self.try_build_segment_index();
        self.set_terminal_state(FileTerminalState::Committed);
        self.complete.store(true, Ordering::Release);
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    pub(crate) fn terminal_state(&self) -> FileTerminalState {
        match self.terminal_state.load(Ordering::Acquire) {
            code if code == FileTerminalState::Committed as u8 => FileTerminalState::Committed,
            code if code == FileTerminalState::Failed as u8 => FileTerminalState::Failed,
            code if code == FileTerminalState::Cancelled as u8 => FileTerminalState::Cancelled,
            _ => FileTerminalState::Active,
        }
    }

    pub(crate) fn refresh_unmanaged_terminal(&self) {
        if self.resource_lease.is_some() || self.is_complete() {
            return;
        }
        self.set_terminal_state(FileTerminalState::from(&self.asset.reader.status()));
    }

    fn set_terminal_state(&self, state: FileTerminalState) {
        self.terminal_state.store(state as u8, Ordering::Release);
    }

    pub(crate) fn set_worker_wake(&self, wake: Arc<dyn WorkerWake>) {
        let _ = self.worker_wake.set(wake);
    }

    pub(crate) fn arm_reader_waker(self: &Arc<Self>) {
        let Some(lease) = self.resource_lease.as_ref() else {
            return;
        };
        let waker = self.reader_waker.get_or_init(|| {
            Waker::from(Arc::new(FileReaderWake {
                inner: Arc::downgrade(self),
            }))
        });
        lease.register_reader_waker(waker);
    }

    pub(crate) fn observe_committed(&self) -> bool {
        if self.resource_lease.is_none() {
            return self.complete.load(Ordering::Acquire);
        }
        let ResourceStatus::Committed { final_len } = self.asset.reader.status() else {
            return false;
        };
        let total_bytes = final_len.map_or_else(|| self.asset.reader.len(), Some);
        self.source.coord.set_total_bytes(total_bytes);
        if let Some(total_bytes) = total_bytes {
            self.source.coord.set_download_pos(total_bytes);
        }
        if self.content_type_info.get().is_none()
            && let Some(codec) = sniff_codec(&self.asset.reader)
        {
            let _ = self.content_type_info.set(MediaInfo::from(codec));
        }
        self.publish_opened(
            total_bytes,
            false,
            total_bytes.map(|_| TotalBytesSource::CommittedLen),
        );
        self.mark_complete();
        true
    }

    /// Commit the epoch once every byte up to `final_len` has landed.
    /// Returns whether the resource committed through this epoch.
    pub(crate) fn commit_if_complete(&self, epoch: &WriterEpoch, final_len: u64) -> bool {
        if self.asset.reader.next_gap(0, final_len).is_some() {
            return false;
        }
        match epoch.commit(Some(final_len)).current() {
            Some(Ok(())) => {
                self.observe_committed();
                true
            }
            Some(Err(error)) => {
                self.source.bus.publish(FileEvent::Error {
                    error: FileError::Io(error.to_string()),
                });
                false
            }
            None => false,
        }
    }

    /// One-shot fragmented-mp4 parse from the fully cached file bytes.
    /// Idempotent: a second call is a `OnceLock::get` fast-path no-op.
    /// Called from `mark_complete` (and from `new` for files
    /// constructed already-complete) so the hot-path `byte_map`
    /// audit only ever reads the cached result.
    fn try_build_segment_index(&self) {
        if self.segment_index.get().is_some() {
            return;
        }
        if let Some(index) = self.build_segment_index_from_cache() {
            let _ = self.segment_index.set(index);
        }
    }

    pub(crate) fn wake_worker(&self) {
        if let Some(wake) = self.worker_wake.get() {
            wake.wake();
        }
    }
}

struct FileReaderWake {
    inner: Weak<FileInner>,
}

impl Wake for FileReaderWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(inner) = self.inner.upgrade() {
            match inner.asset.reader.status() {
                ResourceStatus::Active => inner.arm_reader_waker(),
                ResourceStatus::Committed { .. } => {
                    let _ = inner.observe_committed();
                }
                ResourceStatus::Failed(_) => {
                    inner.set_terminal_state(FileTerminalState::Failed);
                }
                ResourceStatus::Cancelled => {
                    inner.set_terminal_state(FileTerminalState::Cancelled);
                }
            }
            inner.wake_worker();
        }
    }
}

pub(crate) fn sniff_codec(reader: &AssetReader) -> Option<AudioCodec> {
    let mut buf = [0u8; CODEC_SNIFF_BYTES];
    let read = reader.read_at(0, &mut buf).ok()?;
    AudioCodec::try_from(&buf[..read]).ok()
}

fn map_media_info(info: &MediaInfo) -> (Option<AudioCodecKind>, Option<ContainerKind>) {
    (
        info.codec.map(map_audio_codec),
        info.container.map(map_container),
    )
}

const fn map_audio_codec(codec: AudioCodec) -> AudioCodecKind {
    match codec {
        AudioCodec::AacLc => AudioCodecKind::AacLc,
        AudioCodec::AacHe => AudioCodecKind::AacHe,
        AudioCodec::AacHeV2 => AudioCodecKind::AacHeV2,
        AudioCodec::Mp3 => AudioCodecKind::Mp3,
        AudioCodec::Flac => AudioCodecKind::Flac,
        AudioCodec::Vorbis => AudioCodecKind::Vorbis,
        AudioCodec::Opus => AudioCodecKind::Opus,
        AudioCodec::Alac => AudioCodecKind::Alac,
        AudioCodec::Pcm => AudioCodecKind::Pcm,
        AudioCodec::Adpcm => AudioCodecKind::Adpcm,
    }
}

const fn map_container(container: ContainerFormat) -> ContainerKind {
    match container {
        ContainerFormat::Mp4 => ContainerKind::Mp4,
        ContainerFormat::Fmp4 => ContainerKind::Fmp4,
        ContainerFormat::MpegTs => ContainerKind::MpegTs,
        ContainerFormat::MpegAudio => ContainerKind::MpegAudio,
        ContainerFormat::Adts => ContainerKind::Adts,
        ContainerFormat::Flac => ContainerKind::Flac,
        ContainerFormat::Wav => ContainerKind::Wav,
        ContainerFormat::Ogg => ContainerKind::Ogg,
        ContainerFormat::Caf => ContainerKind::Caf,
        ContainerFormat::Mkv => ContainerKind::Mkv,
    }
}
