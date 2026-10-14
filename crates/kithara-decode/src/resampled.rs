use std::num::{NonZeroU32, NonZeroUsize};

use kithara_bufpool::{HasPool, PoolRegion, SampleBuffer};
use kithara_platform::time::Duration;
use kithara_resampler::{
    Resampler, ResamplerBackend, ResamplerConfig, ResamplerMode, ResamplerProcess,
    ResamplerSettings, create_resampler,
};
use kithara_signal::{
    AudioChunk, AudioChunkInfo, AudioSpec, FrameCount, InterleavedView, PlanarBuffer,
};
use kithara_stream::{AudioCodec, PendingReason};
use kithara_test_utils::kithara;
use smallvec::SmallVec;

use crate::{
    BlenderProfile, DecodeError, DecodeResult, Decoder, DecoderChunkOutcome,
    DecoderResamplerConfig, DecoderSeekOutcome, DecoderTrackInfo, GaplessInfo,
    GaplessTailCompensation, TrackMetadata,
};

pub(crate) fn wrap<B, S>(
    decoder: Box<dyn Decoder>,
    config: Option<DecoderResamplerConfig<B>>,
    pools: &PoolRegion<S>,
) -> DecodeResult<Box<dyn Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<f32> + Send + Sync + 'static,
{
    let Some(config) = config else {
        return Ok(decoder);
    };
    if decoder.spec().sample_rate == config.target_sample_rate {
        return Ok(decoder);
    }
    Ok(Box::new(ResampledDecoder::new(decoder, config, pools)?))
}

struct ResampledDecoder<B, S>
where
    B: ResamplerBackend,
{
    source_spec: AudioSpec,
    target_spec: AudioSpec,
    config: DecoderResamplerConfig<B>,
    decoder: Box<dyn Decoder>,
    last_input_meta: Option<AudioChunkInfo>,
    pending_meta: Option<AudioChunkInfo>,
    input: PlanarBuffer,
    output: PlanarBuffer,
    scratch: PlanarBuffer,
    pools: PoolRegion<S>,
    resampler: B::Resampler,
    eof_flushed: bool,
    reanchor_output_on_next_chunk: bool,
    emitted_frames: u64,
    output_frame_offset: u64,
    source_frames_seen: u64,
    output_skip_frames: usize,
    prepared: Option<DecodeResult<DecoderChunkOutcome>>,
}

