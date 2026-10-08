use std::{
    io::Cursor,
    num::NonZeroU32,
    ops::Range,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use kithara_abr::{AbrMode, AbrReason, AbrState, VariantIndex};
use kithara_bufpool::PoolConfig;
use kithara_decode::{
    DecodeError, DecodeResult, Decoder, DecoderChunkOutcome, DecoderSeekOutcome, GaplessInfo,
    GaplessMode, GaplessProfile, SilenceTrimParams,
};
use kithara_events::{DeferredBus, EventBus};
use kithara_platform::{
    sync::{Arc, Condvar, Mutex, Notify},
    time::Duration,
};
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use kithara_storage::WaitOutcome;
use kithara_stream::{
    Activity, ActivityWriter, AudioCodec, ByteMap, ContainerFormat, DeferredWake, MediaInfo,
    OpenedReader, OpenedVariantReader, PlayheadRead, PlayheadState, PlayheadWrite, PrerollHint,
    ReadOutcome, ReaderProfile, SegmentDescriptor, Source, SourceError, SourcePhase, SourceProbe,
    SourceSeekAnchor, Stream, StreamError, StreamResult, StreamType, VariantControl,
    VariantPromotion, VariantReaderPlan, VariantReaderTake, VariantTransition, VariantTransitionId,
    mock::NoopWorkerWake,
};
use kithara_test_fixtures::unit_fixtures::{RoutePcm, route_pcm};
use kithara_test_utils::kithara;

use crate::{
    AudioEvent, AudioLaneEvent, DecoderChangeCause, DecoderEvent, TrackFailureKind, consts,
    pipeline::{
        decode::{
            DecoderGeneration,
            core::{ActiveDecode, DecoderFactory},
            transition::OutgoingFrontier,
        },
        fetch::{Fetch, SourceEnd},
        rebuild::{RecreateCause, RecreateState},
        source::StreamAudioSource,
        stream::shared::SharedStream,
        track::{TrackStep, WaitingReason},
    },
    test_pools::{Pools, pools, pools_with, sample_buffer},
    traits::AudioSource,
};

pub(super) fn produced_data(fetch: Fetch<AudioChunk>) -> AudioChunk {
    let Fetch::Data { data, .. } = fetch else {
        panic!("TrackStep::Produced must carry PCM data");
    };
    data
}

pub(super) fn spec(sample_rate: u32) -> AudioSpec {
    AudioSpec::new(
        consts::REBUILD_CHANNELS,
        NonZeroU32::new(sample_rate).expect("test sample rate is non-zero"),
    )
}

pub(super) struct TestDecoder {
    drops: Arc<Mutex<Vec<u64>>>,
    preparations: Arc<AtomicU64>,
    id: u64,
    seek_error: Option<DecodeError>,
}

impl TestDecoder {
    pub(super) fn new(id: u64, drops: Arc<Mutex<Vec<u64>>>) -> Self {
        Self {
            drops,
            id,
            preparations: Arc::new(AtomicU64::new(0)),
            seek_error: None,
        }
    }
}

impl TestDecoder {
    pub(super) fn with_seek_error(mut self, error: DecodeError) -> Self {
        self.seek_error = Some(error);
        self
    }
}

impl Drop for TestDecoder {
    fn drop(&mut self) {
        self.drops.lock().push(self.id);
    }
}

impl Decoder for TestDecoder {
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        Ok(DecoderChunkOutcome::Eof)
    }

    fn prepare_next_chunk(&mut self) {
        self.preparations.fetch_add(1, Ordering::Relaxed);
    }

    fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        if let Some(error) = self.seek_error.take() {
            return Err(error);
        }
        Ok(DecoderSeekOutcome::Landed {
            landed_at: pos,
            landed_frame: 0,
            landed_byte: None,
            preroll: PrerollHint::NotNeeded,
        })
    }

    fn spec(&self) -> AudioSpec {
        AudioSpec::new(2, NonZeroU32::MIN)
    }

    fn update_byte_len(&self, _len: u64) {}
}

#[kithara::test(tokio)]
async fn retired_generations_are_all_reclaimed_after_a_burst() {
    let RebuildFixture { mut source, .. } = test_source(0).await;
    let drops = Arc::new(Mutex::new(Vec::new()));
    let initial = DecoderGeneration::new(
        Box::new(TestDecoder::new(0, Arc::clone(&drops))),
        None,
        0,
        None,
        None,
        GaplessMode::Disabled,
    );
    drop(source.decode.replace_active(initial));
    assert!(
        drops.lock().is_empty(),
        "the active generation is still owned"
    );
    for id in 1..=5 {
        let generation = DecoderGeneration::new(
            Box::new(TestDecoder::new(id, Arc::clone(&drops))),
            None,
            0,
            None,
            None,
            GaplessMode::Disabled,
        );
        drop(source.decode.replace_active(generation));
    }
    drop(source);
    let mut dropped = drops.lock().clone();
    dropped.sort_unstable();
    assert_eq!(dropped, (0..=5).collect::<Vec<_>>());
}

#[kithara::test(tokio)]
async fn checked_seek_defers_more_than_64_pcm_chunks_without_leaking() {
    let config = PoolConfig::builder()
        .max_buffers(128)
        .max_retained_capacity(1)
        .build();
    let pools = pools_with(1024 * 1024, config, config);
    let baseline = pools.stats().allocated_bytes;
    let RebuildFixture {
        drops, mut source, ..
    } = test_source(0).await;
    let mut generation = DecoderGeneration::new(
        Box::new(TestDecoder::new(7, drops)),
        None,
        0,
        None,
        None,
        GaplessMode::Disabled,
    );
    for _ in 0..65 {
        generation.stage(AudioChunk::new(
            AudioChunkInfo::default(),
            sample_buffer(&pools, &[0.0, 0.0]),
        ));
    }
    assert!(generation.has_output());
    let old = source.decode.replace_active(generation);
    drop(old);
    source.finish_deferred();
    assert!(pools.stats().allocated_bytes > baseline);

    assert!(
        pools.stats().allocated_bytes > baseline,
        "owner retains all 65 chunks before seek"
    );
    source
        .seek(Duration::from_secs(1))
        .expect("synchronous owner seek");
    source.finish_deferred();
    assert_eq!(pools.stats().allocated_bytes, baseline);
}

struct FailingDecoder;

impl Decoder for FailingDecoder {
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        Err(DecodeError::InvalidData {
            detail: "fixture decode failure",
        })
    }

    fn seek(&mut self, position: Duration) -> DecodeResult<DecoderSeekOutcome> {
        Ok(DecoderSeekOutcome::Landed {
            landed_at: position,
            landed_frame: 0,
            landed_byte: None,
            preroll: PrerollHint::NotNeeded,
        })
    }

    fn spec(&self) -> AudioSpec {
        AudioSpec::new(2, NonZeroU32::MIN)
    }

    fn update_byte_len(&self, _len: u64) {}
}

struct ProfileCountingDecoder {
    gapless_profile_reads: Arc<AtomicU64>,
}

impl Decoder for ProfileCountingDecoder {
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }

    fn gapless_profile(&self, _codec: Option<AudioCodec>) -> GaplessProfile {
        self.gapless_profile_reads.fetch_add(1, Ordering::AcqRel);
        GaplessProfile::new(self.spec(), None, None, 0)
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        Ok(DecoderChunkOutcome::Eof)
    }

    fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        Ok(DecoderSeekOutcome::Landed {
            landed_at: pos,
            landed_frame: 0,
            landed_byte: None,
            preroll: PrerollHint::NotNeeded,
        })
    }

    fn spec(&self) -> AudioSpec {
        AudioSpec::new(2, NonZeroU32::MIN)
    }

    fn update_byte_len(&self, _len: u64) {}
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, with)]
struct RouteSignalDecoder {
    drops: Arc<Mutex<Vec<u64>>>,
    pcm: Arc<[f32]>,
    gapless: Option<GaplessInfo>,
    remaining_chunks: Option<usize>,
    pools: Pools,
    sample_rate: u32,
    id: u64,
    next_frame: u64,
    #[field(with, vis = "")]
    timeline_gap: u64,
    phase: Option<Arc<Mutex<SourcePhase>>>,
}

impl RouteSignalDecoder {
    fn new(
        route_pcm: &RoutePcm,
        id: u64,
        sample_rate: u32,
        gapless: Option<GaplessInfo>,
        remaining_chunks: Option<usize>,
        drops: Arc<Mutex<Vec<u64>>>,
        pools: Pools,
    ) -> Self {
        let index = match sample_rate {
            44_100 => 0,
            48_000 => 1,
            _ => panic!("unprepared route sample rate: {sample_rate}"),
        };
        Self {
            drops,
            gapless,
            id,
            pools,
            remaining_chunks,
            sample_rate,
            pcm: route_pcm[index].clone(),
            next_frame: 0,
            timeline_gap: 0,
            phase: None,
        }
    }

