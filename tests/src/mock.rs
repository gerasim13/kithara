use std::{marker::PhantomData, num::{NonZeroU32, NonZeroUsize}, task::{Context, Waker}};

use kithara::{
    audio::{Audio, AudioConfig, AudioControl, AudioRead, AudioReadError, AudioSession, ChunkOutcome, FailureSource, PendingReason, ReadOutcome, ResamplerBackend, SeekOutcome, TrackFailureKind},
    bufpool::HasPool,
    decode::{DecodeError, TrackMetadata},
    events::{EventBus, EventReceiver, EventSet},
    platform::time::Duration,
    play::{LoadRefusal, PlayWorker, TrackConfig},
    signal::{AudioChunk, AudioSpec, SegmentId},
    stream::{Stream, StreamType, WorkerWake},
    warp::SpeedCurve,
};
use kithara_command::{Batch, Outcome, Sender, When};
use kithara_render::{LaneCommand, LaneProtocol, LaneStart, LaneTask, PcmPacket, PcmReceiver};

pub const READ_PENDING_POLL: Duration = Duration::from_millis(1);
pub const PRELOAD_READY_RETRIES: usize = 4096;

#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
mod ramped;
#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
pub use ramped::{InjectedFactory, RampedFactory};

#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
pub async fn open_resource(
    config: kithara::play::ResourceConfig<crate::bufpool_ext::TestPools>,
) -> Result<kithara::play::Resource, LoadRefusal> {
    let (worker, track) = kithara::play::mock::resource_tracks(config)?;
    let resource = match track {
        futures::future::Either::Left(track) => {
            kithara::play::Resource::from_reader(load_audio(&worker, track).await?, None)
        }
        futures::future::Either::Right(track) => {
            kithara::play::Resource::from_reader(load_audio(&worker, track).await?, None)
        }
    };
    Ok(resource)
}

struct SourceWake<S>(PlayWorker<S>);

impl<S: Send + Sync + 'static> WorkerWake for SourceWake<S> {
    fn defer(&self) {
        self.0.wake();
    }

    fn wake(&self) {
        self.0.wake();
    }
}

pub async fn load_source_audio<T, B, S>(
    worker: &PlayWorker<S>,
    config: AudioConfig<T, B>,
) -> Result<Audio<Stream<T>>, DecodeError>
where
    T: StreamType<Events = EventBus>,
    B: Default + ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    Audio::prepare(
        config,
        kithara::platform::sync::Arc::new(SourceWake(worker.clone())),
        worker.pools().clone(),
    )
    .await
}

#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
pub struct PcmDeck {
    _directory: kithara_test_utils::TestTempDir,
    source: kithara::queue::TrackSource<crate::bufpool_ext::TestPools>,
}

