use std::{
    fmt::{self, Debug, Formatter},
    marker::PhantomData,
    num::NonZeroU32,
    pin::pin,
};

use delegate::delegate;
use futures::future::{Either, select};
use kithara_audio::{
    AudioObserver, AudioReadError, AudioReader, ChunkOutcome, ReadOutcome, ResamplerBackend,
    SeekOutcome,
};
use kithara_bufpool::{HasPool, PoolError, PoolRegion};
use kithara_command::{Inbox, Sender};
use kithara_decode::{DecodeError, TrackMetadata};
use kithara_events::{EventBus, EventReceiver, EventSet};
use kithara_platform::{
    maybe_send::{BoxFuture, MaybeSendFuture},
    sync::Arc,
    time::Duration,
};
use kithara_render::{
    LaneProtocol, LaneStart, LoadRefusal, Open, PcmReceiver,
    rt::{
        DeckMixerConfig,
        track::{PcmConsumer, PlayerResource},
    },
};
use kithara_signal::{AudioSpec, FrameCount};
use num_traits::ToPrimitive;
use tracing::warn;

use super::{PlaybackResamplerBackend, ResourceConfig, ResourceLane, SourceType};
use crate::PlayError;

/// A directly owned decoded reader used outside the deck's packet-ring path.
#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct Resource {
    #[field(get, deref = false)]
    src: Arc<str>,
    #[field(get = event_bus)]
    bus: EventBus,
    reader: Box<dyn AudioReader>,
}

impl Resource {
    /// Wraps a directly owned reader and prepares its first input off-RT.
    #[must_use]
    pub fn from_reader<R: AudioReader + 'static>(mut reader: R, src: Option<Arc<str>>) -> Self {
        let bus = reader.event_bus().clone();
        let src = src.unwrap_or_else(|| Arc::from("unknown"));
        if let Err(error) = reader.preload() {
            warn!(%src, %error, "resource preload failed");
        }
        Self {
            src,
            bus,
            reader: Box::new(reader),
        }
    }

    /// Prepares input on the reader's owning thread without a producer gate.
    ///
    /// # Errors
    /// Returns the reader's source or decoder failure.
    pub async fn preload(&mut self) -> Result<(), AudioReadError> {
        self.reader.preload()
    }

    /// Subscribe to unified source and decoder events.
    #[must_use]
    pub fn subscribe<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }

    delegate! {
        to self.reader {
            /// Adaptive bitrate control, when the source has one.
            #[must_use]
            pub fn abr_handle(&self) -> Option<kithara_abr::AbrHandle>;
            /// Source span already cached on disk.
            #[must_use]
            pub fn cached_span(&self) -> Duration;
            /// Source position through which input has been decoded.
            #[must_use]
            pub fn decoded_frontier(&self) -> Duration;
            /// Total source duration, when known.
            #[must_use]
            pub fn duration(&self) -> Option<Duration>;
            /// Tags captured from the source.
            #[must_use]
            pub fn metadata(&self) -> &TrackMetadata;
            /// Current committed source position.
            #[must_use]
            pub fn position(&self) -> Duration;
            /// Current decoded-audio format.
            #[must_use]
            pub fn spec(&self) -> AudioSpec;
            /// Read one decoded chunk with its metadata.
            pub fn next_chunk(&mut self) -> Result<ChunkOutcome, AudioReadError>;
            /// Read interleaved decoded samples.
            pub fn read(&mut self, buf: &mut [f32]) -> Result<ReadOutcome, AudioReadError>;
            /// Read deinterleaved decoded samples.
            pub fn read_planar<'a>(
                &mut self,
                output: &'a mut [&'a mut [f32]],
            ) -> Result<ReadOutcome, AudioReadError>;
            /// Seek synchronously on the source's owning thread.
            pub fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError>;
            /// Rebuild decoder resampling on the source's owning thread.
            pub fn set_host_sample_rate(&mut self, sample_rate: NonZeroU32);
        }
    }
}

impl Debug for Resource {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Resource")
            .field("src", &self.src)
            .finish_non_exhaustive()
    }
}

type ResourceOpen = Result<(OpenedTrack, ResourceLane, FrameCount), LoadRefusal>;
type LaneChannel = (Sender<LaneProtocol>, Inbox<LaneProtocol>);

/// A single-use source open and its preparation-owned lane wiring.
pub struct ResourceLoad<S, B = PlaybackResamplerBackend> {
    opener: Box<
        dyn FnOnce(Duration, LaneStart, Inbox<LaneProtocol>) -> BoxFuture<'static, ResourceOpen>
            + Send,
    >,
    channel: Option<Box<dyn Fn() -> LaneChannel + Send>>,
    geometry: Result<(Option<FrameCount>, FrameCount), PlayError>,
    marker: PhantomData<fn() -> (S, B)>,
}