    fn audio_spec(&self) -> AudioSpec {
        spec(self.sample_rate)
    }
}

impl Drop for RouteSignalDecoder {
    fn drop(&mut self) {
        self.drops.lock().push(self.id);
    }
}

impl Decoder for RouteSignalDecoder {
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(60))
    }

    fn gapless_profile(&self, _codec: Option<AudioCodec>) -> GaplessProfile {
        GaplessProfile::new(self.audio_spec(), self.gapless, None, 0)
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        if self.phase.as_ref().is_some_and(|phase| {
            matches!(
                *phase.lock(),
                SourcePhase::Waiting | SourcePhase::WaitingDemand | SourcePhase::WaitingMetadata
            )
        }) {
            return Ok(DecoderChunkOutcome::Pending(
                kithara_stream::PendingReason::NotReady(
                    kithara_stream::NotReadyCause::SourcePending,
                ),
            ));
        }
        if self.remaining_chunks == Some(0) {
            return Ok(DecoderChunkOutcome::Eof);
        }
        if let Some(remaining) = self.remaining_chunks.as_mut() {
            *remaining = remaining.saturating_sub(1);
        }
        let spec = self.audio_spec();
        let channels = usize::from(consts::REBUILD_CHANNELS);
        let frames = consts::ROUTE_CHUNK_FRAMES;
        let start_sample =
            usize::try_from(self.next_frame).expect("fixture frame index") * channels;
        let samples = &self.pcm[start_sample..start_sample + frames * channels];
        let frame_count = u32::try_from(frames).unwrap_or(u32::MAX);
        let start = self.next_frame;
        let end = start.saturating_add(u64::from(frame_count));
        self.next_frame = end;
        Ok(DecoderChunkOutcome::Chunk(AudioChunk::new(
            AudioChunkInfo {
                spec,
                timestamp: spec
                    .duration_for(start)
                    .expect("route signal timestamp fits Duration"),
                end_timestamp: spec
                    .duration_for(end)
                    .expect("route signal end timestamp fits Duration"),
                frame_offset: start,
                frames: frame_count,
                ..Default::default()
            },
            sample_buffer(&self.pools, samples),
        )))
    }

    fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        let frame = u64::try_from(
            self.audio_spec()
                .frames_for(pos)
                .expect("route signal seek fits frame count")
                .get(),
        )
        .unwrap_or(u64::MAX);
        self.next_frame = frame;
        Ok(DecoderSeekOutcome::Landed {
            landed_at: self
                .audio_spec()
                .duration_for(frame)
                .expect("route signal landing fits Duration"),
            landed_frame: frame,
            landed_byte: None,
            preroll: PrerollHint::NotNeeded,
        })
    }

    fn spec(&self) -> AudioSpec {
        self.audio_spec()
    }

    fn timeline_gap_frames(&self) -> u64 {
        self.timeline_gap
    }

    fn update_byte_len(&self, _len: u64) {}
}

pub(super) struct TestControl {
    byte_map_enabled: AtomicBool,
    demand_in_flight: AtomicBool,
    exact_reader_ready: AtomicBool,
    exact_reader_taken: AtomicBool,
    plan_calls: AtomicU64,
    prepare_calls: AtomicU64,
    promote_calls: AtomicU64,
    take_calls: AtomicU64,
    aborted_transition: Mutex<Option<VariantTransition>>,
    exact_plan: Mutex<Option<VariantReaderPlan>>,
    format_range: Mutex<Option<Range<u64>>>,
    landing: Mutex<Option<Duration>>,
    media_info: Mutex<Option<MediaInfo>>,
    prepared_profile: Mutex<Option<ReaderProfile>>,
    promotion: Mutex<VariantPromotion>,
}

impl TestControl {
    pub(super) fn new(media_info: MediaInfo) -> Self {
        Self {
            aborted_transition: Mutex::new(None),
            byte_map_enabled: AtomicBool::new(false),
            demand_in_flight: AtomicBool::new(false),
            exact_plan: Mutex::new(None),
            exact_reader_ready: AtomicBool::new(false),
            exact_reader_taken: AtomicBool::new(false),
            landing: Mutex::new(None),
            media_info: Mutex::new(Some(media_info)),
            plan_calls: AtomicU64::new(0),
            prepare_calls: AtomicU64::new(0),
            prepared_profile: Mutex::new(None),
            promote_calls: AtomicU64::new(0),
            promotion: Mutex::new(VariantPromotion::Stale),
            take_calls: AtomicU64::new(0),
            format_range: Mutex::new(Some(0..32)),
        }
    }

    pub(super) fn aborted_transition(&self) -> Option<VariantTransition> {
        *self.aborted_transition.lock()
    }

    pub(super) fn enable_byte_map(&self) {
        self.byte_map_enabled.store(true, Ordering::Release);
    }

    pub(super) fn landing(&self) -> Option<Duration> {
        *self.landing.lock()
    }

    pub(super) fn plan_calls(&self) -> u64 {
        self.plan_calls.load(Ordering::Acquire)
    }

    pub(super) fn prepare_calls(&self) -> u64 {
        self.prepare_calls.load(Ordering::Acquire)
    }

    pub(super) fn prepared_profile(&self) -> Option<ReaderProfile> {
        *self.prepared_profile.lock()
    }

    pub(super) fn promote_calls(&self) -> u64 {
        self.promote_calls.load(Ordering::Acquire)
    }

    pub(super) fn set_demand_in_flight(&self, in_flight: bool) {
        self.demand_in_flight.store(in_flight, Ordering::Release);
    }

    pub(super) fn set_exact_plan(&self, plan: VariantReaderPlan) {
        *self.exact_plan.lock() = Some(plan);
        *self.prepared_profile.lock() = None;
        self.exact_reader_ready.store(false, Ordering::Release);
        self.exact_reader_taken.store(false, Ordering::Release);
    }

    pub(super) fn set_exact_reader_ready(&self) {
        self.exact_reader_ready.store(true, Ordering::Release);
    }

    /// Publish a new active variant on the source, the way a promoted ABR
    /// switch does. `rebuild::policy::superseded` reads exactly this.
    fn set_media_info(&self, media_info: MediaInfo) {
        *self.media_info.lock() = Some(media_info);
    }

    pub(super) fn set_promotion(&self, promotion: VariantPromotion) {
        *self.promotion.lock() = promotion;
    }

    pub(super) fn take_calls(&self) -> u64 {
        self.take_calls.load(Ordering::Acquire)
    }
}

impl VariantControl for TestControl {
    fn abort_variant(&self, transition: VariantTransition) -> bool {
        let mut exact_plan = self.exact_plan.lock();
        if exact_plan
            .as_ref()
            .is_none_or(|plan| plan.transition() != transition)
        {
            return false;
        }
        *exact_plan = None;
        drop(exact_plan);
        *self.aborted_transition.lock() = Some(transition);
        true
    }

    fn format_change_segment_range(&self) -> StreamResult<Range<u64>> {
        self.format_range
            .lock()
            .clone()
            .ok_or(StreamError::Source(SourceError::FormatChangeNotApplicable))
    }

    fn plan_variant_reader(
        &self,
        landing: Option<Duration>,
    ) -> StreamResult<Option<VariantReaderPlan>> {
        self.plan_calls.fetch_add(1, Ordering::AcqRel);
        if let Some(landing) = landing {
            *self.landing.lock() = Some(landing);
        }
        Ok(self.exact_plan.lock().clone())
    }

    fn prepare_variant_reader(
        &self,
        plan: VariantReaderPlan,
        profile: ReaderProfile,
    ) -> StreamResult<Option<VariantTransition>> {
        self.prepare_calls.fetch_add(1, Ordering::AcqRel);
        *self.prepared_profile.lock() = Some(profile);
        Ok((self.exact_plan.lock().as_ref() == Some(&plan)).then(|| plan.transition()))
    }

    fn promote_variant(&self, transition: VariantTransition) -> VariantPromotion {
        self.promote_calls.fetch_add(1, Ordering::AcqRel);
        if !self
            .exact_plan
            .lock()
            .as_ref()
            .is_some_and(|plan| plan.transition() == transition)
        {
            return VariantPromotion::Stale;
        }
        let promotion = *self.promotion.lock();
        if promotion == VariantPromotion::Promoted {
            *self.exact_plan.lock() = None;
        }
        promotion
    }