impl<B, S> ResampledDecoder<B, S>
where
    B: ResamplerBackend,
    S: HasPool<f32>,
{
    fn new(
        decoder: Box<dyn Decoder>,
        config: DecoderResamplerConfig<B>,
        pools: &PoolRegion<S>,
    ) -> DecodeResult<Self> {
        let source_spec = decoder.spec();
        let target_spec = AudioSpec::new(source_spec.channels, config.target_sample_rate);
        let resampler = build_resampler(
            config.backend.clone(),
            source_spec,
            config.target_sample_rate,
            config.quality,
            config.options,
            pools,
        )?;
        let empty = FrameCount::new(0);
        Ok(Self {
            config,
            decoder,
            emitted_frames: 0,
            eof_flushed: false,
            input: PlanarBuffer::new(pools, source_spec, empty)?,
            last_input_meta: None,
            output: PlanarBuffer::new(pools, target_spec, empty)?,
            output_frame_offset: 0,
            output_skip_frames: resampler.output_delay(),
            pending_meta: None,
            prepared: None,
            pools: pools.clone(),
            reanchor_output_on_next_chunk: false,
            resampler,
            scratch: PlanarBuffer::new(pools, target_spec, empty)?,
            source_frames_seen: 0,
            source_spec,
            target_spec,
        })
    }

    fn append_chunk(&mut self, chunk: &AudioChunk) -> DecodeResult<()> {
        let spec = chunk.spec();
        let source_spec_changed = spec != self.source_spec;
        if source_spec_changed {
            self.rebuild_for_source_spec(spec)?;
        }
        if source_spec_changed || self.reanchor_output_on_next_chunk {
            self.output_frame_offset = self
                .target_spec
                .frame_at(chunk.meta.timestamp)
                .unwrap_or(u64::MAX);
            self.reanchor_output_on_next_chunk = false;
        }
        if self.pending_meta.is_none() {
            self.pending_meta = Some(chunk.meta);
        }
        self.last_input_meta = Some(chunk.meta);
        let channels = self.channels();
        let frames = chunk.frames();
        self.source_frames_seen = self
            .source_frames_seen
            .saturating_add(u64::try_from(frames).unwrap_or(u64::MAX));
        let base_len = self.input.frames().get();
        let source_frames = FrameCount::new(frames);
        let samples = spec.sample_count(source_frames)?.get();
        self.input.append_interleaved(InterleavedView::new(
            &chunk.samples[..samples],
            spec,
            source_frames,
        )?)?;
        for channel in 0..channels {
            kithara_dsp::sanitize(&mut self.input.channel_mut(channel)?[base_len..]);
        }
        Ok(())
    }

    fn channels(&self) -> usize {
        usize::from(self.source_spec.channels)
    }

    #[kithara::measure(label = "decode.resampled.next")]
    fn prepare_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        if let Some(output) = self.drain_ready()? {
            return Ok(DecoderChunkOutcome::Chunk(output));
        }
        self.decoder.prepare_next_chunk();
        match self.decoder.next_chunk_prepared()? {
            DecoderChunkOutcome::Chunk(chunk) => {
                self.append_chunk(&chunk)?;
                Ok(self.drain_ready()?.map_or(
                    DecoderChunkOutcome::Pending(PendingReason::Retry),
                    DecoderChunkOutcome::Chunk,
                ))
            }
            DecoderChunkOutcome::Pending(reason) => Ok(DecoderChunkOutcome::Pending(reason)),
            DecoderChunkOutcome::Eof => {
                if !self.eof_flushed {
                    self.eof_flushed = true;
                    if let Some(output) = self.flush_residual()? {
                        return Ok(DecoderChunkOutcome::Chunk(output));
                    }
                }
                Ok(DecoderChunkOutcome::Eof)
            }
        }
    }

    /// Resamples every whole block the buffered input affords and emits them
    /// as one chunk.
    ///
    /// A resampler consumes a fixed input block, so emitting one chunk per
    /// block would hand the consumer pieces a fraction of the size the
    /// decoder produced, while the bypassed path hands over whole decoder
    /// chunks. A consumer that plans its work per chunk then sees the stream
    /// break up for no reason other than the presence of a resampler, so the
    /// blocks of one input chunk are coalesced back into one output chunk.
    #[kithara::hang_watchdog]
    fn drain_ready(&mut self) -> DecodeResult<Option<AudioChunk>> {
        loop {
            hang_tick!();
            let input_frames = self.resampler.input_frames_next();
            if self.input.frames().get() < input_frames {
                return self.finish_output();
            }
            let process = self.process_block(input_frames)?;
            if process.first() > input_frames {
                return Err(DecodeError::InvalidData {
                    detail: "decoder resampler consumed more frames than supplied",
                });
            }
            self.drop_consumed(process.first())?;
        }
    }

    fn drop_consumed(&mut self, frames: usize) -> DecodeResult<()> {
        self.input.truncate_front(FrameCount::new(frames))?;
        Ok(())
    }

    fn expected_output_frames(&self) -> u64 {
        let source_rate = self.source_spec.sample_rate.get();
        let expected = u128::from(self.source_frames_seen)
            .saturating_mul(u128::from(self.config.target_sample_rate.get()))
            .saturating_add(u128::from(source_rate / 2))
            / u128::from(source_rate);
        u64::try_from(expected).unwrap_or(u64::MAX)
    }

    fn finish_output(&mut self) -> DecodeResult<Option<AudioChunk>> {
        if self.output.frames().get() == 0 {
            return Ok(None);
        }
        let frames = self.output.frames();
        let samples = self.interleave(frames)?;
        let mut meta = self.pending_meta.take().unwrap_or_default();
        meta.spec = self.target_spec;
        meta.frame_offset = self.output_frame_offset;
        meta.frames = u32::try_from(frames.get()).unwrap_or(u32::MAX);
        meta.timestamp = self
            .target_spec
            .duration_for(meta.frame_offset)
            .unwrap_or(Duration::from_nanos(u64::MAX));
        self.output_frame_offset = self
            .output_frame_offset
            .saturating_add(u64::try_from(frames.get()).unwrap_or(u64::MAX));
        self.emitted_frames = self
            .emitted_frames
            .saturating_add(u64::try_from(frames.get()).unwrap_or(u64::MAX));
        meta.end_timestamp = self
            .target_spec
            .duration_for(self.output_frame_offset)
            .unwrap_or(Duration::from_nanos(u64::MAX));
        self.output.clear();
        Ok(Some(AudioChunk::new(meta, samples)))
    }

    fn flush_residual(&mut self) -> DecodeResult<Option<AudioChunk>> {
        if self.input.frames().get() == 0
            && self.ready_output_frames() >= self.expected_output_frames()
        {
            return Ok(None);
        }
        if self.pending_meta.is_none() {
            self.pending_meta = self.last_input_meta;
        }
        while self.ready_output_frames() < self.expected_output_frames() {
            let input_frames = self.resampler.input_frames_next();
            self.input.resize_frames(FrameCount::new(input_frames))?;
            let ready_before = self.ready_output_frames();
            let process = self.process_block(input_frames)?;
            if process.first() > input_frames {
                return Err(DecodeError::InvalidData {
                    detail: "decoder resampler consumed more frames than supplied",
                });
            }
            if process.first() == 0 && self.ready_output_frames() == ready_before {
                break;
            }
            self.drop_consumed(process.first())?;
        }
        self.finish_output()
    }

    fn interleave(&self, frames: FrameCount) -> DecodeResult<SampleBuffer> {
        let mut samples = self.pools.get::<f32>();
        let sample_count = self.target_spec.sample_count(frames)?.get();
        samples.ensure_len(sample_count)?;
        self.output.view().interleave_into(&mut samples)?;
        Ok(samples)
    }

    fn process_block(&mut self, input_frames: usize) -> DecodeResult<ResamplerProcess> {
        let channels = self.channels();
        let output_frames = self.resampler.output_frames_next();
        self.scratch.resize_frames(FrameCount::new(output_frames))?;
        let input_view = self.input.view().range(0..input_frames)?;
        let input = (0..channels)
            .map(|channel| input_view.channel(channel))
            .collect::<Result<SmallVec<[&[f32]; 8]>, _>>()?;
        let process = {
            let stride = self.scratch.stride().get();
            let mut remaining = self.scratch.as_samples_mut();
            let mut output = SmallVec::<[&mut [f32]; 8]>::with_capacity(channels);
            for _ in 0..channels {
                let (channel, rest) = remaining.split_at_mut(stride);
                output.push(&mut channel[..output_frames]);
                remaining = rest;
            }
            self.resampler
                .process_into_buffer(&input, &mut output)
                .map_err(DecodeError::backend)?
        };
        if process.second() > output_frames {
            return Err(DecodeError::InvalidData {
                detail: "decoder resampler produced more frames than requested",
            });
        }
        let skip = self.output_skip_frames.min(process.second());
        self.output_skip_frames -= skip;
        let available = process.second().saturating_sub(skip);
        let remaining = self
            .expected_output_frames()
            .saturating_sub(self.ready_output_frames());
        let usable = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(available);
        let old_len = self.output.frames().get();
        self.output
            .resize_frames(FrameCount::new(old_len.saturating_add(usable)))?;
        for channel in 0..channels {
            let src = &self.scratch.channel(channel)?[skip..skip + usable];
            let dst = &mut self.output.channel_mut(channel)?[old_len..old_len + usable];
            dst.copy_from_slice(src);
        }
        Ok(process)
    }

    fn ready_output_frames(&self) -> u64 {
        self.emitted_frames
            .saturating_add(u64::try_from(self.output.frames().get()).unwrap_or(u64::MAX))
    }

    fn rebuild_for_source_spec(&mut self, source_spec: AudioSpec) -> DecodeResult<()> {
        let target_spec = AudioSpec::new(source_spec.channels, self.config.target_sample_rate);
        let resampler = build_resampler(
            self.config.backend.clone(),
            source_spec,
            self.config.target_sample_rate,
            self.config.quality,
            self.config.options,
            &self.pools,
        )?;
        let empty = FrameCount::new(0);
        let input = PlanarBuffer::new(&self.pools, source_spec, empty)?;
        let output = PlanarBuffer::new(&self.pools, target_spec, empty)?;
        let scratch = PlanarBuffer::new(&self.pools, target_spec, empty)?;
        self.source_spec = source_spec;
        self.target_spec = target_spec;
        self.input = input;
        self.output = output;
        self.output_skip_frames = resampler.output_delay();
        self.scratch = scratch;
        self.resampler = resampler;
        self.emitted_frames = 0;
        self.source_frames_seen = 0;
        Ok(())
    }

    fn reset_resampler_state(&mut self) {
        self.input.clear();
        self.output.clear();
        self.pending_meta = None;
        self.reanchor_output_on_next_chunk = false;
        self.last_input_meta = None;
        self.emitted_frames = 0;
        self.eof_flushed = false;
        self.output_skip_frames = self.resampler.output_delay();
        self.source_frames_seen = 0;
        self.resampler.reset();
    }

    fn scaled_gapless(&self, info: GaplessInfo) -> DecodeResult<GaplessInfo> {
        let source_rate = self.source_spec.sample_rate.get();
        let target_rate = self.config.target_sample_rate.get();
        Ok(GaplessInfo {
            leading_frames: round_scaled_frames(info.leading_frames, source_rate, target_rate)?,
            trailing_frames: round_scaled_frames(info.trailing_frames, source_rate, target_rate)?,
        })
    }
}