impl<S, B> Debug for ResourceLoad<S, B> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceLoad")
            .finish_non_exhaustive()
    }
}

impl<S, B> ResourceLoad<S, B> {
    /// Allocates the owner's lane sender before transferring the inbox in Load.
    pub(crate) fn lane_channel(
        &self,
    ) -> Result<(Sender<LaneProtocol>, Inbox<LaneProtocol>), PlayError> {
        let channel = self.channel.as_ref().ok_or_else(|| {
            PlayError::Internal("ResourceConfig requires an explicit PlayWorker".into())
        })?;
        Ok(channel())
    }

    /// Maximum rendered lead, including the held packet, and the lane's Jump ramp.
    pub(crate) fn lane_geometry(&self) -> Result<(FrameCount, FrameCount), PlayError> {
        let (ring_depth, declick) = self.geometry.clone()?;
        let ring_depth = match ring_depth {
            Some(ring_depth) => ring_depth,
            None => todo!(
                "kithara-warp::WarpRenderer uncapped output-packet frame bound (contract §2 lane lead)"
            ),
        };
        Ok((ring_depth, declick))
    }
}

impl<S, B> ResourceLoad<S, B>
where
    B: Default + ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    /// Captures the configured source and decoder observer for one open.
    #[must_use]
    pub fn new(config: ResourceConfig<S, B>, observer: Box<dyn AudioObserver>) -> Self {
        let geometry = Self::geometry(&config);
        let channel = config.worker.clone().map(|worker| {
            Box::new(move || worker.lane_channel()) as Box<dyn Fn() -> LaneChannel + Send>
        });
        let cancel = config.cancel.clone();
        Self {
            opener: Box::new(move |position, start, inbox| {
                Box::pin(async move {
                    let open = Self::load(config, observer, position, start, inbox);
                    match cancel {
                        None => open.await,
                        Some(cancel) => match select(pin!(cancel.cancelled()), pin!(open)).await {
                            Either::Left(((), _open)) => Err(LoadRefusal::Cancelled),
                            Either::Right((opened, _cancel)) => opened,
                        },
                    }
                })
            }),
            channel,
            geometry,
            marker: PhantomData,
        }
    }

    fn geometry(
        config: &ResourceConfig<S, B>,
    ) -> Result<(Option<FrameCount>, FrameCount), PlayError> {
        let worker = config.worker.as_ref().ok_or_else(|| {
            PlayError::Internal("ResourceConfig requires an explicit PlayWorker".into())
        })?;
        let audio = config.clone().build_file_config(worker, None);
        let track = config.build_track_config(audio);
        let ring_depth = track
            .warp()
            .render_quantum_frames()
            .map(|quantum| {
                track
                    .audio_buffer_chunks()
                    .get()
                    .checked_add(1)
                    .and_then(|packets| packets.checked_mul(quantum.get()))
                    .and_then(|frames| frames.checked_sub(1))
                    .map(FrameCount::new)
                    .ok_or_else(|| PlayError::Internal("lane ring frame depth overflow".into()))
            })
            .transpose()?;
        let rate = config.host_sample_rate.ok_or_else(|| {
            PlayError::Internal("lane geometry requires the prepared host sample rate".into())
        })?;
        let declick = (f64::from(rate.get())
            * f64::from(DeckMixerConfig::default().declick().smooth_seconds))
        .to_usize()
        .ok_or_else(|| PlayError::Internal("lane Jump ramp frame count overflow".into()))?
        .max(1);
        Ok((ring_depth, FrameCount::new(declick)))
    }

    async fn load(
        config: ResourceConfig<S, B>,
        observer: Box<dyn AudioObserver>,
        position: Duration,
        start: LaneStart,
        inbox: Inbox<LaneProtocol>,
    ) -> ResourceOpen {
        let src: Arc<str> = Arc::from(config.src.to_string());
        let source_type = SourceType::detect(&config.src)?;
        let worker = config.worker.clone().ok_or(DecodeError::InvalidData {
            detail: "ResourceConfig requires an explicit PlayWorker",
        })?;
        let (receiver, lane, latency) = match source_type {
            SourceType::RemoteFile(_) | SourceType::LocalFile(_) => {
                let audio = config.clone().build_file_config(&worker, Some(observer));
                let track = config.build_track_config(audio);
                let (receiver, lane, latency) = worker.load(track, position, start, inbox).await?;
                (receiver, ResourceLane::new(lane), latency)
            }
            SourceType::HlsStream(_) => {
                let audio = config.clone().build_hls_config(&worker, Some(observer))?;
                let track = config.build_track_config(audio);
                let (receiver, lane, latency) = worker.load(track, position, start, inbox).await?;
                (receiver, ResourceLane::new(lane), latency)
            }
        };
        Ok((
            OpenedTrack::new(receiver, src, worker.pools())?,
            lane,
            latency,
        ))
    }
}

