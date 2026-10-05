use std::num::NonZeroU32;

use delegate::delegate;
use kithara_audio::{
    AudioObserver, AudioReader, ChunkOutcome, ConsumerWakeMode, ReadOutcome, ResamplerBackend,
    SeekOutcome,
};
use kithara_bufpool::HasPool;
use kithara_command::Sender;
use kithara_decode::{DecodeError, DecodeResult, TrackMetadata};
use kithara_events::{EventBus, EventReceiver, EventSet};
use kithara_platform::{CancelToken, sync::Arc, time::Duration, tokio::task};
use kithara_render::LaneProtocol;
use kithara_signal::AudioSpec;
use kithara_stream::{Stream, StreamType};
use kithara_warp::{
    PresentationFrontier, RenderContext, RenderPublisher, RenderReader, supports_playback_rate,
};
use tracing::warn;

use super::{
    ArtifactFetch, ArtifactSource, PreparedGrid, ResourceConfig, SourceType, StagingRecipe,
};
use crate::{
    PlayWorker, TrackConfig,
    worker::{ServiceClass, TrackPriority},
};

/// The prepared beat grid this load starts with, and the read that fills it
/// in when the track named a source instead of handing one over.
///
/// The read is deliberately not awaited here. A track whose audio is ready
/// becomes playable at once; its grid arrives when its own source answers,
/// into the very slot this load handed out. A load that is over has dropped
/// that slot, so a late answer reaches nobody.
fn prepared_grid<S, B>(config: &ResourceConfig<S, B>) -> Arc<PreparedGrid>
where
    B: Default,
    S: HasPool<u8> + Send + Sync + 'static,
{
    match config.beat_grid() {
        None => Arc::default(),
        Some(ArtifactSource::Value(model)) => Arc::new(PreparedGrid::holding(Arc::clone(model))),
        Some(source) => {
            let slot = Arc::new(PreparedGrid::default());
            let read = slot.clone();
            let source = source.clone();
            let audio = config.src.clone();
            let downloader = config.downloader.clone();
            let headers = config.headers.clone();
            let cancel = config.cancel.clone();
            drop(task::spawn(async move {
                let fetch = ArtifactFetch::new(
                    &audio,
                    downloader.as_ref(),
                    headers.as_ref(),
                    cancel.as_ref(),
                );
                match source.load(&fetch).await {
                    Ok(model) => read.put(model),
                    Err(error) => warn!(%error, "resource: the prepared beat grid never arrived"),
                }
            }));
            slot
        }
    }
}

/// Type-erased audio resource wrapping any `AudioReader`.
///
/// Provides a unified interface for reading decoded audio
/// regardless of the underlying source (file, HLS, custom).
///
/// # Example
///
/// ```ignore
/// use kithara_assets::AssetStore;
/// use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
/// use kithara_play::{PlayWorker, PlayWorkerConfig, Resource, ResourceConfig, ResourceSrc};
///
/// pool_schema! {
///     pub AppPools {
///         bytes: u8,
///         samples: f32,
///     }
/// }
/// let config = || PoolConfig::builder().max_buffers(128).build();
/// let pools = AppPools::builder(OverallBudget(64 * 1024 * 1024))
///     .bytes(config())
///     .samples(config())
///     .build()?;
/// let worker = PlayWorker::new(PlayWorkerConfig::builder(pools.clone()).build());
///
/// // Auto-detect: .m3u8 -> HLS, everything else -> progressive file
/// let config: ResourceConfig<AppPools> = ResourceConfig::for_src(ResourceSrc::parse(
///     "https://example.com/song.mp3",
/// )?)
/// .store(AssetStore::builder(pools).build())
/// .worker(worker)
/// .build();
/// let mut resource = Resource::new(config).await?;
///
/// let spec = resource.spec();
/// let meta = resource.metadata();
///
/// let mut buf = [0.0f32; 1024];
/// resource.read(&mut buf);
/// ```
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct Resource {
    /// The prepared beat grid of this load. Empty for a track opened without
    /// one, and empty until the read answers for a track opened with a source.
    #[field(get, deref = false)]
    beat_grid: Arc<PreparedGrid>,
    #[field(get, deref = false)]
    src: Arc<str>,
    #[field(get = event_bus)]
    bus: EventBus,
    priority: Option<TrackPriority>,
    /// Player end of the render lane of a reader opened on a play worker.
    lane: Option<Sender<LaneProtocol>>,
    render_publisher: Option<RenderPublisher>,
    #[field(with)]
    playback_rate: PlaybackRate,
    /// How to open a staged lane of this recording; `None` for a reader
    /// handed over whole, or where no renderer can enter a plan.
    staging: Option<StagingRecipe>,
    reader: ReaderOwner,
}

