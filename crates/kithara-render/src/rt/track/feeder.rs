use std::ops::Range;

use kithara_audio::FailureSource;
use kithara_bufpool::{HasPool, PoolError, PoolRegion};
use kithara_decode::TrackMetadata;
use kithara_platform::{maybe_send::WasmSend, sync::Arc, time::Duration};
use kithara_signal::{AudioChunk, AudioSpec, SegmentId, SessionFrame};

use super::PcmConsumer;
use crate::{LaneFrame, bridge::SlotMark, worker::PcmPacket};

/// Owns at most one popped packet, including a packet the reverse ring refused.
pub struct PlayerResource {
    src: Arc<str>,
    consumer: WasmSend<PcmConsumer>,
    packet: Option<PcmPacket>,
    offset: usize,
    lane: LaneFrame,
    position: Duration,
    mapped: bool,
    awaiting_segment: bool,
    eof: bool,
    failed: Option<FailureSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    Full { frames: usize },
    Partial { frames: usize },
    Eof,
    Failed(FailureSource),
}

impl PlayerResource {
    /// Wraps a prepared packet receiver without calling its source.
    ///
    /// # Errors
    /// The resource signature shares the pool error boundary with deck construction.
    pub fn new<S>(
        consumer: PcmConsumer,
        src: Arc<str>,
        _pools: &PoolRegion<S>,
    ) -> Result<Self, PoolError>
    where
        S: HasPool<f32>,
    {
        let position = consumer.receiver.position();
        Ok(Self {
            src,
            consumer: WasmSend::new(consumer),
            packet: None,
            offset: 0,
            lane: LaneFrame {
                segment: SegmentId::FIRST,
                frame: 0,
            },
            position,
            mapped: false,
            awaiting_segment: false,
            eof: false,
            failed: None,
        })
    }

    #[must_use]
    pub fn src(&self) -> &Arc<str> {
        &self.src
    }

    #[must_use]
    pub fn spec(&self) -> AudioSpec {
        self.consumer.get().receiver.spec()
    }

    #[must_use]
    pub fn metadata(&self) -> &TrackMetadata {
        self.consumer.get().receiver.metadata()
    }

    #[must_use]
    pub fn duration(&self) -> f64 {
        self.consumer
            .get()
            .receiver
            .duration()
            .map_or(0.0, |duration| duration.as_secs_f64())
    }

    #[must_use]
    pub fn decoded_frontier(&self) -> f64 {
        self.consumer
            .get()
            .receiver
            .decoded_frontier()
            .as_secs_f64()
    }

    #[must_use]
    pub fn cached_span(&self) -> f64 {
        self.consumer.get().receiver.cached_span().as_secs_f64()
    }

    pub(super) fn segment(&self) -> SegmentId {
        self.lane.segment
    }

    pub(super) fn position(&self) -> Duration {
        self.position
    }

    pub(super) fn set_playing(&mut self, playing: bool) {
        self.consumer.get_mut().receiver.set_playing(playing);
    }

    pub(super) fn mark(&self, session: SessionFrame) -> Option<SlotMark> {
        self.mapped.then_some(SlotMark {
            session,
            lane: self.lane,
            position: self.position,
        })
    }

    pub(super) fn select_segment(&mut self, segment: SegmentId) {
        if self.lane.segment != segment {
            self.lane = LaneFrame { segment, frame: 0 };
            self.mapped = false;
            self.eof = false;
            self.awaiting_segment = true;
        }
    }

    fn return_packet(&mut self) -> bool {
        let Some(packet) = self.packet.take() else {
            return true;
        };
        match self.consumer.get_mut().receiver.recycle(packet) {
            Ok(()) => {
                self.offset = 0;
                true
            }
            Err(packet) => {
                self.packet = Some(packet);
                false
            }
        }
    }

    /// Does not pop the current segment while silent, and never pops a newer one.
    pub(super) fn recycle_obsolete(&mut self, budget: &mut usize) {
        loop {
            if let Some(packet) = &self.packet {
                let older = matches!(packet, PcmPacket::Chunk(chunk) if chunk.meta.segment < self.lane.segment);
                let spent = match packet {
                    PcmPacket::Chunk(chunk) => self.offset >= chunk.frames(),
                    PcmPacket::Failed { .. } => true,
                };
                if !older && !spent {
                    return;
                }
                if older {
                    if *budget == 0 {
                        return;
                    }
                    *budget -= 1;
                }
                if !self.return_packet() {
                    if older {
                        *budget += 1;
                    }
                    return;
                }
            }
            let receiver = &mut self.consumer.get_mut().receiver;
            let Some(next) = receiver.peek() else {
                return;
            };
            if matches!(next, PcmPacket::Failed { .. }) || packet_segment(next) >= self.lane.segment || *budget == 0 {
                return;
            }
            self.packet = receiver.pop();
        }
    }