    fn take_prepared_variant_reader(
        &self,
        transition: VariantTransition,
    ) -> StreamResult<VariantReaderTake> {
        self.take_calls.fetch_add(1, Ordering::AcqRel);
        let Some(plan) = self
            .exact_plan
            .lock()
            .clone()
            .filter(|plan| plan.transition() == transition)
        else {
            return Ok(VariantReaderTake::Stale);
        };
        if !self.exact_reader_ready.load(Ordering::Acquire) {
            return Ok(VariantReaderTake::Preparing);
        }
        if self.exact_reader_taken.swap(true, Ordering::AcqRel) {
            return Ok(VariantReaderTake::Taken);
        }
        let reader = OpenedReader::new(Cursor::new(Vec::new()), Some(0), None, None, None);
        Ok(VariantReaderTake::Ready(OpenedVariantReader::new(
            plan, reader,
        )))
    }

    fn transition_demand_in_flight(&self, transition: VariantTransition) -> bool {
        self.demand_in_flight.load(Ordering::Acquire)
            && self
                .exact_plan
                .lock()
                .as_ref()
                .is_some_and(|plan| plan.transition() == transition)
    }
}

/// Optional park inside `wait_range`, letting a test hold the stream's
/// control mutex the way a real construction read does: `Stream::read`
/// enters `Source::wait_range` under the `SharedStream` mutex and stays
/// there until data lands. Disarmed by default — no other test changes.
#[derive(Default)]
pub(super) struct WaitPark {
    armed: AtomicBool,
    condvar: Condvar,
    state: Mutex<WaitParkState>,
    entered: Notify,
}

#[derive(Default)]
struct WaitParkState {
    entered: bool,
    released: bool,
}

impl WaitPark {
    pub(super) fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }

    fn enter_if_armed(&self) {
        if !self.armed.load(Ordering::Acquire) {
            return;
        }
        let mut state = self.state.lock();
        state.entered = true;
        self.entered.notify_one();
        while !state.released {
            state = self.condvar.wait(state);
        }
        drop(state);
    }

    pub(super) fn release(&self) {
        let mut state = self.state.lock();
        state.released = true;
        drop(state);
        self.condvar.notify_all();
    }

    /// Wait until the holder is inside `wait_range` — i.e. the control
    /// mutex is held by a parked blocking read.
    pub(super) async fn wait_entered(&self) {
        while !self.state.lock().entered {
            self.entered.notified().await;
        }
    }
}

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, with)]
pub(super) struct TestSource {
    byte_map: Arc<TestByteMap>,
    control: Arc<TestControl>,
    park: Arc<WaitPark>,
    phase: Arc<Mutex<SourcePhase>>,
    playhead: Arc<PlayheadState>,
    position: Arc<AtomicU64>,
    activity: Activity,
    writer: Option<ActivityWriter>,
    waits: Arc<Mutex<Vec<Range<u64>>>>,
    #[field(with = with_peer_wake, option_set_some, vis = "pub(super)")]
    peer: Option<Arc<DeferredWake>>,
}

impl TestSource {
    pub(super) fn new(control: Arc<TestControl>) -> Self {
        let writer = ActivityWriter::new();
        Self {
            activity: writer.reader(),
            writer: Some(writer),
            control,
            byte_map: Arc::new(TestByteMap),
            park: Arc::new(WaitPark::default()),
            peer: None,
            phase: Arc::new(Mutex::new(SourcePhase::Ready)),
            playhead: Arc::new(PlayheadState::new()),
            position: Arc::new(AtomicU64::new(0)),
            waits: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn park_handle(&self) -> Arc<WaitPark> {
        Arc::clone(&self.park)
    }

    pub(super) fn phase_handle(&self) -> Arc<Mutex<SourcePhase>> {
        Arc::clone(&self.phase)
    }

    fn segmented(control: Arc<TestControl>) -> Self {
        control.enable_byte_map();
        Self::new(control)
    }

    pub(super) fn waits_handle(&self) -> Arc<Mutex<Vec<Range<u64>>>> {
        Arc::clone(&self.waits)
    }
}

/// Byte-space probe sharing the test source's scripted cells — the same
/// phase, cursor, and byte-map gating as the `Source` impl below.
struct SharedPhaseProbe {
    byte_map: Arc<TestByteMap>,
    control: Arc<TestControl>,
    phase: Arc<Mutex<SourcePhase>>,
    position: Arc<AtomicU64>,
}

impl SourceProbe for SharedPhaseProbe {
    fn byte_map(&self) -> Option<Arc<dyn ByteMap>> {
        if self.control.byte_map_enabled.load(Ordering::Acquire) {
            Some(self.byte_map.clone() as Arc<dyn ByteMap>)
        } else {
            None
        }
    }

    fn len(&self) -> Option<u64> {
        Some(4096)
    }

    fn phase(&self) -> SourcePhase {
        *self.phase.lock()
    }

    fn phase_at(&self, _range: Range<u64>) -> SourcePhase {
        *self.phase.lock()
    }

    fn position(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    fn set_position(&self, pos: u64) {
        self.position.store(pos, Ordering::Release);
    }
}

impl Source for TestSource {
    fn activity(&self) -> Activity {
        self.activity.clone()
    }

    fn take_activity_writer(&mut self) -> Option<ActivityWriter> {
        self.writer.take()
    }

    fn advance(&self, n: u64) {
        self.position.fetch_add(n, Ordering::AcqRel);
    }

    fn byte_map(&self) -> Option<Arc<dyn ByteMap>> {
        if self.control.byte_map_enabled.load(Ordering::Acquire) {
            Some(self.byte_map.clone() as Arc<dyn ByteMap>)
        } else {
            None
        }
    }

    fn len(&self) -> Option<u64> {
        Some(4096)
    }

    fn media_info(&self) -> Option<MediaInfo> {
        self.control.media_info.lock().clone()
    }

    fn peer_wake(&self) -> Option<Arc<DeferredWake>> {
        self.peer.clone()
    }

    fn phase_at(&self, _range: Range<u64>) -> SourcePhase {
        *self.phase.lock()
    }

    fn playhead_read(&self) -> Arc<dyn PlayheadRead> {
        Arc::clone(&self.playhead) as Arc<dyn PlayheadRead>
    }

    fn playhead_write(&self) -> Arc<dyn PlayheadWrite> {
        Arc::clone(&self.playhead) as Arc<dyn PlayheadWrite>
    }

    fn position(&self) -> u64 {
        self.position.load(Ordering::Acquire)
    }

    fn probe(&self) -> Arc<dyn SourceProbe> {
        Arc::new(SharedPhaseProbe {
            phase: Arc::clone(&self.phase),
            position: Arc::clone(&self.position),
            control: Arc::clone(&self.control),
            byte_map: Arc::clone(&self.byte_map),
        })
    }

    fn read_at(&mut self, _offset: u64, _buf: &mut [u8]) -> StreamResult<ReadOutcome> {
        Ok(ReadOutcome::Eof)
    }

    fn set_position(&self, pos: u64) {
        self.position.store(pos, Ordering::Release);
    }

    fn variant_control(&self) -> Option<Arc<dyn VariantControl>> {
        Some(Arc::clone(&self.control) as Arc<dyn VariantControl>)
    }

    fn wait_range(
        &mut self,
        range: Range<u64>,
        _timeout: Option<Duration>,
    ) -> StreamResult<WaitOutcome> {
        self.park.enter_if_armed();
        self.waits.lock().push(range);
        match *self.phase.lock() {
            SourcePhase::Ready => Ok(WaitOutcome::Ready),
            SourcePhase::Eof => Ok(WaitOutcome::Eof),
            _ => Err(StreamError::Source(SourceError::WaitBudgetExceeded)),
        }
    }
}

struct TestByteMap;

impl TestByteMap {
    const CONTAINER_ORIGIN: u64 = 0;
    const INIT_BYTES: u64 = 627;
    const SEGMENT_BYTES: u64 = 4096;
    const SEGMENT_SECS: u64 = 4;

    fn descriptor(index: u64) -> SegmentDescriptor {
        let start = Self::INIT_BYTES.saturating_add(index.saturating_mul(Self::SEGMENT_BYTES));
        SegmentDescriptor::new(
            start..start.saturating_add(Self::SEGMENT_BYTES),
            Duration::from_secs(index.saturating_mul(Self::SEGMENT_SECS)),
            Duration::from_secs(Self::SEGMENT_SECS),
            u32::try_from(index).unwrap_or(u32::MAX),
            0,
        )
    }
}

impl ByteMap for TestByteMap {
    fn anchor_at_time(&self, position: Duration) -> StreamResult<Option<SourceSeekAnchor>> {
        let segment = Self::descriptor(position.as_secs() / Self::SEGMENT_SECS);
        Ok(Some(
            SourceSeekAnchor::builder()
                .segment_start(segment.decode_time)
                .segment_end(segment.decode_time.saturating_add(segment.duration))
                .segment_index(segment.segment_index)
                .variant_index(segment.variant_index)
                .byte_offset(segment.byte_range.start)
                .build(),
        ))
    }

    fn init_segment_range(&self) -> Range<u64> {
        Self::CONTAINER_ORIGIN..Self::INIT_BYTES
    }

    fn len(&self) -> Option<u64> {
        Some(Self::INIT_BYTES.saturating_add(Self::SEGMENT_BYTES))
    }

    fn segment_after_byte(&self, byte_offset: u64) -> Option<SegmentDescriptor> {
        (byte_offset < Self::INIT_BYTES).then(|| Self::descriptor(0))
    }

    fn segment_at_time(&self, t: Duration) -> Option<SegmentDescriptor> {
        Some(Self::descriptor(t.as_secs() / Self::SEGMENT_SECS))
    }

    fn segment_count(&self) -> Option<u32> {
        Some(1)
    }
}

pub(super) struct TestConfig {
    pub(super) source: TestSource,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            source: TestSource::new(Arc::new(TestControl::new(media_info(0)))),
        }
    }
}

pub(super) struct TestStream;

impl StreamType for TestStream {
    type Config = TestConfig;
    type Events = ();
    type Source = TestSource;

