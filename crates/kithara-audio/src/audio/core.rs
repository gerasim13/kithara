use super::{
    chunk_position,
    cursor::{ChunkCursor, ReadBuffer, source_spans_coalesce},
};
use crate::{
    AudioControl, AudioRead, AudioReadError, FailureSource, AudioSession, AudioSource, ChunkOutcome, Fetch,
    PendingReason, ReadOutcome, SeekOutcome, SourceEnd, SourceSpan, TrackStep,
};
use kithara_decode::TrackMetadata;
use kithara_events::EventBus;
use kithara_platform::{CancelToken, sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioSpec};
use kithara_stream::{Activity, ActivityWriter, PlayheadWrite};
use std::{
    marker::PhantomData,
    num::{NonZeroU32, NonZeroUsize},
};

/// Open decoded source owned and driven by one lane thread.
pub struct Audio<S> {
    source: Box<dyn AudioSource<Chunk = AudioChunk>>,
    playhead: Arc<dyn PlayheadWrite>,
    bus: EventBus,
    metadata: TrackMetadata,
    abr: Option<kithara_abr::AbrHandle>,
    activity: Activity,
    activity_writer: Option<ActivityWriter>,
    cancel: CancelToken,
    cursor: ChunkCursor,
    current_chunk: Option<AudioChunk>,
    preloaded: bool,
    failure: Option<crate::TrackFailureKind>,
    marker: PhantomData<fn() -> S>,
}