#[cfg(all(feature = "all", not(target_arch = "wasm32")))]
impl PcmDeck {
    pub fn new(mut reader: Box<dyn kithara::audio::AudioReader>) -> Self {
        use std::io::{Seek, SeekFrom, Write};

        let directory = kithara_test_utils::TestTempDir::new();
        let path = directory.path().join("deck.wav");
        let mut file = std::io::BufWriter::new(
            std::fs::File::create(&path).expect("create PCM deck WAV"),
        );
        let spec = reader.spec();
        let block_align = spec.channels.checked_mul(4).expect("WAV block alignment");
        let byte_rate = spec.sample_rate.get().checked_mul(u32::from(block_align))
            .expect("WAV byte rate");
        file.write_all(b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x03\0")
            .expect("write float WAV header");
        file.write_all(&spec.channels.to_le_bytes()).expect("write WAV channels");
        file.write_all(&spec.sample_rate.get().to_le_bytes()).expect("write WAV rate");
        file.write_all(&byte_rate.to_le_bytes()).expect("write WAV byte rate");
        file.write_all(&block_align.to_le_bytes()).expect("write WAV block alignment");
        file.write_all(&32u16.to_le_bytes()).expect("write WAV bit depth");
        file.write_all(b"data\0\0\0\0").expect("write WAV data header");
        let mut samples = vec![0.0; usize::from(spec.channels) * 4096];
        let mut bytes = 0u32;
        loop {
            match reader.read(&mut samples).expect("read PCM deck samples") {
                ReadOutcome::Frames { count, .. } => {
                    for sample in &samples[..count.get()] {
                        file.write_all(&sample.to_le_bytes()).expect("write PCM sample");
                    }
                    let written = u32::try_from(count.get()).expect("WAV sample count")
                        .checked_mul(4).expect("WAV sample bytes");
                    bytes = bytes.checked_add(written).expect("PCM deck fits RIFF");
                }
                ReadOutcome::Pending { .. } => std::thread::yield_now(),
                ReadOutcome::Eof { .. } => break,
            }
        }
        file.seek(SeekFrom::Start(4)).expect("seek RIFF size");
        file.write_all(&bytes.checked_add(36).expect("RIFF length").to_le_bytes())
            .expect("write RIFF size");
        file.seek(SeekFrom::Start(40)).expect("seek WAV data size");
        file.write_all(&bytes.to_le_bytes()).expect("write WAV data size");
        file.flush().expect("flush PCM deck WAV");
        Self {
            _directory: directory,
            source: kithara::queue::TrackSource::Uri(
                path.to_str().expect("UTF-8 test WAV path").to_owned(),
            ),
        }
    }

    pub fn source(&self) -> kithara::queue::TrackSource<crate::bufpool_ext::TestPools> {
        self.source.clone()
    }
}

#[cfg(all(test, feature = "all", not(target_arch = "wasm32")))]
mod tests {
    use ::kithara::audio::mock::TestPcmReader;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn pcm_deck_preserves_rate_samples_and_file_lifetime() {
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test rate"));
        let samples = vec![0.25f32, -0.5, 1.0, -1.0];
        let deck = PcmDeck::new(Box::new(TestPcmReader::with_samples(spec, samples.clone())));
        let path = deck._directory.path().join("deck.wav");
        let bytes = std::fs::read(&path).expect("read PCM deck WAV");
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[20..22], &3u16.to_le_bytes());
        assert_eq!(&bytes[22..24], &2u16.to_le_bytes());
        assert_eq!(&bytes[24..28], &48_000u32.to_le_bytes());
        assert_eq!(&bytes[34..36], &32u16.to_le_bytes());
        assert_eq!(&bytes[40..44], &16u32.to_le_bytes());
        assert_eq!(bytes.len(), 60);
        for (encoded, sample) in bytes[44..].chunks_exact(4).zip(samples) {
            assert_eq!(encoded, &sample.to_le_bytes());
        }
        assert!(path.exists());
        drop(deck);
        assert!(!path.exists());
    }
}

pub struct LaneAudio<T, S> {
    worker: PlayWorker<S>,
    #[cfg(not(target_arch = "wasm32"))]
    lane: Box<dyn LaneTask + Send>,
    #[cfg(target_arch = "wasm32")]
    lane: Box<dyn LaneTask>,
    sender: Sender<LaneProtocol>,
    pcm: PcmReceiver,
    bus: EventBus,
    segment: SegmentId,
    speed: SpeedCurve,
    ready: Option<SegmentId>,
    committed_segment: Option<SegmentId>,
    chunk: Option<AudioChunk>,
    offset: usize,
    failure: Option<TrackFailureKind>,
    eof: bool,
    marker: PhantomData<fn() -> T>,
}

pub async fn load_audio<T, B, S, C>(worker: &PlayWorker<S>, config: C) -> Result<LaneAudio<Stream<T>, S>, LoadRefusal>
where
    T: StreamType<Events = EventBus>,
    B: Default + ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    C: Into<TrackConfig<T, B>>,
{
    let config = config.into();
    let bus = T::event_bus(config.audio().stream())
        .or_else(|| config.audio().bus().cloned())
        .unwrap_or_default();
    let start = LaneStart {
        speed: SpeedCurve::Constant(config.warp().speed()),
        keylock: config.warp().keylock(),
        backend: config.warp().backend(),
    };
    let speed = start.speed.clone();
    let (sender, inbox) = worker.lane_channel();
    let (pcm, lane, _) = worker.load(config, Duration::ZERO, start, inbox).await?;
    Ok(LaneAudio {
        worker: worker.clone(),
        lane: Box::new(lane),
        sender,
        pcm,
        bus,
        segment: SegmentId::FIRST,
        speed,
        ready: Some(SegmentId::FIRST),
        committed_segment: None,
        chunk: None,
        offset: 0,
        failure: None,
        eof: false,
        marker: PhantomData,
    })
}

#[kithara_test_utils::kithara::flash(true)]
pub async fn wait_for_preload<T, S>(audio: &mut LaneAudio<T, S>, label: &str) {
    let segment = audio.segment();
    let mut retries = 0usize;
    while !audio.current_segment_ready() {
        retries += 1;
        assert!(
            retries < PRELOAD_READY_RETRIES,
            "{label}: preload segment {segment:?} never became ready within {PRELOAD_READY_RETRIES} polls",
        );
        kithara::platform::time::sleep(READ_PENDING_POLL).await;
    }
}

