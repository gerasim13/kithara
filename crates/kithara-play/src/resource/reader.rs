use std::num::NonZeroU32;

use delegate::delegate;
use kithara_audio::{
    AudioObserver, AudioReader, ChunkOutcome, ReadOutcome, ResamplerBackend, SeekOutcome,
};
use kithara_decode::{DecodeError, DecodeResult, TrackMetadata};
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc, time::Duration};
use kithara_signal::AudioSpec;
use kithara_stream::{Stream, StreamType};
use kithara_warp::{StretchControls, WarpConfig};
use tracing::warn;

use super::{ResourceConfig, SourceType};
use crate::{
    PlayWorker, TrackConfig,
    effects::supports_playback_rate,
    worker::{ServiceClass, TrackPriority},
};

/// Type-erased audio resource wrapping any `AudioReader`.
///
/// Provides a unified interface for reading decoded audio
/// regardless of the underlying source (file, HLS, custom).
///
/// # Example
///
/// ```ignore
/// use kithara_assets::AssetStore;
/// use kithara_bufpool::Region;
/// use kithara_play::{PlayWorker, PlayWorkerConfig, Resource, ResourceConfig};
///
/// let region = Region::default();
/// let worker = PlayWorker::new(
///     PlayWorkerConfig::for_pools(region.byte_pool(), region.sample_pool()).build(),
/// );
///
/// // Auto-detect: .m3u8 -> HLS, everything else -> progressive file
/// let config: ResourceConfig = ResourceConfig::for_src(ResourceConfig::parse_src(
///     "https://example.com/song.mp3",
/// )?)
/// .store(AssetStore::builder().pool(region.byte_pool()).build())
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
    pub(crate) inner: Box<dyn AudioReader>,
    priority: Option<TrackPriority>,
    #[field(with)]
    playback_rate: PlaybackRate,
    #[field(get, deref = false)]
    src: Arc<str>,
    /// Drop guard for the per-track cancel — the token passed as
    /// `ResourceConfig.cancel`, whose subtree covers BOTH the inner stream
    /// (File/Hls) and the `Audio` pipeline (each a `child()` of it). Declared
    /// first so it drops before `inner`: a mid-session unload tears down the
    /// whole track subtree — not just the `Audio` half `Audio::Drop` would
    /// reach under propagate-down — while the stream's fetch loops still
    /// observe the cancel as `Audio` drops. `None` for custom-reader resources;
    /// disarmed by the `From<Resource>` reader unwrap when the live reader
    /// passes to the analysis worker.
    cancel: CancelGuard,
    #[field(get = event_bus)]
    bus: EventBus,
}

/// Cancels the wrapped per-track token on drop. A `Resource` field rather than
/// a `Resource: Drop` impl so the `From<Resource>` reader unwrap can move
/// `inner` out of the wrapper after [`disarm`](CancelGuard::disarm)ing. Passive
/// when `None`.
struct CancelGuard(Option<CancelToken>);

enum PlaybackRate {
    Fixed,
    Warp(Arc<StretchControls>),
}

impl PlaybackRate {
    fn for_warp(controls: Arc<StretchControls>) -> Self {
        if supports_playback_rate() {
            Self::Warp(controls)
        } else {
            Self::Fixed
        }
    }

    fn apply(&self, requested: f32) -> f32 {
        if let Self::Warp(controls) = self {
            controls.set_speed(requested);
        }
        self.into()
    }
}