    pub(super) fn poll_end(&mut self, budget: &mut usize) -> Option<ReadOutcome> {
        self.recycle_obsolete(budget);
        if self.eof {
            return Some(ReadOutcome::Eof);
        }
        if let Some(kind) = self.failed {
            return Some(ReadOutcome::Failed(kind));
        }
        if self.packet.is_some() {
            return None;
        }
        let receiver = &mut self.consumer.get_mut().receiver;
        let Some(next) = receiver.peek() else {
            if receiver.is_closed() {
                self.failed = Some(FailureSource::ChannelClosed);
                return Some(ReadOutcome::Failed(FailureSource::ChannelClosed));
            }
            return None;
        };
        match next {
            PcmPacket::Chunk(chunk)
                if chunk.meta.segment == self.lane.segment
                    && chunk.meta.end_of_track
                    && chunk.frames() == 0 =>
            {
                self.lane.frame = chunk.meta.lane_frame;
                if let Some(position) = chunk_position(chunk, 0..0) {
                    self.position = position;
                    self.mapped = true;
                }
                self.eof = true;
            }
            PcmPacket::Failed { failure, .. } => {
                self.failed = Some(if !self.awaiting_segment {
                    FailureSource::Producer { failure: *failure }
                } else {
                    FailureSource::ProducerAfterSeek { failure: *failure }
                });
            }
            _ => return None,
        }
        receiver.set_position(self.position);
        self.packet = receiver.pop();
        let _ = self.return_packet();
        Some(self.failed.map_or(ReadOutcome::Eof, ReadOutcome::Failed))
    }

    pub(super) fn refresh_mark(&mut self, budget: &mut usize) {
        self.recycle_obsolete(budget);
        let next = self
            .packet
            .as_ref()
            .or_else(|| self.consumer.get().receiver.peek());
        if let Some(PcmPacket::Chunk(chunk)) = next
            && chunk.meta.segment == self.lane.segment
        {
            let offset = if self.packet.is_some() {
                self.offset
            } else {
                0
            };
            self.mapped = if let Some((frame, position)) = chunk
                .meta
                .lane_frame
                .checked_add(u64::try_from(offset).unwrap_or(u64::MAX))
                .zip(chunk_position(chunk, offset..offset))
            {
                self.lane.frame = frame;
                self.position = position;
                true
            } else {
                false
            };
        }
        if self.mapped {
            self.consumer.get_mut().receiver.set_position(self.position);
        }
    }

    pub(super) fn read(
        &mut self,
        buffers: &mut [&mut [f32]],
        range: Range<usize>,
        budget: &mut usize,
    ) -> ReadOutcome {
        let [left, right, ..] = buffers else {
            return ReadOutcome::Full { frames: 0 };
        };
        let requested = range
            .len()
            .min(left.len().saturating_sub(range.start))
            .min(right.len().saturating_sub(range.start));
        let mut written = 0;
        while written < requested {
            if let Some(end) = self.poll_end(budget) {
                return if written == 0 {
                    end
                } else {
                    ReadOutcome::Partial { frames: written }
                };
            }
            if self.packet.is_none() {
                let receiver = &mut self.consumer.get_mut().receiver;
                if !receiver.peek().is_some_and(|packet| {
                    matches!(packet, PcmPacket::Chunk(chunk) if chunk.meta.segment == self.lane.segment && chunk.frames() > 0)
                }) {
                    break;
                }
                self.packet = receiver.pop();
                self.awaiting_segment = false;
                self.offset = 0;
            }
            let Some(PcmPacket::Chunk(chunk)) = &self.packet else {
                break;
            };
            let channels = usize::from(chunk.spec().channels);
            let count = chunk
                .frames()
                .saturating_sub(self.offset)
                .min(requested - written);
            if count == 0 {
                break;
            }
            for frame in 0..count {
                let input = (self.offset + frame) * channels;
                let output = range.start + written + frame;
                left[output] = chunk.samples[input];
                right[output] = chunk.samples[input + usize::from(channels > 1)];
            }
            let offset = self.offset;
            self.offset += count;
            written += count;
            self.mapped = if let Some((frame, position)) = chunk
                .meta
                .lane_frame
                .checked_add(u64::try_from(self.offset).unwrap_or(u64::MAX))
                .zip(chunk_position(chunk, offset..self.offset))
            {
                self.lane.frame = frame;
                self.position = position;
                true
            } else {
                false
            };
        }
        if self.mapped {
            self.consumer.get_mut().receiver.set_position(self.position);
        }
        ReadOutcome::Full { frames: written }
    }
}