/// The owner-facing receiver and source facts returned by a dispatcher load.
pub struct OpenedTrack {
    pub pcm: Box<PlayerResource>,
    pub duration: Option<Duration>,
    pub abr: Option<kithara_abr::AbrHandle>,
    pub metadata: TrackMetadata,
}

impl Debug for OpenedTrack {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedTrack")
            .field("src", self.pcm.src())
            .field("duration", &self.duration)
            .finish_non_exhaustive()
    }
}

impl OpenedTrack {
    fn new<S>(
        receiver: PcmReceiver,
        src: Arc<str>,
        pools: &PoolRegion<S>,
    ) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        let duration = receiver.duration();
        let abr = receiver.abr_handle();
        let metadata = receiver.metadata().clone();
        let pcm = Box::new(PlayerResource::new(PcmConsumer::new(receiver), src, pools)?);
        Ok(Self {
            pcm,
            duration,
            abr,
            metadata,
        })
    }
}

impl<S, B> Open for ResourceLoad<S, B> {
    type Opened = OpenedTrack;
    type Lane = ResourceLane;

    /// Opens once with cancellation and concrete pool ownership captured at construction.
    fn open(
        self,
        position: Duration,
        start: LaneStart,
        inbox: Inbox<LaneProtocol>,
    ) -> impl MaybeSendFuture<Output = Result<(OpenedTrack, ResourceLane, FrameCount), LoadRefusal>>
    {
        (self.opener)(position, start, inbox)
    }
}

