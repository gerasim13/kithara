use std::{
    io::{Error as IoError, Seek, SeekFrom},
    marker::PhantomData,
    num::NonZeroU32,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};

use kithara_bufpool::{BytePool, SamplePool};
use kithara_decode::{Decoder, DecoderConfig, DecoderFactory, DecoderResamplerConfig};
use kithara_events::{DecoderChangeCause, Event, EventBus, FrameDomain};
use kithara_platform::{
    CancelScope,
    sync::Arc,
    tokio::{runtime::Handle as RuntimeHandle, task::spawn_blocking},
};
use kithara_resampler::ResamplerBackend;
use kithara_signal::{AudioChunk, AudioSpec};
use kithara_stream::{MediaInfo, OpenedReader, PlayheadWrite, Stream, StreamType, WorkerWake};
use tracing::{debug, info, warn};

use super::{
    AudioConfig, AudioDecoderConfig, AudioSession, ConsumerWakeMode, DecodeError, DecodeInit,
    PreparedAudioLane, ProducerPort, RebuildRuntime, SharedStream, SourceParts, StreamAudioSource,
    StreamDecoderFactory, ThreadWake,
    core::{Audio, AudioParts, AudioRuntime, Controls, PreparedAudio, Session},
    event::{
        AudioEvents, DecoderChangedEventData, decoder_changed_event, decoder_gapless_event,
        decoder_resampler_event, playback_resampler_event,
    },
    ring::{RingConsumer, RingParts, create_channels, create_trash_channel},
};
const WARM_DECODE_FRAMES: usize = 4608;

#[derive(Clone)]
struct DecoderDeps<B> {
    host_sample_rate: Arc<AtomicU32>,
    decoder: AudioDecoderConfig<B>,
    byte_pool: BytePool,
    sample_pool: SamplePool,
}

impl<B> DecoderDeps<B>
where
    B: Default + ResamplerBackend,
{
    fn new(
        decoder: AudioDecoderConfig<B>,
        sample_pool: SamplePool,
        byte_pool: BytePool,
        host_sample_rate: &Arc<AtomicU32>,
    ) -> Self {
        Self {
            byte_pool,
            decoder,
            sample_pool,
            host_sample_rate: Arc::clone(host_sample_rate),
        }
    }

    fn decode_init(
        &self,
        decoder: Box<dyn Decoder>,
        decoder_factory: StreamDecoderFactory,
        media_info: Option<MediaInfo>,
    ) -> DecodeInit {
        DecodeInit {
            decoder,
            decoder_factory,
            media_info,
            decoder_backend: self.backend(),
            gapless_mode: self.decoder.gapless_mode(),
            host_sample_rate: Arc::clone(&self.host_sample_rate),
            sample_pool: self.sample_pool.clone(),
            playback_resampler_backend: self.playback_resampler_backend(),
            // A requested host rate always resolves to a resampler plan, so a
            // route change is decided by `ResumeCursor`'s rate guards alone.
            recreate_on_host_rate_change: true,
        }
    }

    /// Publish the initial decoder state before the source becomes registrable.
    /// A registered source may emit `FormatDetected` on its first worker pass.
    fn publish_initial_events(
        &self,
        bus: &EventBus,
        media_info: Option<&MediaInfo>,
        spec: AudioSpec,
        track_info: &kithara_decode::DecoderTrackInfo,
        duration: Option<kithara_platform::time::Duration>,
    ) {
        bus.publish(decoder_changed_event(DecoderChangedEventData {
            media_info,
            spec,
            track_info,
            duration,
            backend: self.backend(),
            epoch: 0,
            cause: DecoderChangeCause::Initial,
            base_offset: 0,
        }));
        if let Some(event) =
            decoder_gapless_event(media_info, spec, track_info, FrameDomain::Output)
        {
            bus.publish(event);
        }
        let resampler = self.resampler_config();
        if let Some(event) = decoder_resampler_event(
            resampler.as_ref(),
            spec,
            media_info.and_then(|info| info.sample_rate),
        ) {
            bus.publish(event);
        }
        if let Some(host_rate) = NonZeroU32::new(self.host_sample_rate.load(Ordering::Acquire))
            && let Some(resampler) = resampler.as_ref()
            && let Some(event) = playback_resampler_event(
                &resampler.backend,
                host_rate.get(),
                media_info.and_then(|info| info.sample_rate),
            )
        {
            bus.publish(event);
        }
    }

    fn resampler_config(&self) -> Option<DecoderResamplerConfig<B>> {
        let target_sample_rate = NonZeroU32::new(self.host_sample_rate.load(Ordering::Acquire));
        self.decoder.build_resampler_config(target_sample_rate)
    }

    delegate::delegate! {
        to self.decoder {
            fn backend(&self) -> kithara_decode::DecoderBackend;
            #[call(resampler_backend_name)]
            fn playback_resampler_backend(&self) -> &'static str;
        }
    }
}

