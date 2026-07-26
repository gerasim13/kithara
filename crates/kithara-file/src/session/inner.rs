use std::sync::OnceLock;

use kithara_assets::{AssetReader, ReadSide, ResourceAcquisition};
use kithara_events::{
    AudioCodecKind, ContainerKind, EventBus, FileError, FileEvent, TotalBytesSource,
};
use kithara_platform::sync::Arc;
use kithara_stream::MediaInfo;

use super::segments::FileSegmentIndex;
use crate::{
    coord::FileCoord,
    engine::{EngineIdentity, LifecycleSink},
    error::SourceError,
};

/// Creation helper: acquire the asset resource and prepare the event bus
/// before constructing a remote `FileSource`. The phase-typed acquisition
/// (`Pending` writer to download / `Ready` reader already cached) is decided
/// by the caller.
pub(crate) struct FileStreamState {
    pub(crate) identity: EngineIdentity,
    pub(crate) acq: ResourceAcquisition,
    pub(crate) bus: EventBus,
}

impl FileStreamState {
    pub(crate) fn create(
        identity: EngineIdentity,
        bus: Option<EventBus>,
        event_channel_capacity: usize,
    ) -> Result<Self, SourceError> {
        let acq = identity.acquire().map_err(SourceError::Assets)?;
        let bus = bus.unwrap_or_else(|| EventBus::new(event_channel_capacity));
        Ok(Self { identity, acq, bus })
    }
}

/// File-shell state used by the engine's lifecycle callbacks.
pub(crate) struct FileSourceCtx {
    pub(crate) coord: Arc<FileCoord>,
    pub(crate) bus: EventBus,
    pub(crate) content_type_info: OnceLock<MediaInfo>,
    pub(crate) segment_index: OnceLock<FileSegmentIndex>,
    reader: AssetReader,
    cached: bool,
    opened_emitted: OnceLock<()>,
    total_bytes_source: Option<TotalBytesSource>,
}

impl FileSourceCtx {
    pub(crate) fn new(
        coord: Arc<FileCoord>,
        bus: EventBus,
        reader: AssetReader,
        cached: bool,
        total_bytes_source: Option<TotalBytesSource>,
    ) -> Self {
        Self {
            coord,
            bus,
            reader,
            cached,
            total_bytes_source,
            content_type_info: OnceLock::new(),
            segment_index: OnceLock::new(),
            opened_emitted: OnceLock::new(),
        }
    }

    /// Read the fully cached file bytes and parse a fragmented-mp4 index,
    /// or `None` when the file is not yet complete or not fragmented mp4.
    fn build_segment_index_from_cache(&self) -> Option<FileSegmentIndex> {
        let total = self.reader.len()?;
        if total == 0 || !self.reader.contains_range(0..total) {
            return None;
        }
        let total_usize = usize::try_from(total).ok()?;
        let mut buf: Box<[u8]> = std::iter::repeat_n(0u8, total_usize).collect();
        self.reader.read_at(0, &mut buf).ok()?;
        FileSegmentIndex::try_build(&buf)
    }

    /// One-shot fragmented-mp4 parse from the fully cached file bytes.
    /// Idempotent: a second call is a `OnceLock::get` fast-path no-op.
    pub(crate) fn try_build_segment_index(&self) {
        if self.segment_index.get().is_some() {
            return;
        }
        if let Some(index) = self.build_segment_index_from_cache() {
            let _ = self.segment_index.set(index);
        }
    }
}

impl LifecycleSink for FileSourceCtx {
    fn metadata_resolved(&self, info: Option<MediaInfo>) {
        if let Some(info) = info {
            let _ = self.content_type_info.set(info);
        }
        if self.opened_emitted.set(()).is_err() {
            return;
        }
        let total_bytes = self.coord.total_bytes().or_else(|| self.reader.len());
        let (codec, container) = self
            .content_type_info
            .get()
            .map_or((None, None), map_media_info);
        self.bus.publish(FileEvent::Opened {
            codec,
            container,
            total_bytes,
            cached: self.cached,
        });
        if let (Some(total_bytes), Some(source)) = (total_bytes, self.total_bytes_source) {
            self.bus.publish(FileEvent::TotalBytesResolved {
                total_bytes,
                source,
            });
        }
    }

    fn total_bytes_resolved(&self, total_bytes: u64) {
        let previous_total = self.coord.total_bytes();
        self.coord.set_total_bytes(Some(total_bytes));
        if previous_total != Some(total_bytes) {
            self.bus.publish(FileEvent::TotalBytesResolved {
                total_bytes,
                source: TotalBytesSource::ContentLength,
            });
        }
    }

    fn complete(&self, final_len: Option<u64>) {
        if let Some(final_len) = final_len {
            self.coord.set_total_bytes(Some(final_len));
        }
        if let Some(total_bytes) = self.coord.total_bytes() {
            self.bus.publish(FileEvent::CacheComplete { total_bytes });
        }
    }

    fn error(&self, reason: &str) {
        self.bus.publish(FileEvent::Error {
            error: FileError::Io(reason.to_string()),
        });
    }
}

fn map_media_info(info: &MediaInfo) -> (Option<AudioCodecKind>, Option<ContainerKind>) {
    (
        info.codec.map(map_audio_codec),
        info.container.map(map_container),
    )
}