fn packet_segment(packet: &PcmPacket) -> SegmentId {
    match packet {
        PcmPacket::Chunk(chunk) => chunk.meta.segment,
        PcmPacket::Failed { segment, .. } => *segment,
    }
}

fn chunk_position(chunk: &AudioChunk, range: Range<usize>) -> Option<Duration> {
    let source = chunk.meta.source_span?;
    let start = u64::try_from(range.start).ok()?;
    let end = u64::try_from(range.end).ok()?;
    source
        .for_output_range(start..end)?
        .position_at(end.checked_sub(start)?)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_signal::{AudioSpec, FrameCount, SampleCount, SourceSpan};
    use crate::worker::packet_tests::chunk;
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    #[case::samples(false)]
    #[case::mark(true)]
    fn an_adopted_segment_never_uses_a_held_obsolete_packet(#[case] check_mark: bool) {
        use crate::{test_pools::pools, worker::packet_tests::{PacketRing, chunk}};

        let spec = AudioSpec::new(1, NonZeroU32::new(44_100).expect("rate"));
        let old = SegmentId::FIRST;
        let intermediate = old.next();
        let current = intermediate.next();
        let mut ring = PacketRing::new(spec, Duration::from_secs(1), 2);
        let mut receiver = ring.receiver.take().expect("receiver");
        receiver.recycle(PcmPacket::Chunk(chunk(spec, old, 0, 0, &[1.0]))).expect("first reverse entry");
        ring.push(PcmPacket::Chunk(chunk(spec, old, 10, 10, &[1.0; 8])));
        ring.push(PcmPacket::Chunk(chunk(spec, intermediate, 18, 18, &[1.0; 8])));
        let mut resource = PlayerResource::new(PcmConsumer::new(receiver), Arc::from("segments"), &pools()).expect("resource");
        resource.select_segment(intermediate);
        let mut budget = 1;
        resource.refresh_mark(&mut budget);
        assert_eq!(budget, 0, "this block spent its recycle budget");
        let mut previous_left = [0.0; 1];
        let mut previous_right = [0.0; 1];
        resource.read(&mut [&mut previous_left, &mut previous_right], 0..1, &mut budget);
        assert_eq!(previous_left, [1.0], "the intermediate packet is held before the next Adopt");
        ring.push(PcmPacket::Chunk(chunk(spec, current, 100, 1000, &[2.0; 8])));
        ring.push(PcmPacket::Chunk(chunk(spec, current, 108, 1008, &[2.0; 8])));
        assert!(resource.consumer.get_mut().receiver.recycle(
            PcmPacket::Chunk(chunk(spec, old, 0, 0, &[1.0]))).is_err(), "the reverse ring is full");
        resource.select_segment(current);
        let mut left = [0.0; 4];
        let mut right = [0.0; 4];
        let adopted_at = SessionFrame::new(700);
        resource.read(&mut [&mut left, &mut right], 0..4, &mut budget);
        if check_mark {
            assert_eq!(resource.mark(adopted_at), None, "an obsolete held packet cannot establish the adopted segment's frame or position");
        } else {
            assert_eq!(left, [0.0; 4], "no old-segment PCM may reach the mix after Adopt, even under full rings and exhausted budget");
            assert_eq!(right, left);
        }
        while ring.returned().is_some() {}
        budget = 2;
        resource.refresh_mark(&mut budget);
        resource.read(&mut [&mut left, &mut right], 0..4, &mut budget);
        assert_eq!(left, [2.0; 4]);
        assert_eq!(resource.mark(SessionFrame::new(704)), Some(SlotMark {
            session: SessionFrame::new(704),
            lane: LaneFrame { segment: current, frame: 104 },
            position: spec.duration_for(1004).expect("new segment position"),
        }));
    }

    #[kithara::test]
    #[case(44_100, 8_820)]
    #[case(48_000, 9_600)]
    #[case(96_000, 19_200)]
    fn a_pcm_packet_holds_200ms_of_frames(#[case] sample_rate: u32, #[case] expected: usize) {
        let spec = AudioSpec::new(2, NonZeroU32::new(sample_rate).expect("rate"));
        let duration = Duration::from_millis(200);
        let frames = spec.frames_for(duration).expect("200 ms geometry");
        let samples = spec.sample_count(frames).expect("stereo geometry");
        let packet = chunk(spec, SegmentId::FIRST, 0, 0, &vec![1.0; samples.get()]);
        assert_eq!(
            FrameCount::new(packet.frames()),
            FrameCount::new(expected)
        );
        assert_eq!(packet.meta.end_timestamp, duration);
    }

    #[kithara::test]
    fn an_interleaved_length_is_not_a_frame_count() {
        let spec = AudioSpec::new(2, NonZeroU32::new(48_000).expect("test rate is non-zero"));
        let frames = FrameCount::new(9_600);
        assert_eq!(
            spec.sample_count(frames),
            Ok(SampleCount::new(frames.get() * 2))
        );
    }

    #[kithara::test]
    fn partial_scratch_consumption_preserves_the_render_revision() {
        let rate = NonZeroU32::new(48_000).expect("fixture sample rate is non-zero");
        let source = SourceSpan::new(100, 130, rate, 10).map(|span| span.with_render_revision(7));
        let span = source.expect("source span");

        assert_eq!(
            span.for_output_range(0..4),
            SourceSpan::new(100, 112, rate, 4).map(|span| span.with_render_revision(7))
        );
        assert_eq!(span.for_output_range(4..10).map(SourceSpan::start), Some(112));
        assert_eq!(
            span.for_output_range(4..10),
            SourceSpan::new(112, 130, rate, 6).map(|span| span.with_render_revision(7))
        );
    }

    #[kithara::test]
    fn partial_source_frontier_is_independent_of_callback_partitions() {
        let rate = NonZeroU32::new(48_000).expect("sample rate");
        let mapping = std::num::NonZeroU64::new(3);
        let source = SourceSpan::new(100, 292, rate, 128)
            .map(|span| span.with_render_revision(7).with_mapping_revision(mapping));
        let whole = source.expect("source span");
        let expected = whole.for_output_range(0..2).expect("two output frames");
        let split = whole.for_output_range(1..128).expect("first output frame");
        let actual = split.for_output_range(0..1).expect("second output frame");
        assert_eq!(actual.end(), expected.end());
        assert_eq!(actual.end(), 103);
        assert_eq!(actual.render_revision(), 7);
        assert_eq!(actual.mapping_revision(), mapping);
    }
    #[kithara::test]
    fn zero_source_advance_keeps_mapping_identity_until_pcm_is_consumed() {
        let rate = NonZeroU32::new(48_000).expect("rate");
        let mapping = std::num::NonZeroU64::new(2);
        let source = SourceSpan::new(41, 41, rate, 32)
            .map(|span| span.with_render_revision(7).with_mapping_revision(mapping));
        let window = source.expect("source span");
        assert_eq!(
            window.for_output_range(0..16),
            source.and_then(|span| span.for_output_range(0..16))
        );
        assert_eq!(
            window.for_output_range(16..32),
            source.and_then(|span| span.for_output_range(16..32))
        );
        assert_eq!(
            window.for_output_range(16..32),
            source.and_then(|span| span.for_output_range(0..16))
        );
        assert_eq!(window.for_output_range(32..32).expect("exhausted window").output_frames(), 0);
    }
    #[kithara::test]
    #[case::zero_origin(0)]
    #[case::nonzero_origin(100)]
    fn nested_audio_source_window_preserves_original_rounding(#[case] origin: u64) {
        let rate = NonZeroU32::new(48_000).expect("rate");
        let mapping = std::num::NonZeroU64::new(3);
        let source = SourceSpan::new(origin, origin + 192, rate, 128)
            .expect("source span")
            .with_render_revision(7)
            .with_mapping_revision(mapping);
        let audio_read = source.for_output_range(0..127).expect("Audio partial read");
        assert_eq!(audio_read.end(), origin + 190);
        let consumed = audio_read.for_output_range(0..2).expect("Play partial consumption");
        assert_eq!(consumed.end(), origin + 3);
        assert_eq!(
            consumed,
            source.for_output_range(0..2).expect("direct slice")
        );
        assert_eq!(consumed.render_revision(), 7);
        assert_eq!(consumed.mapping_revision(), mapping);
    }
}

#[cfg(test)]
mod terminal_tests;