    async fn create(config: Self::Config) -> Result<Self::Source, SourceError> {
        Ok(config.source)
    }
}

pub(super) fn media_info(variant: u32) -> MediaInfo {
    let mut info = MediaInfo::builder()
        .maybe_codec(Some(AudioCodec::AacLc))
        .maybe_container(Some(ContainerFormat::Fmp4))
        .build();
    info.variant_index = Some(variant);
    info
}

fn recreate_state(variant: u32) -> RecreateState {
    RecreateState {
        media_info: Some(media_info(variant)),
        cause: RecreateCause::FormatBoundary,
        offset: 0,
    }
}

pub(super) struct RebuildFixture {
    pub(super) control: Arc<TestControl>,
    pub(super) drops: Arc<Mutex<Vec<u64>>>,
    pub(super) pools: Pools,
    pub(super) source: StreamAudioSource<TestStream>,
}

pub(super) struct RouteFixture {
    pub(super) control: Arc<TestControl>,
    pub(super) drops: Arc<Mutex<Vec<u64>>>,
    pub(super) phase: Arc<Mutex<SourcePhase>>,
    pub(super) pools: Pools,
    pub(super) source: StreamAudioSource<TestStream>,
}

pub(super) async fn test_source(variant: u32) -> RebuildFixture {
    test_source_with_mode(variant, GaplessMode::Disabled).await
}

#[kithara::test(native, tokio)]
async fn decoder_readers_have_isolated_construction_gates() {
    let control = Arc::new(TestControl::new(media_info(0)));
    let stream = match Stream::<TestStream>::new(TestConfig {
        source: TestSource::new(control),
    })
    .await
    {
        Ok(stream) => stream,
        Err(error) => panic!("test stream construction failed: {error}"),
    };
    let shared_stream = SharedStream::new(stream);
    let initial = shared_stream.open_initial_reader();
    let rebuild = shared_stream.open_rebuild_reader(0);
    let Some(initial_gate) = initial.construction_gate() else {
        panic!("initial reader must carry a construction gate");
    };
    let Some(rebuild_gate) = rebuild.construction_gate() else {
        panic!("rebuild reader must carry a construction gate");
    };

    initial_gate.arm();

    assert!(initial_gate.is_armed());
    assert!(!rebuild_gate.is_armed());
}

async fn test_source_with_mode(variant: u32, gapless_mode: GaplessMode) -> RebuildFixture {
    let pools = pools();
    let control = Arc::new(TestControl::new(media_info(variant)));
    let drops = Arc::new(Mutex::new(Vec::new()));
    let stream = Stream::<TestStream>::new(TestConfig { source: TestSource::new(control.clone()) })
        .await.expect("test stream");
    let shared_stream = SharedStream::new(stream);
    let factory_drops = drops.clone();
    let decoder_factory = DecoderFactory::new(
        move |_reader, _info, _rate| Ok(Box::new(TestDecoder::new(99, factory_drops.clone()))),
        None,
    );
    let decode = ActiveDecode::new(
        DecoderGeneration::new(
            Box::new(TestDecoder::new(1, drops.clone())),
            Some(media_info(0)),
            0,
            None,
            None,
            gapless_mode,
        ),
        gapless_mode,
        None,
        &pools,
    )
    .expect("decode scratch fits test pools");
    let source = StreamAudioSource::new(
        shared_stream,
        decode,
        decoder_factory,
        NonZeroU32::new(consts::SAMPLE_RATE),
        kithara_decode::DecoderBackend::default(),
        "none",
        Arc::new(DeferredBus::new(EventBus::default(), 16)),
        Arc::new(NoopWorkerWake),
    );
    RebuildFixture {
        control,
        drops,
        pools,
        source,
    }
}

/// `segmented` vends the byte map HLS supplies plus an init-bearing
/// decoder factory: the rebuilt demuxer parses only when it is rooted at
/// the container origin, exactly like the Apple fMP4 segment path. A flat
/// source has neither, so no recreate origin other than `base_offset` is
/// even reachable on it.
struct RouteParams {
    chunks_before_eof: Option<usize>,
    gapless: Option<GaplessInfo>,
    incoming_chunks_before_eof: Option<usize>,
    segmented: bool,
    initial_host_rate: u32,
    active_timeline_gap: u64,
    incoming_timeline_gap: u64,
}

pub(super) async fn route_signal_source(
    route_pcm: &RoutePcm,
    initial_host_rate: u32,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            initial_host_rate,
            chunks_before_eof: None,
            gapless: None,
            incoming_chunks_before_eof: None,
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            segmented: false,
        },
    )
    .await
}

pub(super) async fn route_signal_source_with_eof(
    route_pcm: &RoutePcm,
    initial_host_rate: u32,
    chunks_before_eof: usize,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            initial_host_rate,
            chunks_before_eof: Some(chunks_before_eof),
            gapless: None,
            incoming_chunks_before_eof: None,
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            segmented: false,
        },
    )
    .await
}

pub(super) async fn route_signal_source_with_gapless(
    route_pcm: &RoutePcm,
    initial_host_rate: u32,
    gapless: GaplessInfo,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            initial_host_rate,
            chunks_before_eof: None,
            gapless: Some(gapless),
            incoming_chunks_before_eof: None,
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            segmented: false,
        },
    )
    .await
}

pub(super) async fn route_signal_source_with_gapless_eof(
    route_pcm: &RoutePcm,
    initial_host_rate: u32,
    gapless: GaplessInfo,
    chunks_before_eof: usize,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            initial_host_rate,
            chunks_before_eof: Some(chunks_before_eof),
            gapless: Some(gapless),
            incoming_chunks_before_eof: None,
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            segmented: false,
        },
    )
    .await
}

/// Both sides of a transition run out of source, on the same media length.
///
/// Two variants of one track end together, so an incoming that lands at the container origin
/// stages to the very frame the outgoing frontier stops at. Which decoder reports its exhaustion
/// first is then a race, and this fixture pins the order the race can take.
pub(super) async fn route_signal_source_with_finite_sides(
    route_pcm: &RoutePcm,
    initial_host_rate: u32,
    chunks_before_eof: usize,
    incoming_chunks_before_eof: usize,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            initial_host_rate,
            chunks_before_eof: Some(chunks_before_eof),
            gapless: None,
            incoming_chunks_before_eof: Some(incoming_chunks_before_eof),
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            segmented: false,
        },
    )
    .await
}

pub(super) async fn route_signal_source_with_finite_incoming(
    route_pcm: &RoutePcm,
    initial_host_rate: u32,
    incoming_chunks_before_eof: usize,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            initial_host_rate,
            chunks_before_eof: None,
            gapless: None,
            incoming_chunks_before_eof: Some(incoming_chunks_before_eof),
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            segmented: false,
        },
    )
    .await
}