/// Cancels the wrapped per-track token on drop. A `Resource` field rather than
/// a `Resource: Drop` impl so the `From<Resource>` reader unwrap can move
/// `inner` out of the wrapper after [`disarm`](CancelGuard::disarm)ing. Passive
/// when `None`.
struct CancelGuard(Option<CancelToken>);

/// Cancels before dropping the reader; tuple fields drop in declaration order.
struct ReaderOwner(CancelGuard, Box<dyn AudioReader>);

/// Media seconds a reader consumes per output second.
enum PlaybackRate {
    /// Its own tempo: no renderer changes its speed.
    Fixed,
    /// The speed its renderer was last asked for.
    Warp(f32),
}

impl PlaybackRate {
    fn apply(&mut self, requested: f32) -> f32 {
        if let Self::Warp(rate) = self {
            *rate = requested;
        }
        f32::from(&*self)
    }

    fn for_warp(speed: f32) -> Self {
        if supports_playback_rate() {
            Self::Warp(speed)
        } else {
            Self::Fixed
        }
    }
}

impl From<&PlaybackRate> for f32 {
    fn from(rate: &PlaybackRate) -> Self {
        match rate {
            PlaybackRate::Fixed => 1.0,
            PlaybackRate::Warp(rate) => *rate,
        }
    }
}

impl CancelGuard {
    /// Disarm so dropping the guard cancels nothing — used when the live reader
    /// outlives this wrapper (handed to the analysis worker), where teardown
    /// rides the analysis run-scope cancel (a parent of this token) instead.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.cancel();
        }
    }
}

impl Resource {
    /// Create a resource from a `ResourceConfig`.
    ///
    /// Auto-detects the stream type from the URL:
    /// - URLs ending with `.m3u8` -> HLS stream
    /// - All other URLs -> progressive file download
    ///
    /// # Errors
    ///
    /// Returns an error if source type detection fails, or if the underlying
    /// audio stream cannot be created (network failure, invalid format, etc.).
    pub async fn new<S, B>(config: ResourceConfig<S, B>) -> DecodeResult<Self>
    where
        B: Default + ResamplerBackend,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    {
        Self::open(config, None).await
    }

    pub(crate) fn apply_playback_rate(&mut self, rate: f32) -> f32 {
        self.playback_rate.apply(rate)
    }

    pub(crate) fn staging(&self) -> Option<StagingRecipe> {
        self.staging.clone()
    }

    pub(crate) fn clear_render(&self) {
        if let Some(publisher) = &self.render_publisher {
            publisher.clear();
        }
    }