impl<S> Audio<S> {
    pub(super) fn new(
        source: Box<dyn AudioSource<Chunk = AudioChunk>>,
        playhead: Arc<dyn PlayheadWrite>,
        bus: EventBus,
        metadata: TrackMetadata,
        abr: Option<kithara_abr::AbrHandle>,
        activity: Activity,
        activity_writer: Option<ActivityWriter>,
        cancel: CancelToken,
        spec: AudioSpec,
    ) -> Self {
        Self {
            source,
            playhead,
            bus,
            metadata,
            abr,
            activity,
            activity_writer,
            cancel,
            cursor: ChunkCursor::new(spec),
            current_chunk: None,
            preloaded: false,
            failure: None,
            marker: PhantomData,
        }
    }
    /// Transfer the sole loader-priority writer to the owning lane.
    pub fn take_activity_writer(&mut self) -> Option<ActivityWriter> {
        self.activity_writer.take()
    }
    /// Read-only loader-priority snapshot.
    #[must_use]
    pub fn activity(&self) -> Activity {
        self.activity.clone()
    }
    /// Adaptive bitrate control for this open source.
    #[must_use]
    pub fn abr_handle(&self) -> Option<kithara_abr::AbrHandle> {
        self.abr.clone()
    }
    /// Currently selected adaptive variant.
    #[must_use]
    pub fn current_variant(&self) -> Option<kithara_abr::VariantInfo> {
        self.abr.as_ref()?.current_variant()
    }
    /// Whether initial input preparation was requested.
    #[must_use]
    pub const fn is_preloaded(&self) -> bool {
        self.preloaded
    }
    /// Track metadata captured at open.
    #[must_use]
    pub const fn metadata(&self) -> &TrackMetadata {
        &self.metadata
    }
    /// Current decoded output format.
    #[must_use]
    pub fn spec(&self) -> AudioSpec {
        self.cursor.spec()
    }
    /// Current committed source position.
    #[must_use]
    pub fn position(&self) -> Duration {
        self.playhead.position()
    }
    /// Total content duration, when known.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.playhead.duration()
    }
    /// Prepare one initial chunk without moving work to another thread.
    ///
    /// # Errors
    /// Returns a source or decoder failure.
    pub fn preload(&mut self) -> Result<(), AudioReadError> {
        self.preloaded = true;
        if self.current_chunk.is_none() {
            if let ChunkOutcome::Chunk(chunk) = self.pull_chunk()? {
                self.cursor.begin_chunk(&chunk);
                self.current_chunk = Some(chunk);
            }
        }
        Ok(())
    }
    /// Seek synchronously inside the open source, discarding locally buffered PCM.
    ///
    /// # Errors
    /// Returns a source or decoder failure without rebuilding for a seek failure.
    pub fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError> {
        if let Some(failure) = self.failure {
            return Err(AudioReadError::Stream {
                what: "seek decoded source",
                source: FailureSource::ProducerAfterSeek { failure },
            });
        }
        self.current_chunk = None;
        self.cursor.clear();
        let result = self.source.seek(position);
        if let Err(error) = &result {
            self.failure = Some(crate::TrackFailureKind::from(error));
        }
        if let Some(spec) = self.source.prepare_deferred() {
            self.cursor.set_spec(spec);
        }
        self.source.finish_deferred();
        result
    }
    /// Rebuild the decoder at a host-rate change on the owning thread.
    /// A rebuild error becomes the source's terminal read failure.
    pub fn set_host_sample_rate(&mut self, rate: NonZeroU32) {
        if self.source.host_sample_rate() == Some(rate) {
            return;
        }
        if let Some(chunk) = &self.current_chunk {
            self.source.commit_source_end(SourceEnd::new(
                chunk
                    .meta
                    .frame_offset
                    .saturating_add(self.cursor.consumed_frames()),
                chunk.spec().sample_rate,
            ));
        }
        self.current_chunk = None;
        self.cursor.clear();
        self.source.set_host_sample_rate(rate);
        if let Some(spec) = self.source.prepare_deferred() {
            self.cursor.set_spec(spec);
        }
        self.source.finish_deferred();
    }
    fn pull_chunk(&mut self) -> Result<ChunkOutcome, AudioReadError> {
        if let Some(failure) = self.failure {
            return Err(AudioReadError::Stream {
                what: "read decoded source",
                source: FailureSource::Producer { failure },
            });
        }
        if let Some(spec) = self.source.prepare_deferred() {
            self.cursor.set_spec(spec);
        }
        let step = self.source.step_track();
        self.source.finish_deferred();
        match step {
            TrackStep::Produced(Fetch::Data { data, .. }) => Ok(ChunkOutcome::Chunk(data)),
            TrackStep::Produced(Fetch::NaturalEof) | TrackStep::Eof => Ok(ChunkOutcome::Eof {
                position: self.position(),
            }),
            TrackStep::Produced(Fetch::Failure { failure }) | TrackStep::Failed(failure) => {
                self.failure = Some(failure);
                Err(AudioReadError::Stream {
                    what: "read decoded source",
                    source: FailureSource::Producer { failure },
                })
            }
            TrackStep::Blocked(_) | TrackStep::StateChanged => Ok(ChunkOutcome::Pending {
                reason: PendingReason::StreamBackpressure,
                position: self.position(),
            }),
        }
    }
    /// Pull a chunk on the owning thread.
    ///
    /// # Errors
    /// Returns a source or decoder failure.
    pub fn next_chunk(&mut self) -> Result<ChunkOutcome, AudioReadError> {
        let chunk = if let Some(chunk) = self.current_chunk.take() {
            let mut consumed = self.cursor.consumed_frames();
            crate::pipeline::seek::skip::apply_frames(chunk, &mut consumed)
        } else {
            None
        };
        let outcome = match chunk {
            Some(chunk) => ChunkOutcome::Chunk(chunk),
            None => self.pull_chunk()?,
        };
        if let ChunkOutcome::Chunk(chunk) = &outcome {
            self.cursor.begin_chunk(chunk);
            self.playhead.advance(&chunk_position(&chunk.meta));
            self.source.commit_source_end(SourceEnd::new(
                chunk
                    .meta
                    .frame_offset
                    .saturating_add(u64::from(chunk.meta.frames)),
                chunk.spec().sample_rate,
            ));
        }
        Ok(outcome)
    }
    /// Copy interleaved samples from the open source.
    ///
    /// # Errors
    /// Returns invalid buffer geometry or a source failure.
    pub fn read(&mut self, output: &mut [f32]) -> Result<ReadOutcome, AudioReadError> {
        self.read_into(ReadBuffer::Interleaved(output))
    }
    /// Copy samples into equal-length channel planes.
    ///
    /// # Errors
    /// Returns invalid plane geometry or a source failure.
    pub fn read_planar<'a>(
        &mut self,
        output: &'a mut [&'a mut [f32]],
    ) -> Result<ReadOutcome, AudioReadError> {
        self.read_into(ReadBuffer::Planar(output))
    }
    fn read_into(&mut self, mut output: ReadBuffer<'_, '_>) -> Result<ReadOutcome, AudioReadError> {
        let capacity = output.capacity()?;
        let mut written = 0usize;
        let mut source_span: Option<SourceSpan> = None;
        let mut output_frames = 0u64;
        let mut eof = false;
        while written < capacity {
            if self.current_chunk.is_none() {
                match self.pull_chunk() {
                    Err(_) if written > 0 => break,
                    Err(error) => return Err(error),
                    Ok(outcome) => match outcome {
                    ChunkOutcome::Chunk(chunk) => {
                        self.cursor.begin_chunk(&chunk);
                        self.current_chunk = Some(chunk);
                    }
                    ChunkOutcome::Eof { .. } => {
                        eof = true;
                        break;
                    }
                    ChunkOutcome::Pending { .. } => break,
                    },
                }
            }
            let Some(chunk) = self.current_chunk.as_ref() else {
                break;
            };
            let span = SourceSpan::new(
                chunk.meta.frame_offset,
                chunk
                    .meta
                    .frame_offset
                    .saturating_add(u64::from(chunk.meta.frames)),
                chunk.spec().sample_rate,
                u64::from(chunk.meta.frames),
            );
            if written > 0
                && !source_spans_coalesce(
                    source_span,
                    output_frames,
                    span,
                    u64::from(chunk.meta.frames),
                )
            {
                break;
            }
            let copied =
                self.cursor
                    .copy_into(chunk, span, &mut output, written, self.playhead.as_ref())?;
            self.source.commit_source_end(SourceEnd::new(
                chunk
                    .meta
                    .frame_offset
                    .saturating_add(self.cursor.consumed_frames()),
                chunk.spec().sample_rate,
            ));
            written += copied.count;
            output_frames = output_frames.saturating_add(copied.output_frames);
            source_span = match (source_span, copied.source_span) {
                (Some(previous), Some(next)) => previous.followed_by(next),
                (None, span) => span,
                _ => None,
            };
            if copied.finished {
                self.current_chunk = None;
            } else if copied.count == 0 {
                break;
            }
        }
        if let Some(count) = NonZeroUsize::new(written) {
            return Ok(ReadOutcome::Frames {
                count,
                position: self.position(),
                source_span,
            });
        }
        if eof {
            Ok(ReadOutcome::Eof {
                position: self.position(),
            })
        } else {
            Ok(ReadOutcome::Pending {
                reason: PendingReason::StreamBackpressure,
                position: self.position(),
            })
        }
    }
}