struct FactoryDeps<B> {
    epoch: Arc<AtomicU64>,
    decoder: DecoderDeps<B>,
    /// The caller's `MediaInfo` declaration, kept for the life of the track.
    /// Every decoder built for it resolves through the same precedence as the
    /// initial one — a per-variant plan describes the variant, not the bytes
    /// the caller told us to expect.
    user_media_info: Option<MediaInfo>,
}

impl<B> FactoryDeps<B>
where
    B: ResamplerBackend,
{
    fn new(
        decoder: &DecoderDeps<B>,
        epoch: &Arc<AtomicU64>,
        user_media_info: Option<MediaInfo>,
    ) -> Self {
        Self {
            user_media_info,
            decoder: DecoderDeps::clone(decoder),
            epoch: Arc::clone(epoch),
        }
    }
}

struct StreamSourceRegistration<S> {
    preload_gate: Arc<super::PreloadGate>,
    lane: PreparedAudioLane<S>,
    ring: RingParts,
}

struct PreparedStreamSource<S> {
    preload_gate: Arc<super::PreloadGate>,
    lane: PreparedAudioLane<S>,
    ring: RingParts,
}

impl<T> Audio<Stream<T>>
where
    T: StreamType<Events = EventBus>,
{
    #[must_use]
    /// Returns the unified event bus used by the stream and audio pipeline.
    pub fn event_bus(&self) -> &EventBus {
        AudioSession::event_bus(self)
    }

    #[must_use]
    /// Subscribes to unified stream and audio events.
    pub fn events(&self) -> kithara_events::EventReceiver {
        self.event_bus().subscribe()
    }

    /// Prepare a stream-backed audio reader and its concrete producer lane.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when stream, probe, decoder, or runtime setup fails.
    #[doc(hidden)]
    pub async fn prepare<B>(
        config: AudioConfig<T, B>,
        wake: Arc<dyn WorkerWake>,
        byte_pool: BytePool,
        sample_pool: SamplePool,
    ) -> Result<PreparedAudio<Self, impl crate::AudioSource<Chunk = AudioChunk>>, DecodeError>
    where
        B: Default + ResamplerBackend,
    {
        let AudioConfig {
            hint,
            host_sample_rate: config_host_sr,
            media_info: user_media_info,
            audio_buffer_chunks,
            observer,
            decoder,
            preload_chunks,
            block_on_underrun,
            consumer_wake_mode,
            stream: stream_config,
            bus: config_bus,
            cancel: config_cancel,
        } = config;
        let cancel = CancelScope::new(config_cancel).token();
        let runtime_handle = current_runtime_handle()?;

        let bus = resolve_event_bus::<T>(&stream_config, config_bus);
        let stream = create_stream_with_probe::<T>(stream_config, byte_pool.clone()).await?;
        let playhead = stream.playhead_write();
        let seek = stream.seek_control();
        let seek_obs = stream.seek_observe();
        let initial_media_info =
            merge_user_and_stream_media_info(user_media_info.clone(), stream.media_info());
        debug!(?initial_media_info, "Initial MediaInfo from stream");

        let variant_control = stream.variant_control();
        let shared_stream = SharedStream::new(stream);
        let host_sample_rate = Arc::new(AtomicU32::new(config_host_sr.map_or(0, NonZeroU32::get)));
        warm_sample_pool(
            &sample_pool,
            warm_channels(initial_media_info.as_ref()),
            audio_buffer_chunks,
        );

        let deps = DecoderDeps::new(
            decoder,
            sample_pool.clone(),
            byte_pool.clone(),
            &host_sample_rate,
        );
        let initial_reader = shared_stream.open_initial_reader();
        let decoder =
            create_initial_decoder(initial_reader, initial_media_info.clone(), hint, &deps).await;
        let decoder = decoder?;

        let initial_spec = decoder.spec();
        let initial_track_info = decoder.track_info();
        let total_duration = decoder.duration().or_else(|| playhead.duration());
        playhead.set_duration(total_duration);
        let metadata = decoder.metadata();
        let epoch = Arc::new(AtomicU64::new(0));
        log_pipeline_ready(initial_spec, &host_sample_rate);

        let abr_handle = shared_stream.abr_handle();
        let peer_wake = shared_stream.peer_wake();
        let seek_prepare = shared_stream.seek_prepare();
        let emit = AudioEvents::deferred(&bus);
        deps.publish_initial_events(
            &bus,
            initial_media_info.as_ref(),
            initial_spec,
            &initial_track_info,
            total_duration,
        );
        let wake_stream = shared_stream.clone();
        let preload_gate = Arc::new(super::PreloadGate::default());
        let (port, ring) = prepare_pcm_ring(
            audio_buffer_chunks,
            &emit,
            &epoch,
            block_on_underrun,
            consumer_wake_mode,
        );
        let decode = deps
            .decode_init(
                decoder,
                create_decoder_factory(&deps, &epoch, user_media_info),
                initial_media_info.clone(),
            )
            .into_parts(observer, shared_stream.seek_observe().epoch());
        let parts = SourceParts::new(
            &shared_stream,
            decode,
            Arc::clone(&epoch),
            RebuildRuntime {
                handle: runtime_handle,
                wake: Arc::clone(&wake),
            },
            variant_control,
        );
        let source = StreamAudioSource::new(shared_stream, parts).with_emit(Arc::clone(&emit));
        let lane = PreparedAudioLane {
            source,
            port,
            preload_gate: Arc::clone(&preload_gate),
            preload_chunks: preload_chunks.get(),
            playhead: Arc::clone(&playhead) as Arc<dyn PlayheadWrite>,
            emit: Arc::clone(&emit),
        };
        let registration = prepare_stream_source_registration(lane, ring, preload_gate);
        let prepared = prepare_stream_audio_source(registration, &wake_stream, Arc::clone(&wake));

        let audio = Self::from(AudioParts {
            sample_pool,
            emit,
            ring: RingConsumer::new(prepared.ring),
            runtime: AudioRuntime { wake, cancel },
            session: Session {
                playhead,
                seek,
                seek_obs,
                metadata,
                abr_handle,
                peer_wake,
                seek_prepare,
                preload_gate: prepared.preload_gate,
            },
            controls: Controls { host_sample_rate },
            spec: initial_spec,
            marker: PhantomData,
        });
        Ok(PreparedAudio::new(audio, prepared.lane))
    }
}