async fn route_source(route_pcm: &RoutePcm, params: RouteParams) -> RouteFixture {
    let pools = pools();
    let control = Arc::new(TestControl::new(media_info(0)));
    let drops = Arc::new(Mutex::new(Vec::new()));
    let chunks_before_eof = params.chunks_before_eof;
    let gapless = params.gapless;
    let incoming_chunks_before_eof = params.incoming_chunks_before_eof;
    let active_timeline_gap = params.active_timeline_gap;
    let incoming_timeline_gap = params.incoming_timeline_gap;
    let segmented = params.segmented;
    let test_source = if segmented {
        TestSource::segmented(control.clone())
    } else {
        TestSource::new(control.clone())
    };
    let phase = test_source.phase_handle();
    let stream = match Stream::<TestStream>::new(TestConfig {
        source: test_source,
    })
    .await
    {
        Ok(stream) => stream,
        Err(err) => panic!("test stream construction failed: {err}"),
    };
    let shared_stream = SharedStream::new(stream);
    let container_byte_len = shared_stream.len();
    let factory_drops = drops.clone();
    let factory_pools = pools.clone();
    let factory_pcm = route_pcm.clone();
    let decoder_factory = DecoderFactory::new(
        move |reader, _info, host_rate| {
            if segmented && reader.byte_len() != container_byte_len {
                return Err(DecodeError::InvalidData {
                    detail: "init-bearing container demuxed from a media byte",
                });
            }
            let rate = host_rate.map_or(consts::SAMPLE_RATE, NonZeroU32::get);
            Ok(Box::new(
                RouteSignalDecoder::new(
                    &factory_pcm,
                    99,
                    rate,
                    gapless,
                    incoming_chunks_before_eof,
                    factory_drops.clone(),
                    factory_pools.clone(),
                )
                .with_timeline_gap(incoming_timeline_gap),
            ))
        },
        None,
    );
    let mode = if gapless.is_some() {
        GaplessMode::MediaOnly
    } else {
        GaplessMode::Disabled
    };
    let mut decoder = RouteSignalDecoder::new(
        route_pcm,
        1,
        consts::SAMPLE_RATE,
        gapless,
        chunks_before_eof,
        drops.clone(),
        pools.clone(),
    )
    .with_timeline_gap(active_timeline_gap);
    decoder.phase = Some(phase.clone());
    let decode = ActiveDecode::new(
        DecoderGeneration::new(Box::new(decoder), Some(media_info(0)), 0, None, None, mode),
        mode,
        None,
        &pools,
    )
    .expect("decode scratch fits pools");
    let source = StreamAudioSource::new(
        shared_stream,
        decode,
        decoder_factory,
        NonZeroU32::new(params.initial_host_rate),
        kithara_decode::DecoderBackend::default(),
        "none",
        Arc::new(DeferredBus::new(EventBus::default(), 16)),
        Arc::new(NoopWorkerWake),
    );
    RouteFixture {
        control,
        drops,
        phase,
        pools,
        source,
    }
}

pub(super) async fn route_signal_source_with_gaps(
    route_pcm: &RoutePcm,
    active_timeline_gap: u64,
    incoming_timeline_gap: u64,
) -> RouteFixture {
    route_source(
        route_pcm,
        RouteParams {
            active_timeline_gap,
            incoming_timeline_gap,
            chunks_before_eof: None,
            gapless: None,
            incoming_chunks_before_eof: None,
            initial_host_rate: consts::SAMPLE_RATE,
            segmented: false,
        },
    )
    .await
}

fn run_pending_rebuild_inline(source: &mut StreamAudioSource<TestStream>) {
    source.prepare_deferred();
    source.finish_deferred();
}

fn append_left_channel(left: &mut Vec<f32>, chunk: &AudioChunk) {
    let channels = usize::from(chunk.meta.spec.channels);
    for frame in 0..chunk.frames() {
        left.push(chunk.samples[frame * channels]);
    }
}

fn peak_first_diff(left: &[f32], center: usize, half: usize) -> f32 {
    assert!(
        (1..left.len()).contains(&center),
        "first-difference center must be in 1..{}, got {center}",
        left.len(),
    );
    let start = center.saturating_sub(half).max(1);
    let end = center.saturating_add(half).min(left.len() - 1);
    let mut peak = 0.0_f32;
    for i in start..=end {
        peak = peak.max((left[i] - left[i - 1]).abs());
    }
    peak
}

fn next_test_chunk(
    source: &mut StreamAudioSource<TestStream>,
    route_recreated: &mut bool,
) -> AudioChunk {
    let chunk = next_decoded_chunk(source, route_recreated);
    source.commit_source_end(SourceEnd::new(
        chunk
            .meta
            .frame_offset
            .saturating_add(u64::from(chunk.meta.frames)),
        chunk.meta.spec.sample_rate,
    ));
    chunk
}

fn next_decoded_chunk(
    source: &mut StreamAudioSource<TestStream>,
    route_recreated: &mut bool,
) -> AudioChunk {
    loop {
        run_pending_rebuild_inline(source);
        *route_recreated |=
            source.decode.output_spec().sample_rate.get() == consts::ROUTE_SAMPLE_RATE;
        match source.step_track() {
            TrackStep::Produced(fetch) => return produced_data(fetch),
            TrackStep::StateChanged => {
                *route_recreated |=
                    source.decode.output_spec().sample_rate.get() != consts::SAMPLE_RATE;
            }
            TrackStep::Blocked(_) => {}
            TrackStep::Eof => panic!("route test source reached EOF"),
            TrackStep::Failed(_) => panic!("route test source failed"),
        }
    }
}

fn enter_rebuilding(source: &mut StreamAudioSource<TestStream>, recreate: RecreateState) {
    source
        .install_replacement(recreate, None)
        .expect("synchronous replacement");
}

fn install_test_factory(
    source: &mut StreamAudioSource<TestStream>,
    id: u64,
    drops: Arc<Mutex<Vec<u64>>>,
) {
    source.factory = DecoderFactory::new(
        move |_reader, _info, _rate| Ok(Box::new(TestDecoder::new(id, drops.clone()))),
        None,
    );
}

fn exact_incoming_plan() -> VariantReaderPlan {
    let abr = AbrState::new(AbrMode::Auto(Some(VariantIndex::new(0))));
    abr.request_target(VariantIndex::new(1), AbrReason::ManualOverride);
    let claim = abr
        .claim_pending_decision(VariantIndex::new(0))
        .expect("incoming rebuild fixture requires an exact ABR claim");
    let transition = VariantTransition::new(
        VariantTransitionId::new(claim.ticket()),
        VariantIndex::new(0),
        VariantIndex::new(1),
    );
    VariantReaderPlan::new(transition, media_info(1), Duration::ZERO)
}

fn route_generation(
    route_pcm: &RoutePcm,
    pools: &Pools,
    decoder_id: u64,
    variant: u32,
    drops: Arc<Mutex<Vec<u64>>>,
) -> DecoderGeneration {
    DecoderGeneration::new(
        Box::new(RouteSignalDecoder::new(
            route_pcm,
            decoder_id,
            consts::SAMPLE_RATE,
            None,
            None,
            drops,
            pools.clone(),
        )),
        Some(media_info(variant)),
        0,
        None,
        None,
        GaplessMode::Disabled,
    )
}

fn install_route_factory(
    route_pcm: &RoutePcm,
    pools: &Pools,
    source: &mut StreamAudioSource<TestStream>,
    id: u64,
    drops: Arc<Mutex<Vec<u64>>>,
) {
    let pcm = route_pcm.clone();
    let pools = pools.clone();
    source.factory = DecoderFactory::new(
        move |_reader, _info, rate| {
            Ok(Box::new(RouteSignalDecoder::new(
                &pcm,
                id,
                rate.map_or(consts::SAMPLE_RATE, NonZeroU32::get),
                None,
                None,
                drops.clone(),
                pools.clone(),
            )))
        },
        None,
    );
}

fn assert_replacement_decodes(source: &mut StreamAudioSource<TestStream>) {
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    assert!(matches!(source.step_track(), TrackStep::Produced(_)));
}

#[kithara::test(tokio)]
async fn matching_replacement_aborts_primed_incoming_before_profile_prepare(route_pcm: RoutePcm) {
    let RebuildFixture {
        control,
        drops,
        pools,
        mut source,
    } = test_source(1).await;
    let plan = exact_incoming_plan();
    let transition = plan.transition();
    control.set_exact_plan(plan);
    assert!(
        source
            .decode
            .begin_incoming(transition, OutgoingFrontier::Awaiting)
            .is_none()
    );
    assert!(source.decode.incoming_is_preparing(transition));
    let incoming = route_generation(&route_pcm, &pools, 8, 1, drops.clone());
    assert!(
        source
            .decode
            .install_incoming(transition, incoming)
            .is_none()
    );
    assert!(source.decode.incoming_is_priming(transition));
    let factory_control = control.clone();
    let factory_drops = drops.clone();
    let factory_pools = pools.clone();
    source.factory = DecoderFactory::new(
        move |_reader, _info, rate| {
            assert_eq!(factory_control.aborted_transition(), Some(transition));
            assert_eq!(factory_drops.lock().as_slice(), &[8]);
            Ok(Box::new(RouteSignalDecoder::new(
                &route_pcm,
                2,
                rate.map_or(consts::SAMPLE_RATE, NonZeroU32::get),
                None,
                None,
                factory_drops.clone(),
                factory_pools.clone(),
            )))
        },
        None,
    );
    enter_rebuilding(&mut source, recreate_state(1));

    assert_eq!(source.decode.incoming_transition(), None);
    assert_eq!(control.aborted_transition(), Some(transition));
    assert_eq!(drops.lock().as_slice(), &[8, 1]);
    assert_replacement_decodes(&mut source);
    source.finish_deferred();
    assert_eq!(drops.lock().as_slice(), &[8, 1]);
}