impl<S> Drop for Audio<S> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl<S> AudioRead for Audio<S> {
    fn decoded_frontier(&self) -> Duration {
        self.current_chunk
            .as_ref()
            .map_or_else(|| self.position(), |chunk| chunk.meta.end_timestamp)
    }
    fn spec(&self) -> AudioSpec {
        self.spec()
    }
    fn next_chunk(&mut self) -> Result<ChunkOutcome, AudioReadError> {
        self.next_chunk()
    }
    fn position(&self) -> Duration {
        self.position()
    }
    fn read(&mut self, output: &mut [f32]) -> Result<ReadOutcome, AudioReadError> {
        self.read(output)
    }
    fn read_planar<'a>(
        &mut self,
        output: &'a mut [&'a mut [f32]],
    ) -> Result<ReadOutcome, AudioReadError> {
        self.read_planar(output)
    }
}
impl<S> AudioSession for Audio<S> {
    fn abr_handle(&self) -> Option<kithara_abr::AbrHandle> {
        self.abr_handle()
    }
    fn duration(&self) -> Option<Duration> {
        self.duration()
    }
    fn event_bus(&self) -> &EventBus {
        &self.bus
    }
    fn is_preloaded(&self) -> bool {
        self.is_preloaded()
    }
    fn metadata(&self) -> &TrackMetadata {
        self.metadata()
    }
}
impl<S> AudioControl for Audio<S> {
    fn preload(&mut self) -> Result<(), AudioReadError> {
        self.preload()
    }
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError> {
        self.seek(position)
    }
    fn set_host_sample_rate(&mut self, rate: NonZeroU32) {
        self.set_host_sample_rate(rate);
    }
}
impl<S: 'static> AudioSource for Audio<S> {
    type Chunk = AudioChunk;
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError> {
        self.seek(position)
    }
    fn set_host_sample_rate(&mut self, rate: NonZeroU32) {
        self.set_host_sample_rate(rate);
    }
    fn host_sample_rate(&self) -> Option<NonZeroU32> {
        self.source.host_sample_rate()
    }
    fn commit_source_end(&mut self, end: SourceEnd) {
        self.source.commit_source_end(end);
    }
    fn discontinuity(&self) -> Option<crate::SourceDiscontinuity> {
        self.source.discontinuity()
    }
    fn finish_deferred(&mut self) {
        self.source.finish_deferred();
    }
    fn prepare_deferred(&mut self) -> Option<AudioSpec> {
        self.source.prepare_deferred()
    }
    fn step_track(&mut self) -> TrackStep<AudioChunk> {
        if let Some(chunk) = self.current_chunk.take() {
            let mut consumed = self.cursor.consumed_frames();
            if let Some(chunk) = crate::pipeline::seek::skip::apply_frames(chunk, &mut consumed) {
                let end = SourceEnd::new(
                    chunk
                        .meta
                        .frame_offset
                        .saturating_add(u64::from(chunk.meta.frames)),
                    chunk.spec().sample_rate,
                );
                self.cursor.begin_chunk(&chunk);
                return TrackStep::Produced(Fetch::rendered(chunk, end));
            }
        }
        self.source.step_track()
    }
    fn warm_up(&mut self) {
        self.source.warm_up();
    }
}
#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroU32,
        sync::atomic::{AtomicU32, AtomicU64},
    };

    use kithara_events::EventReceiver;
    use kithara_platform::{CancelScope, sync::Arc, tokio::sync::broadcast::error::TryRecvError};
    use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
    use kithara_stream::{PlayheadState, SeekState, mock::NoopWorkerWake};
    use kithara_test_fixtures::unit_fixtures::trim_silence;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::{
        AudioEvent, ConsumerWakeMode,
        audio::{Fetch, Outlet, ThreadWake, connect, ring::RingParts},
        test_pools::pools,
    };

    struct AudioFixture {
        emit: Arc<kithara_events::DeferredBus<AudioLaneEvent>>,
        audio: Audio<()>,
        data_tx: Outlet<Fetch<AudioChunk>>,
    }

    impl Default for AudioFixture {
        fn default() -> Self {
            Self::with_wake_mode(ConsumerWakeMode::RealtimeDeferred, false)
        }
    }

    impl AudioFixture {
        fn with_wake_mode(consumer_wake_mode: ConsumerWakeMode, block_on_underrun: bool) -> Self {
            let (data_tx, data_rx) = connect::<Fetch<AudioChunk>>(1, None);
            let (trash_tx, _trash_rx) = connect::<AudioChunk>(8, None);
            let epoch = Arc::new(AtomicU64::new(0));
            let ring = RingConsumer::new(RingParts {
                trash_tx,
                epoch,
                consumer_wake_mode,
                block_on_underrun,
                audio_rx: data_rx,
                reader_wake: Arc::new(ThreadWake::default()),
            });
            let seek_state = Arc::new(SeekState::new());
            let seek: Arc<dyn SeekControl> = seek_state.clone();
            let seek_obs: Arc<dyn SeekObserve> = seek_state;
            let playhead: Arc<dyn PlayheadWrite> = Arc::new(PlayheadState::new());
            let cursor = ChunkCursor::new(AudioChunkInfo::default().spec);
            let bus = EventBus::default();
            let emit = AudioEvents::deferred(&bus);
            Self {
                audio: Audio::from(AudioParts {
                    ring,
                    cursor,
                    emit: Arc::clone(&emit),
                    runtime: AudioRuntime {
                        cancel: CancelScope::new(None).token(),
                        wake: Arc::new(NoopWorkerWake),
                    },
                    session: Session {
                        playhead,
                        seek,
                        seek_obs,
                        preload_gate: Arc::new(PreloadGate::default()),
                        metadata: TrackMetadata::default(),
                        abr_handle: None,
                        peer_wake: None,
                        seek_prepare: None,
                    },
                    controls: Controls {
                        host_sample_rate: Arc::new(AtomicU32::new(0)),
                    },
                    marker: PhantomData,
                }),
                data_tx,
                emit,
            }
        }
    }

    #[kithara::test]
    fn seek_rearms_preload_gate_before_worker_refill() {
        let mut fixture = AudioFixture::default();
        fixture.audio.session.preload_gate.signal_epoch(0);
        assert!(fixture.audio.session.preload_gate.is_ready());
        fixture
            .audio
            .seek(Duration::from_millis(250))
            .expect("seek should arm epoch");
        assert!(!fixture.audio.session.preload_gate.is_ready());
    }

    /// The preload latch opens on an upstream park too, so construction must
    /// come back from a ring the producer has not filled. One second instead of
    /// the ambient ten: the regression is a park, and on the flash-off lane that
    /// park is spent in real time.
    #[cfg(not(target_arch = "wasm32"))]
    #[kithara::test(hang_timeout_secs(1))]
    fn preload_returns_when_the_producer_has_delivered_nothing() {
        let mut fixture = AudioFixture::with_wake_mode(ConsumerWakeMode::RealtimeDeferred, true);

        fixture
            .audio
            .preload()
            .expect("preload primes whatever the producer delivered");

        assert!(fixture.audio.is_preloaded());
    }

    fn staged_chunk(trim_silence: &[f32]) -> AudioChunk {
        let mut samples = pools()
            .get_with_len::<f32>(8)
            .expect("staged samples fit test pools");
        samples.copy_from_slice(&trim_silence[..8]);
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test rate is non-zero"));
        let frames = u32::try_from(samples.len() / usize::from(spec.channels))
            .expect("fixture frame count fits u32");
        AudioChunk::new(
            AudioChunkInfo {
                spec,
                frames,
                ..AudioChunkInfo::default()
            },
            samples,
        )
    }

    fn drain_seek_completions(receiver: &mut EventReceiver<AudioEvent>) -> Vec<u64> {
        let mut completions = Vec::new();
        loop {
            match receiver.try_recv() {
                Ok(envelope) => {
                    if let AudioEvent::SeekComplete { seek_epoch, .. } = envelope.event {
                        completions.push(seek_epoch);
                    }
                }
                Err(TryRecvError::Empty) => return completions,
                Err(error) => panic!("event receiver failed: {error:?}"),
            }
        }
    }

    /// Begin a seek epoch and stage one chunk at that epoch, so the next read
    /// returns `Frames` and births `SeekComplete` inside `commit_read`.
    fn seek_and_stage(trim_silence: &[f32], fixture: &mut AudioFixture) -> u64 {
        fixture.audio.ring.preloaded = true;
        fixture
            .audio
            .seek(Duration::from_millis(250))
            .expect("seek begins an epoch");
        let epoch = fixture.audio.ring.validator.epoch;
        fixture
            .data_tx
            .try_push(Fetch::data(staged_chunk(trim_silence), epoch))
            .expect("staged chunk reaches the ring");
        epoch
    }

    #[kithara::test]
    fn off_rt_read_publishes_the_seek_completion_it_births(trim_silence: Vec<f32>) {
        let mut fixture = AudioFixture::with_wake_mode(ConsumerWakeMode::ImmediateOffRt, false);
        let mut receiver = fixture.audio.events.bus().subscribe();
        let epoch = seek_and_stage(&trim_silence, &mut fixture);

        let mut buf = [0.0f32; 8];
        let outcome = fixture.audio.read(&mut buf).expect("staged read");

        assert!(matches!(outcome, ReadOutcome::Frames { .. }));
        assert_eq!(
            drain_seek_completions(&mut receiver),
            vec![epoch],
            "an ImmediateOffRt consumer runs off the real-time thread, so the SeekComplete born inside its read is on the bus when the read returns"
        );
    }

    #[kithara::test]
    fn an_adopted_realtime_mode_moves_the_reader_events_with_the_ring(trim_silence: Vec<f32>) {
        let mut fixture = AudioFixture::with_wake_mode(ConsumerWakeMode::ImmediateOffRt, false);
        let mut receiver = fixture.audio.events.bus().subscribe();
        AudioControl::set_consumer_wake_mode(
            &mut fixture.audio,
            ConsumerWakeMode::RealtimeDeferred,
        );
        let epoch = seek_and_stage(&trim_silence, &mut fixture);

        let mut buf = [0.0f32; 8];
        fixture.audio.read(&mut buf).expect("staged read");

        assert_eq!(
            drain_seek_completions(&mut receiver),
            Vec::<u64>::new(),
            "a reader that adopted RealtimeDeferred reads on the audio callback, so it defers what its read births"
        );

        fixture.emit.flush();
        assert_eq!(drain_seek_completions(&mut receiver), vec![epoch]);
    }

    #[kithara::test]
    fn realtime_read_leaves_its_seek_completion_for_the_shell(trim_silence: Vec<f32>) {
        let mut fixture = AudioFixture::default();
        let mut receiver = fixture.audio.events.bus().subscribe();
        let epoch = seek_and_stage(&trim_silence, &mut fixture);

        let mut buf = [0.0f32; 8];
        let outcome = fixture.audio.read(&mut buf).expect("staged read");

        assert!(matches!(outcome, ReadOutcome::Frames { .. }));
        assert_eq!(
            drain_seek_completions(&mut receiver),
            Vec::<u64>::new(),
            "a RealtimeDeferred read runs on the audio callback, so its events wait for the scheduler shell"
        );

        fixture.emit.flush();
        assert_eq!(
            drain_seek_completions(&mut receiver),
            vec![epoch],
            "the shell flush delivers what the read deferred"
        );
    }
}