    /// Create a resource from any `AudioReader`.
    ///
    /// Custom sources are fixed-rate. Stream-backed resources reuse this
    /// construction path and attach their resident Warp controls before return.
    ///
    /// The resource shares the reader's event bus directly.
    ///
    /// `src` rides along on `PlayerEvent::ItemDidPlayToEnd` and is what
    /// the queue uses to tell which track ended. `None` defaults to
    /// `"unknown"`.
    #[must_use]
    pub fn from_reader<R: AudioReader + 'static>(reader: R, src: Option<Arc<str>>) -> Self {
        let preload = reader.preload_gate().is_none();
        let bus = reader.event_bus().clone();
        let inner: Box<dyn AudioReader> = Box::new(reader);
        let src = src.unwrap_or_else(|| Arc::from("unknown"));
        let mut resource = Self {
            src,
            bus,
            priority: None,
            playback_rate: PlaybackRate::Fixed,
            reader: ReaderOwner(CancelGuard(None), inner),
            lane: None,
            render_publisher: None,
            staging: None,
            beat_grid: Arc::default(),
        };
        if preload && let Err(error) = resource.reader.1.preload() {
            warn!(src = %resource.src, %error, "resource preload failed");
        }
        resource
    }

    /// Create a resource from a concrete stream-backed audio config.
    ///
    /// Generic over any [`StreamType`] whose config carries an optional
    /// `kithara_events::EventBus`. Callers wanting fine-grained control
    /// over `FileConfig` / `HlsConfig` (ABR, keys, etc.) use this path.
    pub(crate) async fn from_stream_audio<T, B, S>(
        config: TrackConfig<T, B>,
        src: Arc<str>,
        worker: &PlayWorker<S>,
    ) -> DecodeResult<Self>
    where
        T: StreamType<Events = EventBus> + 'static,
        B: Default + ResamplerBackend,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
        crate::RegisteredAudio<Stream<T>, S>: AudioReader + 'static,
    {
        let speed = config.warp().speed();
        let mut audio = worker.load(config).await?;
        let priority = audio.priority();
        let render_publisher = audio.take_publisher().ok_or(DecodeError::InvalidData {
            detail: "registered Warp publisher was already taken",
        })?;
        let lane = audio.take_lane().ok_or(DecodeError::InvalidData {
            detail: "registered render lane was already taken",
        })?;
        let mut resource =
            Self::from_reader(audio, Some(src)).with_playback_rate(PlaybackRate::for_warp(speed));
        if let Err(error) = resource.preload().await {
            warn!(src = %resource.src, %error, "resource preload failed");
        }
        resource.priority = Some(priority);
        resource.lane = Some(lane);
        resource.render_publisher = Some(render_publisher);
        Ok(resource)
    }

    /// Create a resource with a bounded observer of decoded audio attached.
    ///
    /// This is a narrow cross-crate composition seam used by queue-owned
    /// orchestration. The ordinary resource API remains [`Self::new`].
    #[doc(hidden)]
    pub async fn new_observed<S, B>(
        config: ResourceConfig<S, B>,
        observer: Box<dyn AudioObserver>,
    ) -> DecodeResult<Self>
    where
        B: Default + ResamplerBackend,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    {
        Self::open(config, Some(observer)).await
    }

    /// Captures the per-track cancel token before `build_*_config` consumes `config`; the same
    /// token is cloned by identity into both the inner stream and the audio path.
    async fn open<S, B>(
        config: ResourceConfig<S, B>,
        observer: Option<Box<dyn AudioObserver>>,
    ) -> DecodeResult<Self>
    where
        B: Default + ResamplerBackend,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    {
        let src: Arc<str> = Arc::from(config.src.to_string());
        let beat_grid = prepared_grid(&config);
        let source_type = SourceType::detect(&config.src)?;
        let worker = config.worker.clone().ok_or(DecodeError::InvalidData {
            detail: "ResourceConfig requires an explicit PlayWorker",
        })?;
        let warp = config.warp.clone();
        let engine_load = config.engine_load.clone();
        let cancel = config.cancel.clone();
        let staging = StagingRecipe::new(&config, &worker);
        let mut resource = match source_type {
            SourceType::RemoteFile(_) | SourceType::LocalFile(_) => {
                let audio_config = config.build_file_config(&worker, observer);
                let track = TrackConfig::for_audio(audio_config)
                    .maybe_engine_load(engine_load)
                    .warp(warp.clone())
                    .build();
                Self::from_stream_audio(track, src, &worker).await?
            }
            SourceType::HlsStream(_) => {
                let audio_config = config.build_hls_config(&worker, observer)?;
                let track = TrackConfig::for_audio(audio_config)
                    .maybe_engine_load(engine_load)
                    .warp(warp)
                    .build();
                Self::from_stream_audio(track, src, &worker).await?
            }
        };
        resource.reader.0 = CancelGuard(cancel);
        resource.beat_grid = beat_grid;
        resource.staging = staging;
        Ok(resource)
    }

    pub(crate) fn playback_rate(&self) -> f32 {
        (&self.playback_rate).into()
    }

    /// Wait for first decoded chunk to be available, then move it to internal buffer.
    ///
    /// After preload completes, the first `read()` returns data without blocking.
    /// Safe to call multiple times (no-op if already preloaded).
    ///
    /// # Errors
    /// Propagated from the underlying [`kithara_audio::AudioControl::preload`] if the
    /// producer channel closed or the initial fill hit a decoder
    /// failure.
    pub async fn preload(&mut self) -> Result<(), DecodeError> {
        if let Some(gate) = self.reader.1.preload_gate() {
            gate.wait_for_epoch(self.reader.1.preload_epoch()).await;
        }
        self.reader.1.preload()
    }

    pub(crate) fn publish_render(&self, context: &RenderContext, frontier: PresentationFrontier) {
        if let Some(publisher) = &self.render_publisher {
            publisher.publish(context, frontier);
        }
    }

    pub(crate) fn take_lane(&mut self) -> Option<Sender<LaneProtocol>> {
        self.lane.take()
    }

    pub(crate) fn render_reader(&self) -> Option<RenderReader> {
        self.render_publisher.as_ref().map(RenderPublisher::reader)
    }

    pub(crate) fn set_service_class(&self, class: ServiceClass) {
        if let Some(priority) = &self.priority {
            priority.set(class);
        }
    }

    /// Subscribe to unified events.
    ///
    /// Returns a receiver for all events published to the bus,
    /// including audio, file, and HLS events.
    #[must_use]
    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }

    delegate! {
        to self.reader.1 {
            /// Runtime ABR handle for adaptive sources (HLS). `None` for files.
            #[must_use]
            pub fn abr_handle(&self) -> Option<kithara_abr::AbrHandle>;
            /// Cached span of the underlying reader: how much of the source is on disk.
            #[must_use]
            pub fn cached_span(&self) -> Duration;
            /// Decoded-ahead frontier of the underlying reader (always `>=` position).
            #[must_use]
            pub fn decoded_frontier(&self) -> Duration;
            /// Get total duration (if known).
            #[must_use]
            pub fn duration(&self) -> Option<Duration>;
            /// Get track metadata.
            #[must_use]
            pub fn metadata(&self) -> &TrackMetadata;
            /// Read the next decoded chunk with full metadata.
            pub fn next_chunk(&mut self) -> Result<ChunkOutcome, DecodeError>;
            /// Get current playback position.
            #[must_use]
            pub fn position(&self) -> Duration;
            /// Read interleaved samples.
            pub fn read(&mut self, buf: &mut [f32]) -> Result<ReadOutcome, DecodeError>;
            /// Read deinterleaved (planar) samples.
            pub fn read_planar<'a>(
                &mut self,
                output: &'a mut [&'a mut [f32]],
            ) -> Result<ReadOutcome, DecodeError>;
            /// Seek to position. Begins and applies in one call, so it takes locks — off the audio
            /// thread only. Audio-thread callers begin through [`seek_handle`](Self::seek_handle)
            /// instead.
            pub fn seek(&mut self, position: Duration) -> Result<SeekOutcome, DecodeError>;
            /// Control-plane handle that begins a seek without touching the reader. `None` for
            /// readers with no worker-backed seek.
            #[must_use]
            pub fn seek_handle(&self) -> Option<Arc<dyn kithara_audio::SeekBegin>>;
            /// Adopt the wake capability of the consumer that will read this
            /// resource.
            pub fn set_consumer_wake_mode(&mut self, mode: ConsumerWakeMode);
            /// Adopt a seek epoch begun through `seek_handle`. Lock-free.
            pub fn sync_seek(&mut self);
            /// Set the target sample rate of the audio host.
            pub fn set_host_sample_rate(&self, sample_rate: NonZeroU32);
            /// Get the current decoded-audio specification.
            #[must_use]
            pub fn spec(&self) -> AudioSpec;
        }
    }
}

