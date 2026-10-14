use std::sync::atomic::Ordering;

use kithara_bufpool::{HasPool, SampleBuffer};
use kithara_platform::time::Duration;
use kithara_signal::{AudioChunk, AudioChunkInfo, AudioSpec};
use kithara_stream::{
    AudioCodec, NotReadyCause, PendingReason, ReaderChunkSignal, ReaderSeekSignal,
};
use kithara_test_utils::kithara;

use super::{ComposedDecoder, DecoderRuntime, HeadStrip, PcmOriginState, Prefill, frames_to_trim};
use crate::{
    BlenderProfile, PcmOrigin,
    codec::FrameCodec,
    consts,
    demuxer::{DemuxOutcome, DemuxSeekOutcome, Demuxer},
    error::{DecodeError, DecodeResult},
    traits::{Decoder, DecoderChunkOutcome, DecoderSeekOutcome},
    types::TrackMetadata,
};

impl HeadStrip {
    /// Stops once a packet comes back whole: from there on the decoder is
    /// through its delay, and a short packet is the stream's own tail
    /// rather than more strip.
    const fn record(&mut self, supplied: u64, emitted: u64) {
        if self.settled || supplied == 0 {
            return;
        }
        if emitted >= supplied {
            self.settled = true;
            return;
        }
        self.frames = self.frames.saturating_add(supplied - emitted);
    }
}