impl From<&PlaybackRate> for f32 {
    fn from(rate: &PlaybackRate) -> Self {
        match rate {
            PlaybackRate::Fixed => 1.0,
            PlaybackRate::Warp(controls) => controls.speed(),
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
    pub async fn new<B>(config: ResourceConfig<B>) -> DecodeResult<Self>
    where
        B: Default + ResamplerBackend,
    {
        Self::open(config, None).await
    }

    /// Create a resource with a bounded observer of decoded audio attached.
    ///
    /// This is a narrow cross-crate composition seam used by queue-owned
    /// orchestration. The ordinary resource API remains [`Self::new`].
    #[doc(hidden)]
    pub async fn new_observed<B>(
        config: ResourceConfig<B>,
        observer: Box<dyn AudioObserver>,
    ) -> DecodeResult<Self>
    where
        B: Default + ResamplerBackend,
    {
        Self::open(config, Some(observer)).await
    }

    async fn open<B>(
        config: ResourceConfig<B>,
        observer: Option<Box<dyn AudioObserver>>,
    ) -> DecodeResult<Self>
    where
        B: Default + ResamplerBackend,
    {
        let src: Arc<str> = Arc::from(config.src.to_string());
        let source_type = SourceType::detect(&config.src)?;
        let worker = config.worker.clone().ok_or(DecodeError::InvalidData {
            detail: "ResourceConfig requires an explicit PlayWorker",
        })?;
        let stretch = Arc::clone(&config.stretch);
        let engine_load = config.engine_load.clone();
        // Capture the per-track cancel before `build_*_config` consumes `config`
        // (it is cloned by identity into both the inner stream and the Audio).
        let cancel = config.cancel.clone();
        let mut resource = match source_type {
            SourceType::RemoteFile(_) | SourceType::LocalFile(_) => {
                let audio_config = config.build_file_config(&worker, observer);
                let track = TrackConfig::for_audio(audio_config)
                    .maybe_engine_load(engine_load)
                    .warp(WarpConfig::builder().stretch(Arc::clone(&stretch)).build())
                    .build();
                Self::from_stream_audio(track, src, &worker).await?
            }
            SourceType::HlsStream(_) => {
                let audio_config = config.build_hls_config(&worker, observer)?;
                let track = TrackConfig::for_audio(audio_config)
                    .maybe_engine_load(engine_load)
                    .warp(WarpConfig::builder().stretch(Arc::clone(&stretch)).build())
                    .build();
                Self::from_stream_audio(track, src, &worker).await?
            }
        };
        resource.cancel = CancelGuard(cancel);
        Ok(resource)
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
        let bus = reader.event_bus().clone();
        let mut inner: Box<dyn AudioReader> = Box::new(reader);
        let src = src.unwrap_or_else(|| Arc::from("unknown"));
        if let Err(e) = inner.preload() {
            warn!(src = %src, error = %e, "resource preload failed");
        }
        Self {
            inner,
            priority: None,
            playback_rate: PlaybackRate::Fixed,
            bus,
            src,
            cancel: CancelGuard(None),
        }
    }

    /// Create a resource from a concrete stream-backed audio config.
    ///
    /// Generic over any [`StreamType`] whose config carries an optional
    /// `kithara_events::EventBus`. Callers wanting fine-grained control
    /// over `FileConfig` / `HlsConfig` (ABR, keys, etc.) use this path.
    pub(crate) async fn from_stream_audio<T, B>(
        config: TrackConfig<T, B>,
        src: Arc<str>,
        worker: &PlayWorker,
    ) -> DecodeResult<Self>
    where
        T: StreamType<Events = EventBus> + 'static,
        B: Default + ResamplerBackend,
        crate::RegisteredAudio<Stream<T>>: AudioReader + 'static,
    {
        let warp_controls = Arc::clone(config.warp().stretch());
        let audio = worker.open(config).await?;
        let priority = audio.priority();
        let mut resource = Self::from_reader(audio, Some(src))
            .with_playback_rate(PlaybackRate::for_warp(warp_controls));
        resource.priority = Some(priority);
        Ok(resource)
    }

    pub(crate) fn apply_playback_rate(&self, rate: f32) -> f32 {
        self.playback_rate.apply(rate)
    }

    pub(crate) fn playback_rate(&self) -> f32 {
        (&self.playback_rate).into()
    }

    pub(crate) fn set_service_class(&self, class: ServiceClass) {
        if let Some(priority) = &self.priority {
            priority.set(class);
        }
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
        if let Some(gate) = self.inner.preload_gate() {
            gate.wait_for_epoch(self.inner.preload_epoch()).await;
        }
        self.inner.preload()
    }

    /// Subscribe to unified events.
    ///
    /// Returns a receiver for all events published to the bus,
    /// including audio, file, and HLS events.
    #[must_use]
    pub fn subscribe(&self) -> kithara_events::EventReceiver {
        self.bus.subscribe()
    }

    delegate! {
        to self.inner {
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
/// source to the shared analysis worker (`kithara_audio::analysis`).
///
/// Disarms the per-track cancel before moving the reader out: the live reader
/// outlives this wrapper, so freeing the wrapper must not tear down its fetch
/// loops. Teardown then rides the analysis run-scope cancel.
impl From<Resource> for Box<dyn AudioReader> {
    fn from(resource: Resource) -> Self {
        let Resource {
            inner, mut cancel, ..
        } = resource;
        cancel.disarm();
        inner
    }
}

#[cfg(test)]
mod tests {
    use std::{
        num::{NonZeroU32, NonZeroUsize},
        sync::atomic::Ordering,
    };

    use firewheel::{
        clock::InstantSamples,
        dsp::{buffer::ChannelBuffer, declick::DeclickValues},
        event::{NodeEvent, ProcEvents, ProcEventsIndex, ScheduledEventEntry},
        log::{RealtimeLoggerConfig, realtime_logger},
        mask::{ConnectedMask, ConstantMask, SilenceMask},
        node::{
            AudioNodeProcessor, NUM_SCRATCH_BUFFERS, ProcBuffers, ProcExtra, ProcInfo, ProcStore,
            StreamStatus,
        },
    };
    use kithara_audio::{AudioControl, AudioRead, AudioSession, ReadOutcome, SeekOutcome};
    use kithara_bufpool::SamplePool;
    use kithara_decode::TrackMetadata;
    use kithara_events::TrackId;
    use kithara_platform::{CancelToken, sync::Arc};
    use kithara_signal::AudioSpec;
    use kithara_test_utils::kithara;
    use ringbuf::traits::{Consumer, Producer};

    use super::*;
    use crate::{
        bridge::{PlayerCmd, PlayerNotification, SharedEq, TrackTransition, slot_channels},
        rt::{PlayerNodeProcessor, StreamShape, track::PlayerResource},
    };

    struct Consts;

    impl Consts {
        const BLOCK_FRAMES: usize = 512;
        const SAMPLE_RATE: u32 = 44_100;
    }

    struct EofReader {
        bus: EventBus,
        spec: AudioSpec,
        meta: TrackMetadata,
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
                    NonZeroU32::new(Consts::SAMPLE_RATE).expect("static rate"),
                ),
                position_frames: 0,
                total_frames: 0,
            }
        }
    }

    impl EofReader {
        fn with_frames(total_frames: usize) -> Self {
            Self {
                total_frames,
                ..Self::default()
            }
        }

        fn position_duration(&self) -> Duration {
            let frames = u32::try_from(self.position_frames).expect("test frame count fits u32");
            Duration::from_secs_f64(f64::from(frames) / f64::from(Consts::SAMPLE_RATE))
        }

        fn eof(&self) -> ReadOutcome {
            ReadOutcome::Eof {
                position: self.position_duration(),
            }
        }

        fn take_frames(&mut self, capacity: usize) -> Option<NonZeroUsize> {
            let frames = capacity.min(self.total_frames - self.position_frames);
            self.position_frames += frames;
            NonZeroUsize::new(frames)
        }
    }

    impl AudioSession for EofReader {
        fn duration(&self) -> Option<Duration> {
            let frames = u32::try_from(self.total_frames).expect("test frame count fits u32");
            Some(Duration::from_secs_f64(
                f64::from(frames) / f64::from(Consts::SAMPLE_RATE),
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
            buf[..samples].fill(0.5);
            Ok(ReadOutcome::Frames {
                count: NonZeroUsize::new(samples).expect("non-zero stereo sample count"),
                position: self.position_duration(),
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
            for channel in output {
                channel[..frames.get()].fill(0.5);
            }
            Ok(ReadOutcome::Frames {
                count: frames,
                position: self.position_duration(),
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

    fn warped_player_resource(controls: &Arc<StretchControls>, src: &str) -> Box<PlayerResource> {
        let total_frames = usize::try_from(Consts::SAMPLE_RATE).expect("sample rate fits usize");
        let resource = Resource::from_reader(EofReader::with_frames(total_frames), None)
            .with_playback_rate(PlaybackRate::for_warp(Arc::clone(controls)));
        Box::new(PlayerResource::new(
            resource,
            Arc::from(src),
            &SamplePool::default(),
        ))
    }

    fn process_block(processor: &mut PlayerNodeProcessor, extra: &mut ProcExtra) {
        let info = ProcInfo {
            sample_rate: NonZeroU32::new(Consts::SAMPLE_RATE).expect("static sample rate"),
            frames: Consts::BLOCK_FRAMES,
            in_silence_mask: SilenceMask::default(),
            out_silence_mask: SilenceMask::default(),
            in_constant_mask: ConstantMask::default(),
            out_constant_mask: ConstantMask::default(),
            in_connected_mask: ConnectedMask::default(),
            out_connected_mask: ConnectedMask::default(),
            prev_output_was_silent: false,
            sample_rate_recip: f64::from(Consts::SAMPLE_RATE).recip(),
            clock_samples: InstantSamples(0),
            duration_since_stream_start: Duration::ZERO,
            stream_status: StreamStatus::empty(),
            dropped_frames: 0,
        };
        let inputs: [&[f32]; 0] = [];
        let mut left = [0.0; Consts::BLOCK_FRAMES];
        let mut right = [0.0; Consts::BLOCK_FRAMES];
        let mut outputs = [&mut left[..], &mut right[..]];
        let buffers = ProcBuffers {
            inputs: &inputs,
            outputs: &mut outputs,
        };
        let mut immediate: [Option<NodeEvent>; 0] = [];
        let mut scheduled: [Option<ScheduledEventEntry>; 0] = [];
        let mut indices: Vec<ProcEventsIndex> = Vec::new();
        let mut events = ProcEvents::new(&mut immediate, &mut scheduled, &mut indices);
        let _ = processor.process(&info, buffers, &mut events, extra);
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
        let fixed = Resource::from_reader(EofReader::default(), None);
        assert_eq!(fixed.apply_playback_rate(1.5), 1.0);

        let controls = StretchControls::new(1.0);
        let warped = Resource::from_reader(EofReader::default(), None)
            .with_playback_rate(PlaybackRate::for_warp(Arc::clone(&controls)));
        if supports_playback_rate() {
            assert_eq!(warped.apply_playback_rate(1.5), 1.5);
            assert!((controls.speed() - 1.5).abs() < f32::EPSILON);
            controls.set_speed(1.25);
            assert_eq!(warped.playback_rate(), 1.25);
        } else {
            assert_eq!(warped.apply_playback_rate(1.5), 1.0);
            assert!((controls.speed() - 1.0).abs() < f32::EPSILON);
            controls.set_speed(1.25);
            assert_eq!(warped.playback_rate(), 1.0);
        }
    }

    #[kithara::test(native, flash(false))]
    fn loading_next_warp_resource_preserves_shared_target_and_effective_capability() {
        let controls = StretchControls::new(1.0);
        let effective_rate = if supports_playback_rate() { 1.5 } else { 1.0 };
        let (inputs, mut control) = slot_channels(SharedEq::new(0));
        let shape = StreamShape {
            sample_rate: NonZeroU32::new(Consts::SAMPLE_RATE).expect("static sample rate"),
            max_block_frames: NonZeroU32::new(
                u32::try_from(Consts::BLOCK_FRAMES).expect("block size fits u32"),
            )
            .expect("static block size"),
        };
        let mut processor = PlayerNodeProcessor::new(inputs, shape, &SamplePool::default());
        let (logger, _logger_rx) = realtime_logger(RealtimeLoggerConfig::default());
        let mut extra = ProcExtra {
            logger,
            store: ProcStore::with_capacity(0),
            scratch_buffers: ChannelBuffer::<f32, NUM_SCRATCH_BUFFERS>::new(Consts::BLOCK_FRAMES),
            declick_values: DeclickValues::new(NonZeroU32::new(16).expect("static declick length")),
        };
        let first: Arc<str> = Arc::from("first");
        let first_id = TrackId::allocate();
        control
            .cmd_tx
            .try_push(PlayerCmd::LoadTrack {
                resource: warped_player_resource(&controls, &first),
                item_id: first_id,
            })
            .expect("load first track");
        control
            .cmd_tx
            .try_push(PlayerCmd::Transition(TrackTransition::FadeIn(first_id)))
            .expect("fade in first track");
        control
            .cmd_tx
            .try_push(PlayerCmd::SetPaused(false))
            .expect("start playback");
        process_block(&mut processor, &mut extra);
        let _ = rate_notifications(&mut control);

        controls.set_speed(1.5);
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
        let block_frames = u32::try_from(Consts::BLOCK_FRAMES).expect("block size fits u32");
        let expected_advance =
            f64::from(block_frames) * f64::from(effective_rate) / f64::from(Consts::SAMPLE_RATE);
        assert!((first_advance - expected_advance).abs() < f64::EPSILON);
        assert_eq!(
            processor.playback().rate.load(Ordering::Relaxed),
            effective_rate
        );
        let notifications = rate_notifications(&mut control);
        if supports_playback_rate() {
            assert_eq!(notifications, [1.5]);
        } else {
            assert!(notifications.is_empty());
        }

        let next: Arc<str> = Arc::from("next");
        let next_id = TrackId::allocate();
        control
            .cmd_tx
            .try_push(PlayerCmd::LoadTrack {
                resource: warped_player_resource(&controls, &next),
                item_id: next_id,
            })
            .expect("load next track");
        control
            .cmd_tx
            .try_push(PlayerCmd::Transition(TrackTransition::FadeIn(next_id)))
            .expect("fade in next track");
        assert_eq!(controls.speed(), 1.5);

        process_block(&mut processor, &mut extra);

        assert_eq!(controls.speed(), 1.5);
        assert_eq!(
            processor.playback().rate.load(Ordering::Relaxed),
            effective_rate
        );
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
        resource.cancel = CancelGuard(Some(track.clone()));

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
}