impl<T, S> LaneAudio<T, S> {
    fn service(&mut self) {
        let mut context = Context::from_waker(Waker::noop());
        let _ = self.lane.poll_commands(&mut context);
        while self.pcm.peek().is_some_and(|packet| match packet {
            PcmPacket::Chunk(chunk) => chunk.meta.segment != self.segment,
            PcmPacket::Failed { segment, .. } => *segment != self.segment,
        }) {
            let packet = self.pcm.pop().expect("peeked stale packet");
            self.pcm.recycle(packet).expect("recycle stale packet");
            self.lane.recycle();
        }
        self.lane.recycle();
        let _ = self.lane.tick();
        for receipt in self.sender.receipts() {
            if let Outcome::Applied { data, .. } = receipt.outcome() {
                if data.ready == Some(self.segment) {
                    self.ready = data.ready;
                }
            } else {
                panic!("lane command rejected: {:?}", receipt.outcome());
            }
        }
    }

    pub fn current_segment_ready(&mut self) -> bool {
        self.service();
        self.ready == Some(self.segment)
    }

    pub fn segment(&self) -> SegmentId {
        self.segment
    }

    pub fn committed_segment(&self) -> Option<SegmentId> {
        self.committed_segment
    }

    pub fn events<E: EventSet>(&self) -> EventReceiver<E> {
        self.bus.subscribe()
    }

    fn send(&mut self, command: LaneCommand) -> Result<(), AudioReadError> {
        self.sender.send(When::Next, Batch { basis: Vec::new(), commands: vec![command] })
            .map_err(|error| DecodeError::audio_stream("test lane command", format!("{error:?}")))?;
        self.worker.wake();
        Ok(())
    }

    fn pull(&mut self) -> Result<ChunkOutcome, AudioReadError> {
        self.service();
        if let Some(failure) = self.failure {
            return Err(AudioReadError::Stream { what: "read test lane", source: FailureSource::Producer { failure } });
        }
        if self.eof {
            return Ok(ChunkOutcome::Eof { position: self.pcm.position() });
        }
        while let Some(packet) = self.pcm.pop() {
            match packet {
                PcmPacket::Chunk(chunk) if chunk.meta.segment == self.segment => {
                    if chunk.meta.end_of_track {
                        self.pcm.set_position(chunk.meta.timestamp);
                        self.pcm.recycle(PcmPacket::Chunk(chunk)).expect("recycle EOF packet");
                        self.eof = true;
                        return Ok(ChunkOutcome::Eof { position: self.pcm.position() });
                    }
                    return Ok(ChunkOutcome::Chunk(Box::new(chunk)));
                }
                PcmPacket::Failed { segment, failure } if segment == self.segment => {
                    self.failure = Some(failure);
                    return Err(AudioReadError::Stream { what: "read test lane", source: FailureSource::Producer { failure } });
                }
                packet => self.pcm.recycle(packet).expect("return stale packet"),
            }
        }
        Ok(ChunkOutcome::Pending { reason: PendingReason::Buffering, position: self.pcm.position() })
    }
}

impl<T, S> AudioRead for LaneAudio<T, S> {
    fn spec(&self) -> AudioSpec { self.pcm.spec() }
    fn position(&self) -> Duration { self.pcm.position() }
    fn decoded_frontier(&self) -> Duration { self.pcm.decoded_frontier() }
    fn cached_span(&self) -> Duration { self.pcm.cached_span() }

    fn next_chunk(&mut self) -> Result<ChunkOutcome, AudioReadError> {
        if let Some(mut chunk) = self.chunk.take() {
            let channels = usize::from(chunk.meta.spec.channels);
            let skipped = self.offset / channels;
            chunk.samples.drain(..self.offset);
            chunk.meta.timestamp = chunk.meta.source_span.and_then(|span| span.position_at(skipped as u64))
                .unwrap_or_else(|| chunk.meta.timestamp + chunk.meta.spec.duration_for(skipped as u64).expect("packet offset duration"));
            chunk.meta.source_span = chunk.meta.source_span.and_then(|span| span.for_output_range(skipped as u64..u64::from(chunk.meta.frames)));
            chunk.meta.frame_offset += skipped as u64;
            chunk.meta.lane_frame += skipped as u64;
            chunk.meta.frames -= skipped as u32;
            self.offset = 0;
            self.committed_segment = Some(chunk.meta.segment);
            self.pcm.set_position(chunk.meta.source_span.and_then(|span| span.position_at(u64::from(chunk.meta.frames)))
                .unwrap_or_else(|| chunk.meta.timestamp + chunk.meta.spec.duration_for(u64::from(chunk.meta.frames)).expect("packet duration")));
            return Ok(ChunkOutcome::Chunk(Box::new(chunk)));
        }
        let outcome = self.pull()?;
        if let ChunkOutcome::Chunk(chunk) = &outcome {
            self.committed_segment = Some(chunk.meta.segment);
            self.pcm.set_position(chunk.meta.source_span.and_then(|span| span.position_at(u64::from(chunk.meta.frames)))
                .unwrap_or_else(|| chunk.meta.timestamp + chunk.meta.spec.duration_for(u64::from(chunk.meta.frames)).expect("packet duration")));
        }
        Ok(outcome)
    }