impl<D, C, S> ComposedDecoder<D, C, S>
where
    D: Demuxer,
    C: FrameCodec,
    S: HasPool<f32>,
{
    /// Build a decoder from a `(demuxer, codec)` pair and its runtime wiring.
    pub(crate) fn new(demuxer: D, codec: C, runtime: DecoderRuntime<S>) -> Self {
        let spec = codec.spec();
        let duration = demuxer.duration();
        Self {
            demuxer,
            codec,
            spec,
            duration,
            origin: PcmOriginState::Observing(None),
            pools: runtime.pools,
            output: None,
            prefill: Prefill::Needed,
            byte_len_handle: runtime.byte_len_handle,
            hooks: runtime.hooks,
            frame_offset: 0,
            pending_seek_target: None,
            resync_frame_offset_to_pts: true,
            timeline_gap_frames: 0,
            head_strip: HeadStrip::default(),
            zero_frame_count: 0,
        }
    }

    fn source_media_origin(&self) -> Option<Duration> {
        self.demuxer
            .media_origin()?
            .checked_sub(self.codec.media_time_rebase()?)
    }

    /// Build the output `AudioChunk` from a just-filled pool buffer plus
    /// the demuxed frame's metadata. Inlined fields (rather than taking
    /// `&Frame<'_>`) so the caller can release the demuxer borrow before
    /// invoking this — needed because `Frame<'_>` borrows into the
    /// demuxer state and would conflict with `&mut self` here.
    #[kithara::probe(timestamp, frames)]
    fn build_chunk(
        &mut self,
        buf: SampleBuffer,
        frames: u32,
        timestamp: Duration,
        source_bytes: u64,
    ) -> AudioChunk {
        if let PcmOriginState::Observing(packet) = self.origin {
            self.origin = packet.map_or(PcmOriginState::Sealed, |packet| {
                PcmOriginState::Captured(PcmOrigin::new(timestamp, timestamp.min(packet)))
            });
        }
        let live_spec = self.codec.spec();
        self.spec = live_spec;

        let chunk_secs = f64::from(frames) / f64::from(live_spec.sample_rate.get());
        let frame_duration = Duration::from_secs_f64(chunk_secs);
        let end_timestamp = timestamp.saturating_add(frame_duration);

        let timestamp_frame = live_spec.frame_at(timestamp).unwrap_or(u64::MAX);
        if self.resync_frame_offset_to_pts {
            self.resync_frame_offset_to_pts = false;
            self.frame_offset = timestamp_frame;
        } else if timestamp_frame > self.frame_offset {
            self.timeline_gap_frames = self
                .timeline_gap_frames
                .saturating_add(timestamp_frame.saturating_sub(self.frame_offset));
            self.frame_offset = timestamp_frame;
        }
        let frame_offset = self.frame_offset;
        self.frame_offset = self.frame_offset.saturating_add(u64::from(frames));

        let meta = AudioChunkInfo {
            end_timestamp,
            timestamp,
            media_origin: self.source_media_origin(),
            frames,
            frame_offset,
            source_bytes,
            segment_index: self.demuxer.current_segment_index(),
            source_byte_offset: None,
            variant_index: self.demuxer.current_variant_index(),
            spec: live_spec,
            render_revision: 0,
            mapping_revision: None,
            ..AudioChunkInfo::default()
        };
        AudioChunk::new(meta, buf)
    }

    fn decode_prepared(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        let output = self.output.take().ok_or(DecodeError::InvalidData {
            detail: "decoder output was not prepared",
        })??;
        self.output = Some(Ok(output));
        self.next_chunk_inner()
    }

    fn drain_codec_eof(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        if !self
            .codec
            .needs_eof_drain(self.demuxer.track_info().sample_rate)
        {
            return Ok(DecoderChunkOutcome::Eof);
        }

        let mut buf = self.output.take().ok_or(DecodeError::InvalidData {
            detail: "decoder output was not prepared",
        })??;
        let timestamp = self
            .spec
            .duration_for(self.frame_offset)
            .unwrap_or(Duration::from_nanos(u64::MAX));
        let frames = match self.codec.decode_frame(&[], timestamp, &[], &mut buf) {
            Ok(frames) => frames,
            Err(error) => {
                self.output = Some(Ok(buf));
                return Err(error);
            }
        };
        if frames == 0 {
            self.output = Some(Ok(buf));
            return Ok(DecoderChunkOutcome::Eof);
        }
        let timestamp = self.codec.decoded_pts().unwrap_or(timestamp);
        let chunk = self.build_chunk(buf, frames, timestamp, 0);
        Ok(DecoderChunkOutcome::Chunk(Box::new(chunk)))
    }

    fn emit_chunk_signal(&mut self, outcome: &DecoderChunkOutcome) {
        let signal = match outcome {
            DecoderChunkOutcome::Chunk(_) => ReaderChunkSignal::Chunk,
            DecoderChunkOutcome::Pending(reason) => ReaderChunkSignal::Pending(*reason),
            DecoderChunkOutcome::Eof => ReaderChunkSignal::Eof,
        };
        if let Some(hooks) = self.hooks.as_mut() {
            hooks.on_chunk(signal);
        }
    }

    fn emit_seek_signal(&mut self, outcome: &DecoderSeekOutcome) {
        let signal = match outcome {
            DecoderSeekOutcome::Landed {
                landed_byte,
                preroll,
                ..
            } => ReaderSeekSignal::Landed {
                landed_byte: *landed_byte,
                preroll: *preroll,
            },
            DecoderSeekOutcome::PastEof { .. } => ReaderSeekSignal::PastEof,
        };
        if let Some(hooks) = self.hooks.as_mut() {
            hooks.on_seek(signal);
        }
    }

    /// A head-trimmed packet keeps its end time, so the missing prefix precedes its PCM. Duration
    /// rounding can leave the frame's end past the target even when the packet is fully pre-target
    /// in sample space.
    #[kithara::measure(label = "decode.composed.next")]
    #[kithara::hang_watchdog]
    fn next_chunk_inner(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        loop {
            hang_tick!();
            let frame = match self.demuxer.next_frame_prepared()? {
                DemuxOutcome::Frame(frame) => frame,
                DemuxOutcome::Pending(reason) => {
                    return Ok(DecoderChunkOutcome::Pending(reason));
                }
                DemuxOutcome::Eof => return self.drain_codec_eof(),
            };
            hang_reset!();
            let frame_pts = frame.pts;
            if let PcmOriginState::Observing(packet) = &mut self.origin
                && packet.is_none()
            {
                *packet = Some(frame_pts);
            }
            let frame_duration = frame.duration;
            let frame_end = frame_pts.saturating_add(frame_duration);
            let source_bytes = u64::try_from(frame.data.len()).unwrap_or(u64::MAX);
            let mut buf = self.output.take().ok_or(DecodeError::InvalidData {
                detail: "decoder output was not prepared",
            })??;
            let mut frames =
                match self
                    .codec
                    .decode_frame(frame.data, frame_pts, frame.packet_desc, &mut buf)
                {
                    Ok(frames) => frames,
                    Err(error) => {
                        self.output = Some(Ok(buf));
                        return Err(error);
                    }
                };
            let prior_head_strip = self.head_strip.frames();
            let decoded_pts = self.codec.decoded_pts();
            if decoded_pts.is_none() {
                self.head_strip.record(
                    self.spec.frame_at(frame_duration).unwrap_or(u64::MAX),
                    u64::from(frames),
                );
            }
            let zero_frame_budget_reached = if frames == 0 {
                self.zero_frame_count = self.zero_frame_count.saturating_add(1);
                self.zero_frame_count >= consts::ZERO_FRAME_BUDGET
            } else {
                self.zero_frame_count = 0;
                false
            };
            let mut chunk_pts = if frames == 0 {
                frame_pts
            } else if let Some(decoded_pts) = decoded_pts {
                decoded_pts
            } else {
                let stripped = self.head_strip.frames().saturating_sub(prior_head_strip);
                frame_pts.saturating_add(
                    self.codec
                        .spec()
                        .duration_for(stripped)
                        .unwrap_or(Duration::MAX),
                )
            };
            if let Some(target) = self.pending_seek_target {
                let decoded_end = chunk_pts.saturating_add(
                    self.codec
                        .spec()
                        .duration_for(u64::from(frames))
                        .unwrap_or(Duration::from_nanos(u64::MAX)),
                );
                if (frames == 0 && frame_end <= target) || (frames > 0 && decoded_end <= target) {
                    self.output = Some(Ok(buf));
                    if zero_frame_budget_reached {
                        return Ok(DecoderChunkOutcome::Pending(PendingReason::NotReady(
                            NotReadyCause::SourcePending,
                        )));
                    }
                    continue;
                }
                if frames > 0 && chunk_pts < target {
                    let live_spec = self.codec.spec();
                    let trim_frames_u64 =
                        frames_to_trim(chunk_pts, target, live_spec.sample_rate.get())
                            .min(u64::from(frames));
                    let trim_frames = u32::try_from(trim_frames_u64).unwrap_or(frames);
                    if trim_frames >= frames {
                        self.output = Some(Ok(buf));
                        continue;
                    }
                    if trim_frames > 0 {
                        let channels = usize::from(live_spec.channels);
                        let trim_samples = usize::try_from(trim_frames)
                            .unwrap_or(0)
                            .saturating_mul(channels);
                        let total_samples = usize::try_from(frames)
                            .unwrap_or(0)
                            .saturating_mul(channels);
                        if trim_samples < total_samples && trim_samples <= buf.len() {
                            buf.copy_within(trim_samples..total_samples, 0);
                            buf.truncate(total_samples - trim_samples);
                            frames = frames.saturating_sub(trim_frames);
                            chunk_pts = target;
                        }
                    }
                }
                if frames > 0 {
                    self.pending_seek_target = None;
                }
            }
            if frames == 0 {
                self.output = Some(Ok(buf));
                if zero_frame_budget_reached {
                    return Ok(DecoderChunkOutcome::Pending(PendingReason::NotReady(
                        NotReadyCause::SourcePending,
                    )));
                }
                continue;
            }
            let chunk = self.build_chunk(buf, frames, chunk_pts, source_bytes);
            return Ok(DecoderChunkOutcome::Chunk(Box::new(chunk)));
        }
    }

    fn seek_inner(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        let priming = self.codec.priming(self.demuxer.track_info().codec);
        match self.demuxer.seek(pos, priming)? {
            DemuxSeekOutcome::Landed {
                landed_at,
                landed_byte,
                preroll,
            } => {
                self.codec.flush()?;
                self.head_strip = HeadStrip::default();
                self.zero_frame_count = 0;
                self.pending_seek_target = Some(pos);
                self.frame_offset = self.spec.frame_at(landed_at).unwrap_or(u64::MAX);
                self.resync_frame_offset_to_pts = true;
                self.timeline_gap_frames = 0;
                Ok(DecoderSeekOutcome::Landed {
                    landed_byte,
                    landed_at,
                    preroll,
                    landed_frame: self.frame_offset,
                })
            }
            DemuxSeekOutcome::PastEof { duration } => {
                self.codec.flush()?;
                self.head_strip = HeadStrip::default();
                self.zero_frame_count = 0;
                Ok(DecoderSeekOutcome::PastEof { duration })
            }
        }
    }
}