fn current_runtime_handle() -> Result<RuntimeHandle, DecodeError> {
    RuntimeHandle::try_current().map_err(|error| DecodeError::Io {
        source: IoError::other(format!(
            "audio stream construction requires a tokio runtime: {error}"
        )),
    })
}

fn prepare_pcm_ring(
    audio_buffer_chunks: usize,
    emit: &Arc<kithara_events::DeferredBus<Event>>,
    epoch: &Arc<AtomicU64>,
    block_on_underrun: bool,
    consumer_wake_mode: ConsumerWakeMode,
) -> (ProducerPort, RingParts) {
    let reader_wake = Arc::new(ThreadWake::default());
    let (data_tx, data_rx) = create_channels(audio_buffer_chunks, Arc::clone(emit), &reader_wake);
    let (trash_tx, trash_inlet) = create_trash_channel(audio_buffer_chunks);
    let ring = RingParts {
        block_on_underrun,
        consumer_wake_mode,
        trash_tx,
        reader_wake,
        audio_rx: data_rx,
        epoch: Arc::clone(epoch),
    };
    (ProducerPort::new(data_tx, trash_inlet), ring)
}

fn prepare_stream_source_registration<S>(
    lane: PreparedAudioLane<S>,
    ring: RingParts,
    preload_gate: Arc<super::PreloadGate>,
) -> StreamSourceRegistration<S> {
    StreamSourceRegistration {
        preload_gate,
        lane,
        ring,
    }
}