impl<B, S> Decoder for ResampledDecoder<B, S>
where
    B: ResamplerBackend,
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn blender_profile(&self) -> BlenderProfile {
        BlenderProfile::new(self.target_spec)
    }

    fn default_priming_frames(&self, codec: AudioCodec) -> u64 {
        let source = self.decoder.default_priming_frames(codec);
        round_scaled_frames_lossy(
            source,
            self.source_spec.sample_rate.get(),
            self.config.target_sample_rate.get(),
        )
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        self.prepare_next_chunk();
        self.next_chunk_prepared()
    }

    fn next_chunk_prepared(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        self.prepared.take().ok_or(DecodeError::InvalidData {
            detail: "resampled decoder output was not prepared",
        })?
    }

    fn has_prepared_result(&self) -> bool {
        self.prepared
            .as_ref()
            .is_some_and(|result| !matches!(result, Ok(DecoderChunkOutcome::Pending(_))))
    }

    fn prepare_next_chunk(&mut self) {
        if !self.has_prepared_result() {
            self.prepared = Some(self.prepare_chunk());
        }
    }

    fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        self.prepared = None;
        let outcome = self.decoder.seek(pos)?;
        self.reset_resampler_state();
        match outcome {
            DecoderSeekOutcome::Landed {
                landed_at,
                landed_byte,
                preroll,
                ..
            } => {
                self.output_frame_offset = self.target_spec.frame_at(landed_at).unwrap_or(u64::MAX);
                self.reanchor_output_on_next_chunk = true;
                Ok(DecoderSeekOutcome::Landed {
                    landed_at,
                    landed_byte,
                    preroll,
                    landed_frame: self.output_frame_offset,
                })
            }
            DecoderSeekOutcome::PastEof { duration } => {
                Ok(DecoderSeekOutcome::PastEof { duration })
            }
        }
    }

    fn spec(&self) -> AudioSpec {
        self.target_spec
    }

    fn timeline_gap_frames(&self) -> u64 {
        round_scaled_frames_lossy(
            self.decoder.timeline_gap_frames(),
            self.source_spec.sample_rate.get(),
            self.config.target_sample_rate.get(),
        )
    }

    fn track_info(&self) -> DecoderTrackInfo {
        let info = self.decoder.track_info();
        DecoderTrackInfo {
            gapless: info
                .gapless
                .map(|gapless| self.scaled_gapless(gapless))
                .transpose()
                .unwrap_or(None),
            gapless_tail: info.gapless_tail.and_then(|tail| {
                GaplessTailCompensation::for_source_frames(
                    tail.ideal_pre_trim_frames(),
                    self.source_spec.sample_rate.get(),
                    self.config.target_sample_rate.get(),
                )
            }),
        }
    }

    delegate::delegate! {
        to self.decoder {
            fn duration(&self) -> Option<Duration>;
            fn flush_reader_signals(&mut self);
            fn metadata(&self) -> TrackMetadata;
            fn update_byte_len(&self, len: u64);
        }
    }
}