impl<D, C, S> Decoder for ComposedDecoder<D, C, S>
where
    D: Demuxer + 'static,
    C: FrameCodec,
    S: HasPool<f32> + Send + Sync + 'static,
{
    fn blender_profile(&self) -> BlenderProfile {
        BlenderProfile::new(self.spec)
    }

    fn duration(&self) -> Option<Duration> {
        self.duration
    }

    fn media_origin(&self) -> Option<Duration> {
        self.source_media_origin()
    }

    fn initial_pcm_origin(&self) -> Option<PcmOrigin> {
        match self.origin {
            PcmOriginState::Captured(origin) => Some(origin),
            PcmOriginState::Observing(_) | PcmOriginState::Sealed => None,
        }
    }

    fn flush_reader_signals(&mut self) {
        if let Some(hooks) = self.hooks.as_mut() {
            hooks.flush();
        }
    }

    fn metadata(&self) -> TrackMetadata {
        TrackMetadata::default()
    }

    fn next_chunk(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        loop {
            self.prepare_next_chunk();
            match self.next_chunk_prepared()? {
                DecoderChunkOutcome::Pending(PendingReason::Retry) => {}
                outcome => return Ok(outcome),
            }
        }
    }

    fn next_chunk_prepared(&mut self) -> DecodeResult<DecoderChunkOutcome> {
        if matches!(self.prefill, Prefill::Buffered(_)) {
            let Prefill::Buffered(result) = std::mem::replace(&mut self.prefill, Prefill::Complete)
            else {
                unreachable!();
            };
            let outcome = result?;
            if matches!(outcome, DecoderChunkOutcome::Pending(_)) {
                self.prefill = Prefill::Needed;
            }
            self.emit_chunk_signal(&outcome);
            return Ok(outcome);
        }
        let outcome = self.decode_prepared()?;
        if matches!(outcome, DecoderChunkOutcome::Pending(_)) {
            self.prefill = Prefill::Needed;
        }
        self.emit_chunk_signal(&outcome);
        Ok(outcome)
    }

    fn has_prepared_result(&self) -> bool {
        matches!(
            self.prefill,
            Prefill::Buffered(
                Ok(DecoderChunkOutcome::Chunk(_) | DecoderChunkOutcome::Eof) | Err(_)
            )
        ) || matches!(self.output, Some(Err(_)))
    }

    fn prepare_next_chunk(&mut self) {
        if self.has_prepared_result() {
            return;
        }
        if matches!(self.prefill, Prefill::Buffered(_)) {
            self.prefill = Prefill::Needed;
        }
        if let Err(error) = self.demuxer.prepare_frame() {
            self.output = Some(Err(error));
            return;
        }
        if self.output.is_none() {
            let mut buffer = self.pools.get::<f32>();
            self.output = Some(self.codec.prepare_output(&mut buffer).map(|()| buffer));
        }
        if matches!(self.prefill, Prefill::Needed) {
            self.prefill = Prefill::Buffered(self.decode_prepared());
        }
    }

    fn seek(&mut self, pos: Duration) -> DecodeResult<DecoderSeekOutcome> {
        if !pos.is_zero() && matches!(self.origin, PcmOriginState::Observing(_)) {
            self.origin = PcmOriginState::Sealed;
        }
        let outcome = self.seek_inner(pos)?;
        self.prefill = Prefill::Needed;
        if matches!(self.output, Some(Err(_))) {
            self.output = None;
        }
        self.emit_seek_signal(&outcome);
        Ok(outcome)
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }

    /// The frames between a packet's timestamp and the PCM the decoder has
    /// actually produced for it.
    ///
    /// A codec's PTS bias and observed jumps may explain only part of its
    /// algorithmic strip. The directly observed strip remains authoritative
    /// when output timestamps already account for the removed prefix, including
    /// after a seek. Taking the maximum keeps variant-splice offsets consistent.
    fn timeline_gap_frames(&self) -> u64 {
        let modelled = self
            .codec
            .timestamp_bias_frames()
            .saturating_add(self.timeline_gap_frames);
        self.head_strip.frames().max(modelled)
    }

    fn update_byte_len(&self, len: u64) {
        if let Some(handle) = &self.byte_len_handle {
            handle.store(len, Ordering::Release);
        }
    }

    delegate::delegate! {
        to self.codec {
            #[expr(AudioCodec::encoder_priming_frames(codec).saturating_add($))]
            #[call(decoder_algo_delay)]
            fn default_priming_frames(&self, codec: AudioCodec) -> u64;
            fn track_info(&self) -> crate::DecoderTrackInfo;
        }
    }
}

#[cfg(test)]
impl DecoderRuntime<crate::test_pools::TestPools> {
    /// Test-only runtime with a crate-local typed pool region.
    pub(crate) fn for_test() -> Self {
        Self {
            pools: crate::test_pools::pools(),
            byte_len_handle: None,
            hooks: None,
        }
    }
}