/// Unwrap a `Resource` into its underlying reader, e.g. to hand the opened
/// source to the shared `kithara-analysis` worker.
///
/// Disarms the per-track cancel before moving the reader out: the live reader
/// outlives this wrapper, so freeing the wrapper must not tear down its fetch
/// loops. Teardown then rides the analysis run-scope cancel.
impl From<Resource> for Box<dyn AudioReader> {
    fn from(resource: Resource) -> Self {
        let Resource { reader, .. } = resource;
        let ReaderOwner(mut cancel, inner) = reader;
        cancel.disarm();
        inner
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::{NonZeroU32, NonZeroUsize},
        sync::atomic::{AtomicU8, Ordering},
    };

    use firewheel::{
        clock::InstantSamples,
        dsp::{buffer::ConstSequentialBuffer, declick::DeclickValues},
        event::{NodeEvent, ProcEvents, ProcEventsIndex, ScheduledEventEntry},
        log::{RealtimeLoggerConfig, realtime_logger},
        mask::{ConnectedMask, ConstantMask, SilenceMask},
        node::{
            AudioNodeProcessor, NUM_SCRATCH_BUFFERS, ProcBuffers, ProcExtra, ProcInfo, ProcStore,
            StreamStatus,
        },
    };
    use kithara_audio::{AudioControl, AudioRead, AudioSession, ReadOutcome, SeekOutcome};
    use kithara_bufpool::PoolRegion;
    use kithara_decode::TrackMetadata;
    use kithara_events::TrackId;
    use kithara_platform::{CancelToken, sync::Arc};
    use kithara_signal::{AudioSpec, OutputContext, SessionEpoch, SessionFrame};
    use kithara_test_fixtures::play_fixtures::half;
    use kithara_test_utils::kithara;
    use kithara_warp::{Warp, WarpConfig};
    use ringbuf::traits::Consumer;