fn build_resampler<B, S>(
    backend: B,
    source_spec: AudioSpec,
    target_sample_rate: NonZeroU32,
    quality: kithara_resampler::ResamplerQuality,
    options: kithara_resampler::ResamplerOptions,
    pools: &PoolRegion<S>,
) -> DecodeResult<B::Resampler>
where
    B: ResamplerBackend,
    S: HasPool<f32>,
{
    let channels =
        NonZeroUsize::new(usize::from(source_spec.channels)).ok_or(DecodeError::InvalidData {
            detail: "decoder resampler requires at least one channel",
        })?;
    let settings = ResamplerSettings::builder()
        .channels(channels)
        .mode(ResamplerMode::FixedRatio {
            target_sample_rate,
            source_sample_rate: source_spec.sample_rate,
        })
        .options(options)
        .quality(quality)
        .pools(pools.clone())
        .build();
    let config = ResamplerConfig::builder()
        .backend(backend)
        .settings(settings)
        .build();
    create_resampler(&config).map_err(DecodeError::backend)
}

fn round_scaled_frames(count: u64, source_rate: u32, target_rate: u32) -> DecodeResult<u64> {
    if source_rate == 0 {
        return Err(DecodeError::InvalidSampleRate {
            resource: "decoder.resampler.source",
        });
    }
    if target_rate == 0 {
        return Err(DecodeError::InvalidSampleRate {
            resource: "decoder.resampler.target",
        });
    }
    let numerator = u128::from(count)
        .saturating_mul(u128::from(target_rate))
        .saturating_add(u128::from(source_rate / 2));
    let scaled = numerator / u128::from(source_rate);
    u64::try_from(scaled).map_err(|_| DecodeError::InvalidData {
        detail: "decoder resampler frame count overflow",
    })
}