fn map_audio_codec(codec: kithara_stream::AudioCodec) -> AudioCodecKind {
    match codec {
        kithara_stream::AudioCodec::AacLc => AudioCodecKind::AacLc,
        kithara_stream::AudioCodec::AacHe => AudioCodecKind::AacHe,
        kithara_stream::AudioCodec::AacHeV2 => AudioCodecKind::AacHeV2,
        kithara_stream::AudioCodec::Mp3 => AudioCodecKind::Mp3,
        kithara_stream::AudioCodec::Flac => AudioCodecKind::Flac,
        kithara_stream::AudioCodec::Vorbis => AudioCodecKind::Vorbis,
        kithara_stream::AudioCodec::Opus => AudioCodecKind::Opus,
        kithara_stream::AudioCodec::Alac => AudioCodecKind::Alac,
        kithara_stream::AudioCodec::Pcm => AudioCodecKind::Pcm,
        kithara_stream::AudioCodec::Adpcm => AudioCodecKind::Adpcm,
    }
}

fn map_container(container: kithara_stream::ContainerFormat) -> ContainerKind {
    match container {
        kithara_stream::ContainerFormat::Mp4 => ContainerKind::Mp4,
        kithara_stream::ContainerFormat::Fmp4 => ContainerKind::Fmp4,
        kithara_stream::ContainerFormat::MpegTs => ContainerKind::MpegTs,
        kithara_stream::ContainerFormat::MpegAudio => ContainerKind::MpegAudio,
        kithara_stream::ContainerFormat::Adts => ContainerKind::Adts,
        kithara_stream::ContainerFormat::Flac => ContainerKind::Flac,
        kithara_stream::ContainerFormat::Wav => ContainerKind::Wav,
        kithara_stream::ContainerFormat::Ogg => ContainerKind::Ogg,
        kithara_stream::ContainerFormat::Caf => ContainerKind::Caf,
        kithara_stream::ContainerFormat::Mkv => ContainerKind::Mkv,
    }
}

#[cfg(test)]
mod tests {
    use kithara_assets::{
        AcquisitionResult, AssetResource, AssetResourceState, AssetSource, AssetStore,
        AssetStoreBuilder, ResourceKey, StorageBackend, WriteSide,
    };
    use kithara_events::{Envelope, Event};
    use kithara_platform::CancelToken;
    use kithara_stream::{PlayheadState, TimelineState};
    use kithara_test_utils::kithara;
    use url::Url;

    use super::*;
    use crate::{
        File,
        engine::{FilePhase, ResourceEngine},
    };

    fn test_key(store: &AssetStore) -> ResourceKey {
        let source = AssetSource::Remote {
            url: Url::parse("https://example.com/remote.dat").expect("test URL"),
            discriminator: Some("peer-test".to_string()),
        };
        let scope = store.scope::<File>(&source).expect("test scope");
        scope
            .key(&AssetResource::Source {
                extension: "dat".to_string(),
            })
            .expect("test resource key")
    }

    fn make_engine() -> (Arc<ResourceEngine>, Arc<FileSourceCtx>) {
        let store = AssetStoreBuilder::default()
            .backend(StorageBackend::Memory)
            .cancel(CancelToken::never())
            .build();
        let key = test_key(&store);
        let AcquisitionResult::Pending(writer) = store.acquire_resource(&key, None).unwrap() else {
            panic!("fresh acquire must be Pending");
        };
        let reader = writer.reader();
        let coord = Arc::new(FileCoord::new(
            Arc::new(PlayheadState::new()),
            Arc::new(TimelineState::new()),
        ));
        let source = Arc::new(FileSourceCtx::new(
            coord,
            EventBus::new(16),
            reader.clone(),
            false,
            None,
        ));
        let sink = Arc::clone(&source) as Arc<dyn LifecycleSink>;
        let identity = EngineIdentity {
            store,
            key,
            process: None,
            cancel: CancelToken::never(),
        };
        let engine = Arc::new(
            ResourceEngine::builder()
                .identity(identity)
                .reader(reader)
                .writer(writer)
                .sink(sink)
                .initial_phase(FilePhase::Init)
                .build(),
        );
        (engine, source)
    }

    #[kithara::test]
    fn remote_capture_metadata_publishes_opened() {
        let (engine, source) = make_engine();
        let mut rx = source.bus.subscribe();
        let mut headers = kithara_net::Headers::new();
        headers.insert("content-type", "audio/mpeg");
        headers.insert("content-length", "12");

        engine.capture_content_metadata(&headers, 0);

        assert!(matches!(
            rx.try_recv(),
            Ok(Envelope {
                event: Event::File(FileEvent::TotalBytesResolved { .. }),
                ..
            })
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(Envelope {
                event: Event::File(FileEvent::Opened {
                    cached: false,
                    total_bytes: Some(12),
                    ..
                }),
                ..
            })
        ));
    }

    #[kithara::test]
    fn complete_phase_publishes_cache_complete() {
        let (engine, source) = make_engine();
        let mut rx = source.bus.subscribe();
        source.coord.set_total_bytes(Some(12));

        engine.set_phase(FilePhase::Complete, Some(12));

        assert!(matches!(
            rx.try_recv(),
            Ok(Envelope {
                event: Event::File(FileEvent::CacheComplete { total_bytes: 12 }),
                ..
            })
        ));
    }

    #[kithara::test]
    fn terminal_failure_does_not_publish_cache_complete() {
        let (engine, source) = make_engine();
        let mut rx = source.bus.subscribe();
        source.coord.set_total_bytes(Some(12));
        engine.set_phase(FilePhase::Downloading, None);

        engine.fail_and_evict("fixture terminal failure");

        assert!(rx.try_recv().is_err());
        assert!(matches!(
            engine
                .identity
                .store
                .resource_state(&engine.identity.key)
                .expect("resource state"),
            AssetResourceState::Missing
        ));
    }
}