    use super::*;
    use crate::{
        bridge::{DeckPart, PlayerNotification, SharedEq, TrackTransition, slot_channels},
        consts,
        rt::{DeckMixer, DeckMixerConfig, StreamShape, track::PlayerResource},
        test_pools::{TestPools, pools},
    };

    struct DropProbe {
        state: Arc<AtomicU8>,
        cancel: CancelToken,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            let state = if self.cancel.is_cancelled() {
                consts::DROPPED_AFTER_CANCEL
            } else {
                consts::DROPPED_BEFORE_CANCEL
            };
            self.state.store(state, Ordering::SeqCst);
        }
    }

    struct EofReader {
        spec: AudioSpec,
        bus: EventBus,
        _drop_probe: Option<DropProbe>,
        meta: TrackMetadata,
        samples: Vec<f32>,
        position_frames: usize,
        total_frames: usize,
    }

    impl Default for EofReader {
        fn default() -> Self {
            Self {
                bus: EventBus::default(),
                meta: TrackMetadata::default(),
                spec: AudioSpec::new(
                    2,
                    NonZeroU32::new(consts::SAMPLE_RATE).expect("static rate"),
                ),
                position_frames: 0,
                total_frames: 0,
                samples: Vec::new(),
                _drop_probe: None,
            }
        }
    }

    impl EofReader {
        fn eof(&self) -> ReadOutcome {
            ReadOutcome::Eof {
                position: self.position_duration(),
            }
        }

        fn position_duration(&self) -> Duration {
            let frames = u32::try_from(self.position_frames).expect("test frame count fits u32");
            Duration::from_secs_f64(f64::from(frames) / f64::from(consts::SAMPLE_RATE))
        }

        fn take_frames(&mut self, capacity: usize) -> Option<NonZeroUsize> {
            let frames = capacity.min(self.total_frames - self.position_frames);
            self.position_frames += frames;
            NonZeroUsize::new(frames)
        }

        fn with_drop_probe(cancel: CancelToken, state: Arc<AtomicU8>) -> Self {
            Self {
                _drop_probe: Some(DropProbe { state, cancel }),
                ..Self::default()
            }
        }

        fn with_frames(samples: Vec<f32>) -> Self {
            Self {
                total_frames: samples.len() / 2,
                samples,
                ..Self::default()
            }
        }
    }

    impl AudioSession for EofReader {
        fn duration(&self) -> Option<Duration> {
            let frames = u32::try_from(self.total_frames).expect("test frame count fits u32");
            Some(Duration::from_secs_f64(
                f64::from(frames) / f64::from(consts::SAMPLE_RATE),
            ))
        }
        fn event_bus(&self) -> &EventBus {
            &self.bus
        }
        fn metadata(&self) -> &TrackMetadata {
            &self.meta
        }
    }

    impl AudioRead for EofReader {
        fn position(&self) -> Duration {
            self.position_duration()
        }
        fn read(&mut self, buf: &mut [f32]) -> Result<ReadOutcome, DecodeError> {
            let Some(frames) = self.take_frames(buf.len() / 2) else {
                return Ok(self.eof());
            };
            let samples = frames.get() * 2;
            let end = self.position_frames * 2;
            buf[..samples].copy_from_slice(&self.samples[end - samples..end]);
            Ok(ReadOutcome::Frames {
                count: NonZeroUsize::new(samples).expect("non-zero stereo sample count"),
                position: self.position_duration(),
                source_span: None,
            })
        }
        fn read_planar<'a>(
            &mut self,
            output: &'a mut [&'a mut [f32]],
        ) -> Result<ReadOutcome, DecodeError> {
            let capacity = output.first().map_or(0, |channel| channel.len());
            let Some(frames) = self.take_frames(capacity) else {
                return Ok(self.eof());
            };
            let start = self.position_frames - frames.get();
            for (index, channel) in output.iter_mut().enumerate() {
                for (offset, sample) in channel[..frames.get()].iter_mut().enumerate() {
                    *sample = self.samples[(start + offset) * 2 + index];
                }
            }
            Ok(ReadOutcome::Frames {
                count: frames,
                position: self.position_duration(),
                source_span: None,
            })
        }

        fn spec(&self) -> AudioSpec {
            self.spec
        }
    }

    impl AudioControl for EofReader {
        fn seek(&mut self, position: Duration) -> Result<SeekOutcome, DecodeError> {
            Ok(SeekOutcome::Landed {
                target: position,
                landed_at: position,
            })
        }
    }

    fn warped_player_resource(
        pools: &PoolRegion<TestPools>,
        speed: f32,
        src: &str,
        samples: Vec<f32>,
    ) -> Box<PlayerResource> {
        let resource = Resource::from_reader(EofReader::with_frames(samples), None)
            .with_playback_rate(PlaybackRate::for_warp(speed));
        PlayerResource::new(resource, Arc::from(src), pools)
            .map_or_else(|error| panic!("test player resource: {error}"), Box::new)
    }

    fn process_block(processor: &mut DeckMixer, extra: &mut ProcExtra) {
        let info = ProcInfo {
            sample_rate: NonZeroU32::new(consts::SAMPLE_RATE).expect("static sample rate"),
            frames: consts::BLOCK_FRAMES,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            in_constant_mask: ConstantMask::default(),
            out_constant_mask: ConstantMask::default(),
            in_connected_mask: ConnectedMask::default(),
            out_connected_mask: ConnectedMask::default(),
            total_cpu_seconds_recip: 1.0,
            process_to_playback_delay: None,
            did_just_unbypass: false,
            last_marker_instant: InstantSamples(0),
            sample_rate_recip: f64::from(consts::SAMPLE_RATE).recip(),
            clock_samples: InstantSamples(0),
            duration_since_stream_start: Duration::ZERO,
            stream_status: StreamStatus::empty(),
            dropped_frames: 0,
        };
        let inputs: [&[f32]; 0] = [];
        let mut left = [0.0; consts::BLOCK_FRAMES];
        let mut right = [0.0; consts::BLOCK_FRAMES];
        let mut outputs = [&mut left[..], &mut right[..]];
        let buffers = ProcBuffers {
            inputs: &inputs,
            outputs: &mut outputs,
        };
        let mut immediate: [Option<NodeEvent>; 0] = [];
        let mut scheduled: [Option<ScheduledEventEntry>; 0] = [];
        let mut indices: Vec<ProcEventsIndex> = Vec::new();
        let mut events = ProcEvents::new(&mut immediate, &mut scheduled, &mut indices);
        processor.events(&info, &mut events, extra);
        let _ = processor.process(&info, buffers, extra);
    }

    fn rate_notifications(control: &mut crate::bridge::SlotControl) -> Vec<f32> {
        let mut rates = Vec::new();
        while let Some(notification) = control.notif_rx.try_pop() {
            if let PlayerNotification::RateChanged { rate } = notification {
                rates.push(rate);
            }
        }
        rates
    }

    #[kithara::test(native, flash(false))]
    fn playback_rate_reports_only_a_real_warp_control() {
        let mut fixed = Resource::from_reader(EofReader::default(), None);
        assert_eq!(fixed.apply_playback_rate(1.5), 1.0);
        assert_eq!(fixed.playback_rate(), 1.0);

        let mut warped = Resource::from_reader(EofReader::default(), None)
            .with_playback_rate(PlaybackRate::for_warp(1.25));
        let (built, applied) = if supports_playback_rate() {
            (1.25, 1.5)
        } else {
            (1.0, 1.0)
        };
        assert_eq!(warped.playback_rate(), built);
        assert_eq!(warped.apply_playback_rate(1.5), applied);
        assert_eq!(warped.playback_rate(), applied);
    }

    #[kithara::test(native, flash(false))]
    fn a_loaded_track_takes_the_processor_rate(half: Vec<f32>) {
        let pools = pools();
        let effective_rate = if supports_playback_rate() { 1.5 } else { 1.0 };
        let (inputs, mut control) = slot_channels(SharedEq::new(0));
        let shape = StreamShape {
            sample_rate: NonZeroU32::new(consts::SAMPLE_RATE).expect("static sample rate"),
            max_block_frames: NonZeroU32::new(
                u32::try_from(consts::BLOCK_FRAMES).expect("block size fits u32"),
            )
            .expect("static block size"),
        };
        let mut processor = DeckMixer::new(inputs, shape, &pools, DeckMixerConfig::default());
        let (logger, _logger_rx) = realtime_logger(RealtimeLoggerConfig::default());
        let mut extra = ProcExtra {
            logger,
            store: ProcStore::with_capacity(0),
            scratch_buffers: ConstSequentialBuffer::<f32, NUM_SCRATCH_BUFFERS>::new(
                consts::BLOCK_FRAMES,
            ),
            declick_values: DeclickValues::new(NonZeroU32::new(16).expect("static declick length")),
        };
        let first: Arc<str> = Arc::from("first");
        let first_id = TrackId::allocate();
        control
            .send(DeckPart::Attach {
                resource: warped_player_resource(&pools, 1.0, &first, half.clone()),
                item_id: first_id,
            })
            .expect("load first track");
        control
            .send(DeckPart::Fade(TrackTransition::FadeIn {
                item_id: first_id,
                settings: crate::CrossfadeSettings::default(),
                epoch: 0,
            }))
            .expect("fade in first track");
        control.send(DeckPart::StartAll).expect("start playback");
        process_block(&mut processor, &mut extra);
        let _ = rate_notifications(&mut control);

        control
            .send(DeckPart::SetRate(1.5))
            .expect("set the slot rate");
        let first_position = processor
            .track(first_id)
            .expect("first track loaded")
            .position();
        process_block(&mut processor, &mut extra);
        let first_advance = processor
            .track(first_id)
            .expect("first track loaded")
            .position()
            - first_position;
        let block_frames = u32::try_from(consts::BLOCK_FRAMES).expect("block size fits u32");
        let expected_advance =
            f64::from(block_frames) * f64::from(effective_rate) / f64::from(consts::SAMPLE_RATE);
        assert!((first_advance - expected_advance).abs() < f64::EPSILON);
        assert_eq!(processor.playback().rate.load(), effective_rate);
        let notifications = rate_notifications(&mut control);
        if supports_playback_rate() {
            assert_eq!(notifications, [1.5]);
        } else {
            assert!(notifications.is_empty());
        }

        let next: Arc<str> = Arc::from("next");
        let next_id = TrackId::allocate();
        control
            .send(DeckPart::Attach {
                resource: warped_player_resource(&pools, 1.0, &next, half),
                item_id: next_id,
            })
            .expect("load next track");
        control
            .send(DeckPart::Fade(TrackTransition::FadeIn {
                item_id: next_id,
                settings: crate::CrossfadeSettings::default(),
                epoch: 0,
            }))
            .expect("fade in next track");

        process_block(&mut processor, &mut extra);

        assert_eq!(processor.playback().rate.load(), effective_rate);
        assert_eq!(
            processor
                .track(next_id)
                .expect("next track loaded")
                .position(),
            expected_advance
        );
        assert!(rate_notifications(&mut control).is_empty());
    }

    /// Pin (W3 Task 3.3 (b)): a mid-session unload — i.e. dropping the
    /// `Resource` — cancels the whole per-track subtree, not just the `Audio`
    /// half. The per-track token `T` is passed by identity into both the inner
    /// stream (File/Hls) and the `Audio` config; under propagate-down both take
    /// `T.child()`, so `Audio::Drop` alone would only reach its own child and
    /// leave the stream-side fetch loops running. `Resource::Drop` must cancel
    /// `T` so the stream subtree (modelled here by `stream_sub`) is torn down.
    #[kithara::test(native, flash(false))]
    fn drop_cancels_whole_per_track_subtree_not_just_audio() {
        let track = CancelToken::never();
        let stream_sub = track.child(); // File/Hls subtree F = T.child()
        let audio_sub = track.child(); // Audio subtree A = T.child()

        let mut resource = Resource::from_reader(EofReader::default(), None);
        resource.reader.0 = CancelGuard(Some(track.clone()));

        assert!(!stream_sub.is_cancelled() && !audio_sub.is_cancelled());
        drop(resource);
        assert!(
            stream_sub.is_cancelled(),
            "unload must cancel the stream-side subtree, not only the Audio half"
        );
        assert!(audio_sub.is_cancelled());
        assert!(track.is_cancelled());
    }

    /// A resource with no per-track cancel wired in (custom reader) drops
    /// without panicking and cancels nothing.
    #[kithara::test(native, flash(false))]
    fn drop_without_cancel_is_passive() {
        let resource = Resource::from_reader(EofReader::default(), None);
        drop(resource);
    }

    #[kithara::test(native, flash(false))]
    fn drop_cancels_before_inner_reader_teardown() {
        let track = CancelToken::never();
        let state = Arc::new(AtomicU8::new(consts::NOT_DROPPED));
        let reader = EofReader::with_drop_probe(track.clone(), Arc::clone(&state));
        let mut resource = Resource::from_reader(reader, None);
        resource.reader.0 = CancelGuard(Some(track));

        drop(resource);

        assert_eq!(state.load(Ordering::SeqCst), consts::DROPPED_AFTER_CANCEL);
    }

    #[kithara::test(native, flash(false))]
    fn reader_unwrap_disarms_resource_cancel() {
        let track = CancelToken::never();
        let state = Arc::new(AtomicU8::new(consts::NOT_DROPPED));
        let reader = EofReader::with_drop_probe(track.clone(), Arc::clone(&state));
        let mut resource = Resource::from_reader(reader, None);
        resource.reader.0 = CancelGuard(Some(track.clone()));

        let reader: Box<dyn AudioReader> = resource.into();

        assert!(!track.is_cancelled());
        assert_eq!(state.load(Ordering::SeqCst), consts::NOT_DROPPED);

        drop(reader);

        assert!(!track.is_cancelled());
        assert_eq!(state.load(Ordering::SeqCst), consts::DROPPED_BEFORE_CANCEL);
    }

    #[kithara::test(native, flash(false))]
    fn seek_withdraws_the_resident_warp_context(half: Vec<f32>) {
        let mut warp = Warp::new((), &WarpConfig::builder().build());
        let publisher = warp
            .take_publisher()
            .expect("fixture Warp owns its publisher");
        let reader = publisher.reader();
        let mut resource = Resource::from_reader(EofReader::with_frames(half[..2].to_vec()), None);
        let output = OutputContext::new(
            SessionFrame::new(0)..SessionFrame::new(1),
            NonZeroU32::new(consts::SAMPLE_RATE).expect("static sample rate"),
            SessionEpoch::new(1),
            None,
        )
        .expect("fixture output range is ordered");
        let context = RenderContext::new(output, None).expect("fixture context is valid");
        publisher.publish(
            &context,
            PresentationFrontier::builder()
                .source(1)
                .output(SessionFrame::new(0))
                .build(),
        );
        assert!(reader.load().is_some());
        resource.render_publisher = Some(publisher);
        let mut resource = PlayerResource::new(resource, Arc::from("seek"), &pools())
            .unwrap_or_else(|error| panic!("test player resource: {error}"));

        resource.reset_for_seek();

        assert!(reader.load().is_none());
    }
}