fn round_scaled_frames_lossy(count: u64, source_rate: u32, target_rate: u32) -> u64 {
    round_scaled_frames(count, source_rate, target_rate).unwrap_or(u64::MAX)
}

#[cfg(all(test, feature = "resample-rubato"))]
mod tests {
    use std::{collections::VecDeque, sync::Mutex};

    use kithara_platform::sync::Arc;
    use kithara_resampler::{ResamplerOptions, ResamplerQuality, rubato::RubatoBackend};
    use kithara_stream::{
        BoxedEventSink, PrerollHint, ReaderChunkSignal, ReaderEventSink, ReaderSeekSignal,
    };

    use super::*;
    use crate::test_pools::{Pools, TestPools, pools, sample_buffer};

    #[derive(Default)]
    struct Calls {
        reads: usize,
        prepares: usize,
        signals: Vec<&'static str>,
        flushes: usize,
    }

    struct Hooks(Arc<Mutex<Calls>>);

    impl ReaderEventSink for Hooks {
        fn on_chunk(&mut self, signal: ReaderChunkSignal) {
            let tag = match signal {
                ReaderChunkSignal::Chunk => "chunk",
                ReaderChunkSignal::Pending(_) => "pending",
                ReaderChunkSignal::Eof => "eof",
                _ => panic!("unexpected reader signal"),
            };
            self.0.lock().expect("calls").signals.push(tag);
        }

        fn on_seek(&mut self, _signal: ReaderSeekSignal) {
            self.0.lock().expect("calls").signals.push("seek");
        }

        fn flush(&mut self) {
            self.0.lock().expect("calls").flushes += 1;
        }
    }

    struct ScriptedDecoder {
        outcomes: VecDeque<DecodeResult<DecoderChunkOutcome>>,
        after_seek: Option<Vec<DecodeResult<DecoderChunkOutcome>>>,
        calls: Arc<Mutex<Calls>>,
        hooks: BoxedEventSink,
    }