fn prepare_stream_audio_source<T, S>(
    registration: StreamSourceRegistration<S>,
    shared_stream: &SharedStream<T>,
    worker_wake: Arc<dyn WorkerWake>,
) -> PreparedStreamSource<S>
where
    T: StreamType,
{
    shared_stream.set_worker_wake(worker_wake);

    PreparedStreamSource {
        preload_gate: registration.preload_gate,
        ring: registration.ring,
        lane: registration.lane,
    }
}

fn create_decoder_factory<B>(
    decoder: &DecoderDeps<B>,
    epoch: &Arc<AtomicU64>,
    user_media_info: Option<MediaInfo>,
) -> StreamDecoderFactory
where
    B: Default + ResamplerBackend,
{
    let configured_media_info = user_media_info.clone();
    let deps = FactoryDeps::new(decoder, epoch, user_media_info);
    StreamDecoderFactory::new(
        move |mut reader, info| {
            let byte_len = reader.byte_len().unwrap_or(0);
            let byte_len_handle = Arc::new(AtomicU64::new(byte_len));
            let config = DecoderConfig::builder()
                .backend(deps.decoder.decoder.backend())
                .byte_len_handle(byte_len_handle)
                .sample_pool(deps.decoder.sample_pool.clone())
                .byte_pool(deps.decoder.byte_pool.clone())
                .epoch(deps.epoch.load(Ordering::Acquire))
                .maybe_byte_map(reader.byte_map())
                .maybe_hooks(reader.take_event_sink())
                .maybe_resampler(deps.decoder.resampler_config())
                .build();
            let source = reader.into_inner();
            let info = match deps.user_media_info.clone() {
                Some(user) => merge_media_info(user, &info),
                None => info,
            };
            match DecoderFactory::create_from_media_info(source, &info, config) {
                Ok(decoder) => {
                    decoder.update_byte_len(byte_len);
                    Ok(decoder)
                }
                Err(error) => {
                    warn!(?error, "failed to recreate decoder");
                    Err(error)
                }
            }
        },
        configured_media_info,
    )
}

async fn create_initial_decoder<B>(
    mut reader: OpenedReader,
    media_info: Option<MediaInfo>,
    hint: Option<String>,
    deps: &DecoderDeps<B>,
) -> Result<Box<dyn Decoder>, DecodeError>
where
    B: Default + ResamplerBackend,
{
    let byte_len = reader.byte_len().unwrap_or(0);
    let construction_gate = reader.construction_gate();
    let config = DecoderConfig::builder()
        .backend(deps.decoder.backend())
        .byte_len_handle(Arc::new(AtomicU64::new(byte_len)))
        .sample_pool(deps.sample_pool.clone())
        .byte_pool(deps.byte_pool.clone())
        .maybe_byte_map(reader.byte_map())
        .maybe_hooks(reader.take_event_sink())
        .maybe_hint(hint.clone())
        .maybe_resampler(deps.resampler_config())
        .build();
    let source = reader.into_inner();
    if let Some(gate) = &construction_gate {
        gate.arm();
    }
    let built = spawn_blocking(move || {
        if let Some(info) = &media_info {
            DecoderFactory::create_from_media_info(source, info, config)
        } else {
            DecoderFactory::create_with_probe(source, hint.as_deref(), config)
        }
    })
    .await;
    if let Some(gate) = &construction_gate {
        gate.disarm();
    }
    built.map_err(|error| DecodeError::Io {
        source: IoError::other(format!("decoder task panicked: {error}")),
    })?
}

async fn create_stream_with_probe<T>(
    stream_config: T::Config,
    byte_pool: BytePool,
) -> Result<Stream<T>, DecodeError>
where
    T: StreamType,
{
    let stream = Stream::<T>::new(stream_config)
        .await
        .map_err(|error| DecodeError::Io {
            source: IoError::other(error.to_string()),
        })?;
    probe(stream, byte_pool).await
}