    fn read(&mut self, output: &mut [f32]) -> Result<ReadOutcome, AudioReadError> {
        if self.chunk.is_none() {
            match self.pull()? {
                ChunkOutcome::Chunk(chunk) => { self.chunk = Some(*chunk); self.offset = 0; }
                ChunkOutcome::Pending { reason, position } => return Ok(ReadOutcome::Pending { reason, position }),
                ChunkOutcome::Eof { position } => return Ok(ReadOutcome::Eof { position }),
            }
        }
        let chunk = self.chunk.as_ref().expect("a current packet");
        let channels = usize::from(chunk.meta.spec.channels);
        let count = output.len().min(chunk.samples.len() - self.offset) / channels * channels;
        let Some(count) = NonZeroUsize::new(count) else {
            return Ok(ReadOutcome::Pending { reason: PendingReason::Buffering, position: self.pcm.position() });
        };
        let start = self.offset / channels;
        output[..count.get()].copy_from_slice(&chunk.samples[self.offset..self.offset + count.get()]);
        self.offset += count.get();
        let end = self.offset / channels;
        let source_span = chunk.meta.source_span.and_then(|span| span.for_output_range(start as u64..end as u64));
        let position = chunk.meta.source_span.and_then(|span| span.position_at(end as u64))
            .unwrap_or_else(|| chunk.meta.timestamp + chunk.meta.spec.duration_for(end as u64).expect("packet duration"));
        self.committed_segment = Some(chunk.meta.segment);
        self.pcm.set_position(position);
        if self.offset == chunk.samples.len() {
            let chunk = self.chunk.take().expect("consumed packet");
            self.pcm.recycle(PcmPacket::Chunk(chunk)).expect("recycle consumed packet");
        }
        Ok(ReadOutcome::Frames { count, position, source_span })
    }

    fn read_planar<'a>(&mut self, output: &'a mut [&'a mut [f32]]) -> Result<ReadOutcome, AudioReadError> {
        let channels = usize::from(self.spec().channels);
        assert_eq!(output.len(), channels);
        let frames = output.iter().map(|channel| channel.len()).min().unwrap_or(0);
        let mut samples = vec![0.0; frames * channels];
        match self.read(&mut samples)? {
            ReadOutcome::Frames { count, position, source_span } => {
                for (frame, samples) in samples[..count.get()].chunks_exact(channels).enumerate() {
                    for (channel, sample) in samples.iter().enumerate() { output[channel][frame] = *sample; }
                }
                Ok(ReadOutcome::Frames { count: NonZeroUsize::new(count.get() / channels).expect("whole frames"), position, source_span })
            }
            outcome => Ok(outcome),
        }
    }
}

impl<T, S> AudioSession for LaneAudio<T, S> {
    fn duration(&self) -> Option<Duration> { self.pcm.duration() }
    fn event_bus(&self) -> &EventBus { &self.bus }
    fn metadata(&self) -> &TrackMetadata { self.pcm.metadata() }
    fn abr_handle(&self) -> Option<kithara::abr::AbrHandle> { self.pcm.abr_handle() }
    fn is_preloaded(&self) -> bool { self.ready == Some(self.segment) }
}

impl<T, S> AudioControl for LaneAudio<T, S> {
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, AudioReadError> {
        let segment = self.segment.next();
        self.send(LaneCommand::Segment { id: segment, from: position, speed: self.speed.clone() })?;
        self.segment = segment;
        self.ready = None;
        self.eof = false;
        self.failure = None;
        if let Some(chunk) = self.chunk.take() { self.pcm.recycle(PcmPacket::Chunk(chunk)).expect("recycle before seek"); }
        self.offset = 0;
        match self.duration() {
            Some(duration) if position >= duration => Ok(SeekOutcome::PastEof { target: position, duration }),
            _ => Ok(SeekOutcome::Landed { target: position, landed_at: position }),
        }
    }

    fn set_host_sample_rate(&mut self, rate: NonZeroU32) {
        let segment = self.segment.next();
        self.send(LaneCommand::SetHostRate { id: segment, rate }).expect("set test lane rate");
        self.segment = segment;
        self.ready = None;
        if let Some(chunk) = self.chunk.take() { self.pcm.recycle(PcmPacket::Chunk(chunk)).expect("recycle before rate change"); }
        self.offset = 0;
        self.eof = false;
        self.failure = None;
    }
}