    impl Decoder for ScriptedDecoder {
        fn duration(&self) -> Option<Duration> {
            Some(Duration::from_secs(2))
        }

        fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
            self.calls.lock().expect("calls").reads += 1;
            let result = self.outcomes.pop_front().expect("bounded source script");
            if let Ok(outcome) = &result {
                self.hooks.on_chunk(match outcome {
                    DecoderChunkOutcome::Chunk(_) => ReaderChunkSignal::Chunk,
                    DecoderChunkOutcome::Pending(reason) => ReaderChunkSignal::Pending(*reason),
                    DecoderChunkOutcome::Eof => ReaderChunkSignal::Eof,
                });
            }
            result
        }

        fn prepare_next_chunk(&mut self) {
            self.calls.lock().expect("calls").prepares += 1;
        }

        fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
            self.outcomes = self.after_seek.take().expect("seek script").into();
            self.hooks.on_seek(ReaderSeekSignal::Landed {
                landed_byte: Some(0),
                preroll: PrerollHint::NotNeeded,
            });
            Ok(DecoderSeekOutcome::Landed {
                landed_at: pos,
                landed_frame: source_spec().frame_at(pos).expect("source frame"),
                landed_byte: Some(0),
                preroll: PrerollHint::NotNeeded,
            })
        }

        fn spec(&self) -> AudioSpec {
            source_spec()
        }

        fn flush_reader_signals(&mut self) {
            self.hooks.flush();
        }

        fn update_byte_len(&self, _len: u64) {}
    }

    fn source_spec() -> AudioSpec {
        AudioSpec::new(1, NonZeroU32::new(44_100).expect("source rate"))
    }

    fn chunk(pools: &Pools, frames: usize, offset: u64, epoch: u64) -> DecoderChunkOutcome {
        let spec = source_spec();
        let samples: Vec<f32> = (0..frames)
            .map(|frame| {
                let frame = offset + u64::try_from(frame).expect("fixture frame");
                let marker = u8::try_from(frame % 31).expect("bounded marker");
                f32::from(marker) / 31.0
            })
            .collect();
        let frames_u64 = u64::try_from(frames).expect("fixture length");
        DecoderChunkOutcome::Chunk(AudioChunk::new(
            AudioChunkInfo {
                spec,
                frames: u32::try_from(frames).expect("fixture frames"),
                timestamp: spec.duration_for(offset).expect("fixture timestamp"),
                end_timestamp: spec.duration_for(offset + frames_u64).expect("fixture end"),
                frame_offset: offset,
                epoch,
                ..AudioChunkInfo::default()
            },
            sample_buffer(pools, &samples),
        ))
    }

    fn scripted_resampled(
        pools: &Pools,
        outcomes: Vec<DecodeResult<DecoderChunkOutcome>>,
        after_seek: Option<Vec<DecodeResult<DecoderChunkOutcome>>>,
    ) -> (
        ResampledDecoder<RubatoBackend, TestPools>,
        Arc<Mutex<Calls>>,
    ) {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let inner = ScriptedDecoder {
            outcomes: outcomes.into(),
            after_seek,
            calls: Arc::clone(&calls),
            hooks: Box::new(Hooks(Arc::clone(&calls))),
        };
        let config = DecoderResamplerConfig::builder()
            .backend(RubatoBackend::new())
            .target_sample_rate(NonZeroU32::new(48_000).expect("target rate"))
            .options(ResamplerOptions::builder().chunk_size(256).build())
            .quality(ResamplerQuality::Fast)
            .build();
        (
            ResampledDecoder::new(Box::new(inner), config, pools).expect("resampled decoder"),
            calls,
        )
    }

    #[kithara::rtsan_forbid_blocking]
    fn consume_prepared(decoder: &mut dyn Decoder) -> DecodeResult<DecoderChunkOutcome> {
        decoder.next_chunk_prepared()
    }

    #[kithara::test]
    fn a_short_inner_chunk_takes_one_preparation_step() {
        let pools = pools();
        let (mut decoder, calls) = scripted_resampled(
            &pools,
            vec![Ok(chunk(&pools, 128, 0, 1)), Ok(chunk(&pools, 128, 128, 1))],
            None,
        );
        assert!(decoder.resampler.input_frames_next() > 128);
        decoder.prepare_next_chunk();
        assert_eq!(calls.lock().expect("calls").reads, 1);
        assert_eq!(calls.lock().expect("calls").prepares, 1);
        assert!(!decoder.has_prepared_result());
        assert!(matches!(
            consume_prepared(&mut decoder).expect("provisional result"),
            DecoderChunkOutcome::Pending(PendingReason::Retry)
        ));
        assert_eq!(calls.lock().expect("calls").reads, 1);
        decoder.prepare_next_chunk();
        assert_eq!(calls.lock().expect("calls").reads, 2);
        assert_eq!(calls.lock().expect("calls").prepares, 2);
        assert!(decoder.has_prepared_result());
        let DecoderChunkOutcome::Chunk(output) =
            consume_prepared(&mut decoder).expect("completed block")
        else {
            panic!("two short inputs must complete a resampler block");
        };
        assert!(output.frames() > 0);
        assert_eq!(output.meta.frame_offset, 0);
        assert_eq!(decoder.source_frames_seen, 256);
        assert_eq!(calls.lock().expect("calls").signals, ["chunk", "chunk"]);
    }

    #[derive(Clone, Copy, Debug)]
    enum Completion {
        Chunk,
        Error,
        Eof,
    }

    fn completed(pools: &Pools, completion: Completion) -> DecodeResult<DecoderChunkOutcome> {
        match completion {
            Completion::Chunk => Ok(chunk(pools, 512, 0, 1)),
            Completion::Error => Err(DecodeError::InvalidData {
                detail: "scripted decode failure",
            }),
            Completion::Eof => Ok(DecoderChunkOutcome::Eof),
        }
    }

    #[kithara::test]
    #[case::chunk(Completion::Chunk)]
    #[case::error(Completion::Error)]
    #[case::eof(Completion::Eof)]
    fn completed_preparation_is_retained_and_delivered_once(#[case] completion: Completion) {
        let pools = pools();
        let (mut decoder, calls) =
            scripted_resampled(&pools, vec![completed(&pools, completion)], None);
        decoder.prepare_next_chunk();
        assert!(decoder.has_prepared_result());
        let allocated = pools.stats().allocated_bytes;
        let source_frames = decoder.source_frames_seen;
        let emitted_frames = decoder.emitted_frames;
        let signals = calls.lock().expect("calls").signals.len();
        for _ in 0..2 {
            decoder.prepare_next_chunk();
            assert!(decoder.has_prepared_result());
            assert_eq!(calls.lock().expect("calls").reads, 1);
            assert_eq!(calls.lock().expect("calls").prepares, 1);
            assert_eq!(decoder.source_frames_seen, source_frames);
            assert_eq!(pools.stats().allocated_bytes, allocated);
        }
        let result = consume_prepared(&mut decoder);
        assert_eq!(calls.lock().expect("calls").reads, 1);
        assert_eq!(calls.lock().expect("calls").prepares, 1);
        assert_eq!(calls.lock().expect("calls").signals.len(), signals);
        assert_eq!(pools.stats().allocated_bytes, allocated);
        assert_eq!(decoder.source_frames_seen, source_frames);
        assert_eq!(decoder.emitted_frames, emitted_frames);
        match (completion, result) {
            (Completion::Chunk, Ok(DecoderChunkOutcome::Chunk(output))) => {
                assert!(output.frames() > 0);
                assert_eq!(output.meta.epoch, 1);
            }
            (Completion::Error, Err(DecodeError::InvalidData { detail })) => {
                assert_eq!(detail, "scripted decode failure");
            }
            (Completion::Eof, Ok(DecoderChunkOutcome::Eof)) => {}
            other => panic!("completed result was replaced: {other:?}"),
        }
        assert!(!decoder.has_prepared_result());
        assert!(matches!(
            consume_prepared(&mut decoder),
            Err(DecodeError::InvalidData { .. })
        ));
        assert_eq!(calls.lock().expect("calls").reads, 1);
        assert_eq!(calls.lock().expect("calls").prepares, 1);
        assert_eq!(
            calls.lock().expect("calls").signals.len(),
            usize::from(!matches!(completion, Completion::Error))
        );
        decoder.flush_reader_signals();
        assert_eq!(calls.lock().expect("calls").flushes, 1);
    }

    #[kithara::test]
    #[case::chunk(Completion::Chunk)]
    #[case::error(Completion::Error)]
    #[case::eof(Completion::Eof)]
    fn seek_discards_a_completed_preparation(#[case] completion: Completion) {
        let pools = pools();
        let (mut decoder, calls) = scripted_resampled(
            &pools,
            vec![completed(&pools, completion)],
            Some(vec![Ok(chunk(&pools, 512, 44_100, 2))]),
        );
        decoder.prepare_next_chunk();
        assert!(decoder.has_prepared_result());
        decoder.seek(Duration::from_secs(1)).expect("seek");
        assert!(!decoder.has_prepared_result());
        assert!(matches!(
            consume_prepared(&mut decoder),
            Err(DecodeError::InvalidData { .. })
        ));
        decoder.prepare_next_chunk();
        let DecoderChunkOutcome::Chunk(output) =
            consume_prepared(&mut decoder).expect("after seek")
        else {
            panic!("seek must deliver the new input");
        };
        assert_eq!(output.meta.epoch, 2);
        assert_eq!(output.meta.timestamp, Duration::from_secs(1));
        assert_eq!(output.meta.frame_offset, 48_000);
        assert_eq!(calls.lock().expect("calls").reads, 2);
        assert_eq!(
            calls
                .lock()
                .expect("calls")
                .signals
                .iter()
                .filter(|signal| **signal == "seek")
                .count(),
            1
        );
    }

    fn pcm_to_eof(decoder: &mut dyn Decoder, prepared: bool) -> Vec<f32> {
        let mut pcm = Vec::new();
        let mut frame_offset = 0;
        for _ in 0..16 {
            let result = if prepared {
                decoder.prepare_next_chunk();
                consume_prepared(decoder)
            } else {
                decoder.next_chunk()
            };
            match result.expect("decode") {
                DecoderChunkOutcome::Chunk(output) => {
                    assert_eq!(output.meta.frame_offset, frame_offset);
                    frame_offset += u64::try_from(output.frames()).expect("output frames");
                    assert_eq!(output.spec(), decoder.spec());
                    pcm.extend_from_slice(&output.samples);
                }
                DecoderChunkOutcome::Pending(PendingReason::Retry) => {}
                DecoderChunkOutcome::Eof => return pcm,
                other @ DecoderChunkOutcome::Pending(_) => {
                    panic!("unexpected scripted outcome: {other:?}");
                }
            }
        }
        panic!("EOF within the script budget");
    }

    #[kithara::test]
    fn prepared_pcm_eof_residual_accounting_and_hooks_match_regular_decode() {
        let pools = pools();
        let script = || {
            vec![
                Ok(chunk(&pools, 128, 0, 1)),
                Ok(chunk(&pools, 384, 128, 1)),
                Ok(chunk(&pools, 17, 512, 1)),
                Ok(DecoderChunkOutcome::Eof),
                Ok(DecoderChunkOutcome::Eof),
            ]
        };
        let (mut prepared, prepared_calls) = scripted_resampled(&pools, script(), None);
        let (mut regular, regular_calls) = scripted_resampled(&pools, script(), None);
        let actual = pcm_to_eof(&mut prepared, true);
        let expected = pcm_to_eof(&mut regular, false);
        assert_eq!(actual, expected);
        let output_frames = u64::try_from(actual.len()).expect("output frame count");
        assert_eq!(
            output_frames,
            round_scaled_frames(529, 44_100, 48_000).expect("scaled frames")
        );
        assert_eq!(prepared.source_frames_seen, 529);
        assert_eq!(prepared.emitted_frames, output_frames);
        assert_eq!(
            prepared_calls.lock().expect("calls").signals,
            ["chunk", "chunk", "chunk", "eof", "eof"]
        );
        assert_eq!(
            prepared_calls.lock().expect("calls").signals,
            regular_calls.lock().expect("calls").signals
        );
        assert!(!prepared.has_prepared_result());
    }
}