#[kithara::test(tokio)]
async fn replacement_aborts_building_incoming_and_retires_its_late_completion(route_pcm: RoutePcm) {
    let RebuildFixture {
        control,
        drops,
        pools,
        mut source,
    } = test_source(1).await;
    let plan = exact_incoming_plan();
    let transition = plan.transition();
    control.set_exact_plan(plan);
    assert!(
        source
            .decode
            .begin_incoming(transition, OutgoingFrontier::Awaiting)
            .is_none()
    );
    assert!(source.decode.incoming_is_preparing(transition));
    let incoming = route_generation(&route_pcm, &pools, 8, 1, drops.clone());

    install_route_factory(&route_pcm, &pools, &mut source, 2, drops.clone());
    enter_rebuilding(&mut source, recreate_state(1));
    let rejected = source.decode.install_incoming(transition, incoming);
    assert!(rejected.is_some());
    drop(rejected);
    assert_eq!(source.decode.incoming_transition(), None);
    assert_eq!(control.aborted_transition(), Some(transition));
    assert_eq!(drops.lock().as_slice(), &[1, 8]);
    assert_replacement_decodes(&mut source);
    source.finish_deferred();
    assert_eq!(drops.lock().as_slice(), &[1, 8]);
}

#[kithara::test(tokio)]
async fn transition_wait_with_demand_in_flight_is_upstream_pending() {
    let RebuildFixture {
        control,
        mut source,
        ..
    } = test_source(1).await;
    let plan = exact_incoming_plan();
    let transition = plan.transition();
    control.set_exact_plan(plan);
    assert!(
        source
            .decode
            .begin_incoming(transition, OutgoingFrontier::Awaiting)
            .is_none()
    );
    control.set_demand_in_flight(true);

    assert_eq!(
        source.transition_wait_reason(),
        WaitingReason::WaitingDemand
    );
}