/// Transfer the directly owned reader to another off-RT consumer.
impl From<Resource> for Box<dyn AudioReader> {
    fn from(resource: Resource) -> Self {
        resource.reader
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
    use kithara_assets::{AssetStore, StorageBackend};
    use kithara_audio::{
        AudioControl, AudioObserverSlot, AudioRead, AudioSession, ReadOutcome, SeekOutcome,
    };
    use kithara_bufpool::PoolRegion;
    use kithara_decode::TrackMetadata;
    use kithara_events::TrackId;
    use kithara_platform::{CancelToken, sync::Arc};
    use kithara_render::{
        bridge::{DeckPart, PlayerNotification, TrackTransition, slot_channels},
        rt::{DeckMixer, DeckMixerConfig, StreamShape, track::PlayerResource},
    };
    use kithara_signal::{AudioSpec, OutputContext, SessionEpoch, SessionFrame};
    use kithara_test_fixtures::play_fixtures::half;
    use kithara_test_utils::kithara;
    use kithara_warp::{
        PresentationFrontier, RenderContext, SpeedCurve, StretchKind, Warp, WarpConfig,
        supports_playback_rate,
    };
    use ringbuf::traits::Consumer;

    use super::*;
    use crate::{
        PlayWorker, PlayWorkerConfig, ResourceSrc, consts,
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
        fn read(&mut self, buf: &mut [f32]) -> Result<ReadOutcome, AudioReadError> {
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
        ) -> Result<ReadOutcome, AudioReadError> {
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
        fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError> {
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
        let consumer =
            PcmConsumer::from(Resource::from_reader(EofReader::with_frames(samples), None))
                .with_playback_rate(PlaybackRate::for_warp(speed));
        PlayerResource::new(consumer, Arc::from(src), pools)
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

    fn rate_notifications(control: &mut kithara_render::bridge::SlotControl) -> Vec<f32> {
        let mut rates = Vec::new();
        while let Some(notification) = control.notif_rx.try_pop() {
            if let PlayerNotification::RateChanged { rate } = notification {
                rates.push(rate);
            }
        }
        rates
    }

    #[kithara::test(native, flash(false))]
    fn a_loaded_track_takes_the_processor_rate(half: Vec<f32>) {
        let pools = pools();
        let effective_rate = if supports_playback_rate() { 1.5 } else { 1.0 };
        let (inputs, mut control) = slot_channels();
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
        assert_eq!(processor.playback().snapshot().rate(), effective_rate);
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

        assert_eq!(processor.playback().snapshot().rate(), effective_rate);
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
        resource.consumer.cancel_on_drop(Some(track.clone()));

        assert!(!stream_sub.is_cancelled() && !audio_sub.is_cancelled());
        drop(resource);
        assert!(
            stream_sub.is_cancelled(),
            "unload must cancel the stream-side subtree, not only the Audio half"
        );
        assert!(audio_sub.is_cancelled());
        assert!(track.is_cancelled());
    }

    /// A load whose track is cancelled ends at once, refused `Cancelled`,
    /// without opening its source: whoever sent it does not wait on an open
    /// nobody wants.
    #[kithara::test(tokio)]
    async fn a_cancelled_load_is_refused_without_opening_its_source() {
        let pools = pools();
        let mut config: ResourceConfig<TestPools> =
            ResourceConfig::for_src(ResourceSrc::Path("/kithara/missing.mp3".into()))
                .store(
                    AssetStore::builder(pools.clone())
                        .backend(StorageBackend::Memory)
                        .build(),
                )
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools).build()))
                .build();
        let track = CancelToken::never().child();
        track.cancel();
        config.cancel = Some(track);

        let load = ResourceLoad::new(config, Box::new(AudioObserverSlot::default().relay()));
        let (_sender, inbox) = load.lane_channel().expect("configured worker lane channel");
        let refused = load
            .open(
                Duration::ZERO,
                LaneStart {
                    speed: SpeedCurve::Constant(1.0),
                    keylock: false,
                    backend: StretchKind::default(),
                },
                inbox,
            )
            .await
            .map(|(opened, _, _)| opened);

        assert!(
            matches!(refused, Err(LoadRefusal::Cancelled)),
            "a cancelled load opened its source: {refused:?}"
        );
    }

    /// An open in flight ends the moment its track is cancelled, refused
    /// `Cancelled`, and gives its worker slot back: the next load opens
    /// instead of finding the worker full.
    #[kithara::test(native, tokio)]
    async fn an_open_in_flight_ends_cancelled_and_frees_its_slot() {
        use axum::Router;
        use futures::future;
        use kithara_platform::{time, tokio::sync::mpsc::unbounded_channel};
        use kithara_test_utils::TestHttpServer;

        let (reached_tx, mut reached) = unbounded_channel();
        let server = TestHttpServer::new(Router::new().fallback(move || {
            let reached = reached_tx.clone();
            async move {
                let _ = reached.send(());
                future::pending::<()>().await;
            }
        }))
        .await;
        let pools = pools();
        let worker = PlayWorker::new(
            PlayWorkerConfig::builder(pools.clone())
                .capacity(NonZeroUsize::MIN)
                .build(),
        );
        let load = |src: ResourceSrc, track: CancelToken| {
            let mut config: ResourceConfig<TestPools> = ResourceConfig::for_src(src)
                .store(
                    AssetStore::builder(pools.clone())
                        .backend(StorageBackend::Memory)
                        .build(),
                )
                .worker(worker.clone())
                .build();
            config.cancel = Some(track);
            let load = ResourceLoad::new(config, Box::new(AudioObserverSlot::default().relay()));
            let (sender, inbox) = load.lane_channel().expect("configured worker lane channel");
            async move {
                let opened = load.open(
                    Duration::ZERO,
                    LaneStart {
                        speed: SpeedCurve::Constant(1.0),
                        keylock: false,
                        backend: StretchKind::default(),
                    },
                    inbox,
                ).await.map(|(opened, _, _)| opened);
                drop(sender);
                opened
            }
        };
        let track = CancelToken::never().child();
        let stalled = load(
            ResourceSrc::parse(server.url("/stalled.mp3").as_str()).expect("a test URL"),
            track.clone(),
        );
        let cancel_once_reached = async {
            reached.recv().await.expect("the server holds its sender");
            track.cancel();
        };

        let (refused, ()) = time::timeout(
            Duration::from_secs(2),
            future::join(stalled, cancel_once_reached),
        )
        .await
        .expect("a cancelled open ends");
        assert!(
            matches!(refused, Err(LoadRefusal::Cancelled)),
            "a cancelled open answered otherwise: {refused:?}"
        );

        let next = load(
            ResourceSrc::Path("/kithara/missing.mp3".into()),
            CancelToken::never().child(),
        )
        .await;
        assert!(
            matches!(next, Err(LoadRefusal::Open(_))),
            "the cancelled open kept its worker slot: {next:?}"
        );
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
        resource.consumer.cancel_on_drop(Some(track));

        drop(resource);

        assert_eq!(state.load(Ordering::SeqCst), consts::DROPPED_AFTER_CANCEL);
    }

    #[kithara::test(native, flash(false))]
    fn reader_unwrap_disarms_resource_cancel() {
        let track = CancelToken::never();
        let state = Arc::new(AtomicU8::new(consts::NOT_DROPPED));
        let reader = EofReader::with_drop_probe(track.clone(), Arc::clone(&state));
        let mut resource = Resource::from_reader(reader, None);
        resource.consumer.cancel_on_drop(Some(track.clone()));

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
        let resource = Resource::from_reader(EofReader::with_frames(half[..2].to_vec()), None);
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
        let consumer = PcmConsumer::from(resource).with_render_publisher(publisher);
        let mut resource = PlayerResource::new(consumer, Arc::from("seek"), &pools())
            .unwrap_or_else(|error| panic!("test player resource: {error}"));

        resource.reset_for_seek();

        assert!(reader.load().is_none());
    }
}