#[cfg(not(target_arch = "wasm32"))]
async fn probe<T>(stream: Stream<T>, byte_pool: BytePool) -> Result<Stream<T>, DecodeError>
where
    T: StreamType,
{
    spawn_blocking(move || probe_blocking(stream, &byte_pool))
        .await
        .map_err(|error| DecodeError::Io {
            source: IoError::other(format!("probe task panicked: {error}")),
        })?
}

#[cfg(target_arch = "wasm32")]
async fn probe<T>(stream: Stream<T>, byte_pool: BytePool) -> Result<Stream<T>, DecodeError>
where
    T: StreamType,
{
    probe_blocking(stream, &byte_pool)
}

fn probe_blocking<T>(mut stream: Stream<T>, _byte_pool: &BytePool) -> Result<Stream<T>, DecodeError>
where
    T: StreamType,
{
    stream
        .seek(SeekFrom::Start(0))
        .map_err(|source| DecodeError::Io { source })?;
    Ok(stream)
}

fn resolve_event_bus<T>(stream_config: &T::Config, configured: Option<EventBus>) -> EventBus
where
    T: StreamType<Events = EventBus>,
{
    T::event_bus(stream_config)
        .or(configured)
        .unwrap_or_default()
}

fn warm_channels(info: Option<&MediaInfo>) -> usize {
    info.and_then(|info| info.channels).map_or(2, usize::from)
}

fn warm_sample_pool(pool: &SamplePool, channels: usize, chunks: usize) {
    if pool.allocated_bytes() != 0 {
        return;
    }
    let capacity = WARM_DECODE_FRAMES * channels.max(1);
    pool.pre_warm(chunks.saturating_mul(2).max(1), |buffer| {
        buffer.clear();
        buffer.resize(capacity, 0.0);
    });
}

fn log_pipeline_ready(spec: AudioSpec, host_sample_rate: &Arc<AtomicU32>) {
    info!(
        ?spec,
        host_sr = host_sample_rate.load(Ordering::Relaxed),
        "Audio pipeline created"
    );
}

/// Fill the caller's unset fields from what the source reports. The caller's
/// declaration wins: they know the bytes, the source only knows what its
/// container or playlist claims about them.
const fn merge_media_info(mut user: MediaInfo, stream: &MediaInfo) -> MediaInfo {
    if user.codec.is_none() {
        user.codec = stream.codec;
    }
    if user.container.is_none() {
        user.container = stream.container;
    }
    if user.channels.is_none() {
        user.channels = stream.channels;
    }
    if user.sample_rate.is_none() {
        user.sample_rate = stream.sample_rate;
    }
    if user.variant_index.is_none() {
        user.variant_index = stream.variant_index;
    }
    user
}

const fn merge_user_and_stream_media_info(
    user: Option<MediaInfo>,
    stream: Option<MediaInfo>,
) -> Option<MediaInfo> {
    match (user, stream) {
        (Some(user), Some(stream)) => Some(merge_media_info(user, &stream)),
        (Some(user), None) => Some(user),
        (None, stream) => stream,
    }
}

#[cfg(test)]
mod tests {
    use kithara_events::DeferredBus;
    use kithara_signal::AudioChunk;
    use kithara_stream::PlayheadState;
    use kithara_test_utils::kithara;
    use unimock::Unimock;

    use super::*;
    use crate::{ConsumerWakeMode, traits::AudioSource};

    #[kithara::test]
    fn prepares_source_registration_without_worker_activity() {
        let emit = Arc::new(DeferredBus::new(EventBus::new(8), 8));
        let epoch = Arc::new(AtomicU64::new(0));
        let (port, ring) =
            prepare_pcm_ring(1, &emit, &epoch, false, ConsumerWakeMode::RealtimeDeferred);
        let preload_gate = Arc::new(super::super::PreloadGate::default());
        let source: Box<dyn AudioSource<Chunk = AudioChunk>> = Box::new(Unimock::new(()));
        let lane = PreparedAudioLane {
            source,
            port,
            emit,
            preload_gate: Arc::clone(&preload_gate),
            preload_chunks: 1,
            playhead: Arc::new(PlayheadState::new()) as Arc<dyn PlayheadWrite>,
        };

        let registration =
            prepare_stream_source_registration(lane, ring, Arc::clone(&preload_gate));

        assert!(Arc::ptr_eq(&registration.preload_gate, &preload_gate));
    }
}