#[kithara::test(tokio)]
async fn transition_wait_without_demand_stays_watchdog_visible() {
    let RebuildFixture {
        control,
        mut source,
        ..
    } = test_source(1).await;
    let plan = exact_incoming_plan();
    let transition = plan.transition();
    control.set_exact_plan(plan);
    assert!(
        source
            .decode
            .begin_incoming(transition, OutgoingFrontier::Awaiting)
            .is_none()
    );

    assert_eq!(source.transition_wait_reason(), WaitingReason::Waiting);
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_pending_poll_blocks() {
    let RebuildFixture { mut source, .. } = test_source(1).await;
    let transition = exact_incoming_plan().transition();
    source
        .decode
        .begin_incoming(transition, OutgoingFrontier::Awaiting);
    assert!(matches!(
        source.step_track(),
        TrackStep::Blocked(WaitingReason::Waiting)
    ));
    assert!(source.decode.incoming_is_preparing(transition));
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_completion_waits_for_shell_routing() {
    let RebuildFixture {
        drops, mut source, ..
    } = test_source(1).await;
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
    assert!(drops.lock().is_empty());
    install_test_factory(&mut source, 2, drops.clone());
    enter_rebuilding(&mut source, recreate_state(1));
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    assert_eq!(drops.lock().as_slice(), &[1]);
    source.finish_deferred();
    source.finish_deferred();
    assert_eq!(drops.lock().as_slice(), &[1]);
}

#[kithara::test(tokio)]
async fn rebuild_prepares_generation_profiles_before_rt_install() {
    let RebuildFixture { mut source, .. } =
        test_source_with_mode(1, GaplessMode::SilenceTrim(SilenceTrimParams::default())).await;
    let profile_reads = Arc::new(AtomicU64::new(0));
    let factory_reads = profile_reads.clone();
    source.factory = DecoderFactory::new(
        move |_reader, _info, _rate| {
            Ok(Box::new(ProfileCountingDecoder {
                gapless_profile_reads: factory_reads.clone(),
            }))
        },
        None,
    );
    assert_eq!(profile_reads.load(Ordering::Acquire), 0);
    enter_rebuilding(&mut source, recreate_state(1));
    assert_eq!(profile_reads.load(Ordering::Acquire), 1);
    source.finish_deferred();
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(profile_reads.load(Ordering::Acquire), 1);
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_completion_installs_once() {
    let RebuildFixture {
        drops, mut source, ..
    } = test_source(1).await;
    install_test_factory(&mut source, 2, drops.clone());
    enter_rebuilding(&mut source, recreate_state(1));
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    assert!(source.decode.incoming_transition().is_none());
    assert_eq!(drops.lock().as_slice(), &[1]);
    source.finish_deferred();
    assert_eq!(drops.lock().as_slice(), &[1]);
}

#[kithara::test(tokio)]
async fn format_boundary_rebuild_rebases_decode_head_to_rendered_source(route_pcm: RoutePcm) {
    let RouteFixture {
        control,
        drops,
        pools,
        mut source,
        ..
    } = route_signal_source(&route_pcm, consts::SAMPLE_RATE).await;
    let mut route_recreated = false;
    let chunk = next_decoded_chunk(&mut source, &mut route_recreated);

    let raw = source
        .resume
        .decode_head()
        .expect("decoded chunk must advance the raw head");
    let rendered_frame = chunk
        .meta
        .frame_offset
        .saturating_add(u64::from(chunk.meta.frames / 2));
    let rendered = (rendered_frame, chunk.meta.spec.sample_rate.get());
    source.commit_source_end(SourceEnd::new(rendered_frame, chunk.meta.spec.sample_rate));
    assert!(
        raw.0 > rendered.0,
        "fixture requires raw PCM ahead of output"
    );

    let landing = spec(rendered.1)
        .duration_for(rendered.0)
        .expect("rendered position");
    control.set_media_info(media_info(1));
    install_route_factory(&route_pcm, &pools, &mut source, 2, drops);
    source
        .install_replacement(
            recreate_state(1),
            Some(SourceEnd::new(rendered_frame, chunk.meta.spec.sample_rate)),
        )
        .expect("replacement at rendered frontier");
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(source.resume.decode_head(), Some(rendered));
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    control.set_exact_plan(exact_incoming_plan());
    source.prepare_deferred();
    source.finish_deferred();
    assert_eq!(
        control.landing(),
        Some(
            spec(rendered.1)
                .duration_for(rendered.0)
                .expect("rendered fixture landing fits Duration"),
        ),
        "the next ABR plan must start from the rebuilt rendered frontier"
    );
    let mut rebuilt = false;
    let chunk = next_decoded_chunk(&mut source, &mut rebuilt);
    assert_eq!(chunk.meta.timestamp, landing);
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_completion_emits_decoder_changed_cause() {
    let RebuildFixture { mut source, .. } = test_source(1).await;
    let bus = EventBus::new(16);
    let mut events = bus.subscribe();
    source.emit = Arc::new(DeferredBus::new(bus, 16));
    enter_rebuilding(&mut source, recreate_state(1));
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert!(events.try_recv().is_err());
    source.finish_deferred();
    assert!(matches!(
        events.try_recv().map(|envelope| envelope.event),
        Ok(AudioLaneEvent::Decoder(DecoderEvent::DecoderChanged {
            cause: DecoderChangeCause::FormatBoundary,
            ..
        }))
    ));
}

#[kithara::test(tokio)]
async fn decode_error_precedes_track_failure_on_event_bus() {
    let RebuildFixture { mut source, .. } = test_source(0).await;
    let bus = EventBus::new(16);
    let mut events = bus.subscribe();
    source.emit = Arc::new(DeferredBus::new(bus, 16));
    let replacement = DecoderGeneration::new(
        Box::new(FailingDecoder),
        Some(media_info(0)),
        0,
        None,
        None,
        GaplessMode::Disabled,
    );
    let old = source.decode.replace_active(replacement);
    drop(old);

    assert!(matches!(source.step_track(), TrackStep::Failed(_)));
    assert!(events.try_recv().is_err());
    source.finish_deferred();

    assert!(matches!(
        events.try_recv().map(|envelope| envelope.event),
        Ok(AudioLaneEvent::Decoder(DecoderEvent::DecodeError {
            detail: "fixture decode failure",
            ..
        }))
    ));
    assert!(matches!(
        events.try_recv().map(|envelope| envelope.event),
        Ok(AudioLaneEvent::Audio(AudioEvent::TrackFailed {
            failure: TrackFailureKind::Decode { kind: crate::DecodeErrorKind::InvalidData },
        }))
    ));
}

#[kithara::test(tokio)]
async fn route_change_host_rate_delta_starts_decoder_recreate(route_pcm: RoutePcm) {
    let RouteFixture { mut source, .. } =
        route_signal_source(&route_pcm, consts::SAMPLE_RATE).await;
    let origin = source.decode.active().base_offset();
    let position = source.playhead.position();
    source.set_host_sample_rate(NonZeroU32::new(48_000).expect("host rate"));
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(source.host_sample_rate().map(NonZeroU32::get), Some(48_000));
    assert_eq!(source.playhead.position(), position);
    assert_eq!(
        source.decode.active().decoder().spec().sample_rate.get(),
        48_000
    );
    assert!(source.decode.incoming_transition().is_none());
    assert_eq!(source.decode.active().base_offset(), origin);
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(0)
    );
}

#[kithara::test(tokio)]
async fn route_change_resumes_from_the_rendered_source_frontier(route_pcm: RoutePcm) {
    let RouteFixture { mut source, .. } =
        route_signal_source(&route_pcm, consts::SAMPLE_RATE).await;
    let mut route_recreated = false;
    let chunk = next_decoded_chunk(&mut source, &mut route_recreated);

    let rendered_frame =
        u64::try_from(consts::ROUTE_CHUNK_FRAMES / 2).expect("rendered fixture frame fits u64");
    let rendered = spec(consts::SAMPLE_RATE)
        .duration_for(rendered_frame)
        .expect("rendered fixture position fits Duration");

    assert_ne!(
        source.resume.decode_head(),
        Some((rendered_frame, consts::SAMPLE_RATE)),
        "the fixture must distinguish raw decode progress from rendered progress"
    );
    assert_eq!(
        chunk.meta.end_timestamp,
        spec(consts::SAMPLE_RATE)
            .duration_for(
                u64::try_from(consts::ROUTE_CHUNK_FRAMES).expect("route chunk frames fit u64"),
            )
            .expect("route chunk duration fits Duration")
    );
    source.commit_source_end(SourceEnd::new(
        rendered_frame,
        NonZeroU32::new(consts::SAMPLE_RATE).expect("test sample rate is non-zero"),
    ));

    source.set_host_sample_rate(NonZeroU32::new(consts::ROUTE_SAMPLE_RATE).expect("host rate"));
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    let chunk = next_decoded_chunk(&mut source, &mut route_recreated);
    assert_eq!(
        chunk.meta.timestamp, rendered,
        "route recreation resumes at rendered source progress"
    );
    let span = chunk
        .meta
        .source_span
        .expect("rebuilt chunk source mapping");
    assert_eq!(span.start(), rendered_frame);
    assert_eq!(span.sample_rate().get(), consts::SAMPLE_RATE);
    assert_eq!(span.output_frames(), u64::from(chunk.meta.frames));
    let next = next_decoded_chunk(&mut source, &mut route_recreated);
    let next_span = next.meta.source_span.expect("next chunk source mapping");
    assert_eq!(next.meta.timestamp, chunk.meta.end_timestamp);
    assert_eq!(
        next_span.source_ratio_at(0),
        span.source_ratio_at(span.output_frames())
    );
}

#[kithara::test(tokio)]
async fn route_change_recreate_preserves_position_and_output_rate_continuity_metric(
    route_pcm: RoutePcm,
) {
    let RouteFixture { mut source, .. } =
        route_signal_source(&route_pcm, consts::SAMPLE_RATE).await;
    let mut left = Vec::new();
    let mut route_recreated = false;

    for _ in 0..8 {
        let chunk = next_test_chunk(&mut source, &mut route_recreated);
        assert_eq!(chunk.meta.spec.sample_rate.get(), consts::SAMPLE_RATE);
        append_left_channel(&mut left, &chunk);
        source
            .playhead
            .advance(&crate::audio::chunk_position(&chunk.meta));
    }

    let route_frame = left.len();
    let route_position = source.playhead.position();
    source.set_host_sample_rate(NonZeroU32::new(consts::ROUTE_SAMPLE_RATE).expect("host rate"));

    let mut first_route_timestamp = None;
    let mut saw_new_rate = false;
    for _ in 0..8 {
        let chunk = next_test_chunk(&mut source, &mut route_recreated);
        if first_route_timestamp.is_none() {
            first_route_timestamp = Some(chunk.meta.timestamp);
        }
        saw_new_rate |= chunk.meta.spec.sample_rate.get() == consts::ROUTE_SAMPLE_RATE;
        append_left_channel(&mut left, &chunk);
        source
            .playhead
            .advance(&crate::audio::chunk_position(&chunk.meta));
    }

    assert!(
        route_recreated,
        "route change must enter recreate machinery"
    );
    assert!(
        saw_new_rate,
        "route-change output chunks must report the new host rate"
    );
    assert_eq!(
        source.decode.active().decoder().spec().sample_rate.get(),
        consts::ROUTE_SAMPLE_RATE
    );
    let first_route_timestamp =
        first_route_timestamp.expect("route change should produce post-route PCM");
    let drift_ns = first_route_timestamp.abs_diff(route_position).as_nanos();
    assert!(
        drift_ns <= 1_000_000,
        "route recreate drifted by {drift_ns} ns from {route_position:?} to {first_route_timestamp:?}",
    );

    let route_peak = peak_first_diff(&left, route_frame, 64);
    let control_peak = peak_first_diff(&left, consts::ROUTE_CHUNK_FRAMES * 4, 64);
    let ratio = route_peak / control_peak.max(f32::EPSILON);
    println!(
        "S_ROUTE_CONTINUITY route_peak={route_peak:.6} control_peak={control_peak:.6} ratio={ratio:.3}"
    );
    assert!(
        ratio < 2.0,
        "route-change discontinuity {route_peak:.6} is {ratio:.1}x the control boundary {control_peak:.6}",
    );
}

/// A route change swaps the resampler over the SAME container, so the
/// rebuilt demuxer has to be rooted where the live one is — the container
/// origin the running session was installed at. Deriving that origin from
/// the seek anchor instead hands an init-bearing demuxer a media byte; the
/// recreate then fails outright and takes the track with it.
#[kithara::test(tokio)]
async fn route_change_recreate_roots_the_demuxer_at_the_container_origin(route_pcm: RoutePcm) {
    let RouteFixture { mut source, .. } = route_source(
        &route_pcm,
        RouteParams {
            chunks_before_eof: None,
            gapless: None,
            incoming_chunks_before_eof: None,
            active_timeline_gap: 0,
            incoming_timeline_gap: 0,
            initial_host_rate: consts::SAMPLE_RATE,
            segmented: true,
        },
    )
    .await;

    let mut route_recreated = false;
    for _ in 0..4 {
        let chunk = next_test_chunk(&mut source, &mut route_recreated);
        source
            .playhead
            .advance(&crate::audio::chunk_position(&chunk.meta));
    }
    let resume_anchor = source
        .shared_stream
        .seek_time_anchor(source.playhead.position())
        .ok()
        .flatten()
        .expect("segmented source resolves an anchor for the resume position");
    assert_ne!(
        resume_anchor.byte_offset,
        source.decode.active().base_offset(),
        "fixture precondition: the resume anchor must be a media byte, not the container origin"
    );

    source.set_host_sample_rate(NonZeroU32::new(consts::ROUTE_SAMPLE_RATE).expect("host rate"));
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(
        source.host_sample_rate().map(NonZeroU32::get),
        Some(consts::ROUTE_SAMPLE_RATE)
    );
    assert_eq!(
        source.decode.active().base_offset(),
        0,
        "route change reuses container origin"
    );

    let mut saw_new_rate = false;
    for _ in 0..4 {
        let chunk = next_test_chunk(&mut source, &mut route_recreated);
        saw_new_rate |= chunk.meta.spec.sample_rate.get() == consts::ROUTE_SAMPLE_RATE;
        source
            .playhead
            .advance(&crate::audio::chunk_position(&chunk.meta));
    }
    assert!(
        saw_new_rate,
        "the rebuilt decoder must deliver the new host rate"
    );
}

#[kithara::test(tokio)]
async fn equal_host_rate_does_not_start_route_recreate() {
    let RebuildFixture {
        drops, mut source, ..
    } = test_source(0).await;
    source.set_host_sample_rate(NonZeroU32::new(consts::SAMPLE_RATE).expect("host rate"));
    assert!(drops.lock().is_empty());
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
}

#[kithara::test(tokio)]
async fn first_matching_host_rate_latches_without_route_recreate(route_pcm: RoutePcm) {
    let RouteFixture {
        drops, mut source, ..
    } = route_signal_source(&route_pcm, 0).await;
    source.set_host_sample_rate(NonZeroU32::new(consts::SAMPLE_RATE).expect("host rate"));
    assert!(drops.lock().is_empty());
    assert_eq!(
        source.host_sample_rate().map(NonZeroU32::get),
        Some(consts::SAMPLE_RATE)
    );
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
}

#[kithara::test(tokio)]
async fn first_mismatched_host_rate_still_starts_route_recreate(route_pcm: RoutePcm) {
    let RouteFixture {
        drops, mut source, ..
    } = route_signal_source(&route_pcm, 0).await;
    source.set_host_sample_rate(NonZeroU32::new(consts::ROUTE_SAMPLE_RATE).expect("host rate"));
    assert_eq!(drops.lock().as_slice(), &[1]);
    assert_eq!(
        source.decode.output_spec().sample_rate.get(),
        consts::ROUTE_SAMPLE_RATE
    );
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_seek_epoch_supersedes_completion() {
    let RebuildFixture {
        drops, mut source, ..
    } = test_source(1).await;
    install_test_factory(&mut source, 2, drops.clone());
    let target = Duration::from_secs(3);

    enter_rebuilding(&mut source, recreate_state(1));
    let outcome = source.seek(target).expect("owning-thread seek");
    assert!(matches!(outcome, crate::SeekOutcome::Landed { .. }));
    assert!(
        matches!(outcome, crate::SeekOutcome::Landed { target: actual, .. } if actual == target)
    );
    assert_eq!(source.playhead.position(), target);
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(1)
    );
    assert_eq!(drops.lock().as_slice(), &[1]);
    source.finish_deferred();
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
}

#[kithara::test(tokio)]
async fn deferred_preparation_does_not_read_before_seek_is_applied() {
    let RebuildFixture {
        mut source, drops, ..
    } = test_source(0).await;
    let decoder = TestDecoder::new(2, drops);
    let preparations = decoder.preparations.clone();
    drop(source.decode.replace_active(DecoderGeneration::new(
        Box::new(decoder),
        Some(media_info(0)),
        0,
        None,
        None,
        GaplessMode::Disabled,
    )));
    source.prepare_deferred();
    assert_eq!(preparations.swap(0, Ordering::Relaxed), 1);
    let target = Duration::from_secs(3);
    let outcome = source.seek(target).expect("synchronous seek");
    assert_eq!(preparations.load(Ordering::Relaxed), 0);
    assert!(
        matches!(outcome, crate::SeekOutcome::Landed { target: position, .. } if position == target)
    );
    assert_eq!(source.playhead.position(), target);
    assert_eq!(preparations.load(Ordering::Relaxed), 0);
}

#[kithara::test(tokio)]
async fn completed_seek_is_consumed_when_landing_bytes_are_no_longer_ready(route_pcm: RoutePcm) {
    let mut fixture = route_signal_source(&route_pcm, consts::SAMPLE_RATE).await;
    let target = Duration::from_millis(10);
    let outcome = fixture.source.seek(target).expect("synchronous landing");
    assert!(matches!(outcome, crate::SeekOutcome::Landed { .. }));
    *fixture.phase.lock() = SourcePhase::Waiting;
    assert_eq!(fixture.source.playhead.position(), target);
    assert!(matches!(
        fixture.source.phase,
        super::super::OwnerPhase::Decoding
    ));
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_variant_change_supersedes_completion() {
    let RebuildFixture {
        control,
        drops,
        mut source,
        ..
    } = test_source(1).await;
    install_test_factory(&mut source, 2, drops.clone());
    let target = Duration::from_secs(3);
    control.set_media_info(media_info(2));
    enter_rebuilding(&mut source, recreate_state(1));
    let outcome = source.seek(target).expect("owning-thread seek");
    assert!(matches!(outcome, crate::SeekOutcome::Landed { .. }));
    assert!(
        matches!(outcome, crate::SeekOutcome::Landed { target: actual, .. } if actual == target)
    );
    assert_eq!(source.playhead.position(), target);
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(2)
    );
    assert_eq!(drops.lock().as_slice(), &[1, 2]);
    source.finish_deferred();
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
}

#[kithara::test(tokio)]
async fn rebuilding_decoder_variant_change_preserves_inflight_seek() {
    let RebuildFixture {
        control,
        drops,
        mut source,
        ..
    } = test_source(1).await;
    install_test_factory(&mut source, 2, drops.clone());
    let target = Duration::from_secs(3);
    control.set_media_info(media_info(2));
    enter_rebuilding(&mut source, recreate_state(1));
    let outcome = source.seek(target).expect("owning-thread seek");
    assert!(matches!(outcome, crate::SeekOutcome::Landed { .. }));
    assert!(
        matches!(outcome, crate::SeekOutcome::Landed { target: actual, .. } if actual == target)
    );
    assert_eq!(source.playhead.position(), target);
    assert_eq!(
        source
            .decode
            .active()
            .media_info()
            .and_then(|info| info.variant_index),
        Some(2)
    );
    assert_eq!(drops.lock().as_slice(), &[1, 2]);
    source.finish_deferred();
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
}

#[kithara::test(tokio)]
async fn stale_rebuild_completion_retires_decoder_shell_side() {
    let RebuildFixture {
        drops, mut source, ..
    } = test_source(1).await;
    install_test_factory(&mut source, 2, drops.clone());

    let transition = exact_incoming_plan().transition();
    let stale = DecoderGeneration::new(
        Box::new(TestDecoder::new(3, drops.clone())),
        Some(media_info(1)),
        0,
        None,
        None,
        GaplessMode::Disabled,
    );
    assert!(drops.lock().is_empty());
    let rejected = source.decode.install_incoming(transition, stale);
    assert!(rejected.is_some());
    drop(rejected);
    assert_eq!(drops.lock().as_slice(), &[3]);
    assert!(matches!(source.phase, super::super::OwnerPhase::Decoding));
    assert_eq!(source.decode.incoming_transition(), None);
}

/// A decoder factory that panics during construction must not strand the
/// FSM in `RebuildingDecoder` forever. The rebuild port catches the panic,
/// pushes a `SoftFailed` completion, and wakes the worker.
#[kithara::test(tokio)]
async fn rebuild_factory_panic_fails_track_without_hang() {
    let RebuildFixture { mut source, .. } = test_source(1).await;
    source.factory = DecoderFactory::new(
        |_reader, _info, _rate| panic!("decoder construction blew up"),
        None,
    );
    source.set_host_sample_rate(NonZeroU32::new(consts::ROUTE_SAMPLE_RATE).expect("host rate"));
    assert!(matches!(
        source.phase,
        super::super::OwnerPhase::Failed {
            failure: TrackFailureKind::RecreateFailed { offset: 0 },
            error: Some(DecodeError::InvalidData { detail: "decoder factory panicked" })
        }
    ));
    assert!(matches!(
        source.step_track(),
        TrackStep::Failed(TrackFailureKind::RecreateFailed { offset: 0 })
    ));
    source.finish_deferred();
    assert!(matches!(
        source.phase,
        super::super::OwnerPhase::Failed { failure: TrackFailureKind::RecreateFailed { offset: 0 }, error: None }
    ));
}

#[kithara::test]
fn a_seek_releases_its_buffered_chunks_off_rt(route_pcm: RoutePcm) {
    const STAGED: usize = 3;
    let pools = pools();

    let mut generation = DecoderGeneration::new(
        Box::new(RouteSignalDecoder::new(
            &route_pcm,
            1,
            48_000,
            None,
            None,
            Arc::default(),
            pools,
        )),
        None,
        0,
        None,
        None,
        GaplessMode::Disabled,
    );
    for _ in 0..STAGED {
        let DecoderChunkOutcome::Chunk(chunk) = generation.next_chunk().expect("fixture chunk")
        else {
            panic!("the route-signal fixture produces chunks");
        };
        generation.stage(chunk);
    }
    assert!(generation.has_output(), "fixture staged nothing to flush");

    generation.notify_seek();

    assert!(!generation.has_output());
}
