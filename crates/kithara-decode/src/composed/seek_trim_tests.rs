use std::{num::NonZeroU32, sync::atomic::Ordering};

use kithara_platform::{sync::Arc, time::Duration};
use kithara_signal::AudioSpec;
use kithara_stream::AudioCodec;
use kithara_test_fixtures::{mock_fixtures::zero_packet, unit_fixtures::trim_silence};
use kithara_test_utils::kithara;

use super::{test_counting_codec::CountingCodec, test_stub_codec::LaggedQueueCodec, *};
use crate::{
    demuxer::{DemuxOutcome, Frame, TrackInfo},
    traits::Decoder,
};

fn test_spec(sample_rate: u32) -> AudioSpec {
    AudioSpec::new(
        1,
        NonZeroU32::new(sample_rate).expect("test sample rate is non-zero"),
    )
}

fn test_duration(sample_rate: u32, frames: u64) -> Duration {
    test_spec(sample_rate)
        .duration_for(frames)
        .expect("test duration is representable")
}

fn test_frames(sample_rate: u32, duration: Duration) -> usize {
    test_spec(sample_rate)
        .frames_for(duration)
        .expect("test frame count is representable")
        .get()
}

fn test_frame(sample_rate: u32, timestamp: Duration) -> u64 {
    test_spec(sample_rate)
        .frame_at(timestamp)
        .expect("test frame is representable")
}

struct ThreeFrameDemuxer {
    track: TrackInfo,
    held: Vec<u8>,
    idx: usize,
}

#[derive(Clone, Copy)]
struct BoundaryFrame {
    duration: Duration,
    pts: Duration,
}

struct BoundaryFrameDemuxer {
    track: TrackInfo,
    frames: Vec<BoundaryFrame>,
    held: Vec<u8>,
    idx: usize,
}

#[derive(Clone, Copy)]
enum SeekTrimLayout {
    RegularPackets,
    RoundedPastTarget,
    UnknownPacketDuration,
}

impl Demuxer for ThreeFrameDemuxer {
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_millis(60))
    }
    fn next_frame(&mut self) -> DecodeResult<DemuxOutcome<'_>> {
        let pts_ms = match self.idx {
            0 => 0,
            1 => 20,
            2 => 40,
            _ => return Ok(DemuxOutcome::Eof),
        };
        self.idx += 1;
        Ok(DemuxOutcome::Frame(Frame {
            pts: Duration::from_millis(pts_ms),
            duration: Duration::from_millis(20),
            data: &self.held,
            packet_desc: &[],
        }))
    }
    fn track_info(&self) -> &TrackInfo {
        &self.track
    }
    reset_demuxer_seek!(idx = 0);
}

impl Demuxer for BoundaryFrameDemuxer {
    fn duration(&self) -> Option<Duration> {
        None
    }

    fn next_frame(&mut self) -> DecodeResult<DemuxOutcome<'_>> {
        let Some(frame) = self.frames.get(self.idx).copied() else {
            return Ok(DemuxOutcome::Eof);
        };
        self.idx += 1;
        Ok(DemuxOutcome::Frame(Frame {
            pts: frame.pts,
            duration: frame.duration,
            data: &self.held,
            packet_desc: &[],
        }))
    }

    fn track_info(&self) -> &TrackInfo {
        &self.track
    }

    reset_demuxer_seek!(idx = 0);
}

fn empty_track() -> TrackInfo {
    track_with_rate(consts::SAMPLE_RATE)
}

fn track_with_rate(sample_rate: u32) -> TrackInfo {
    TrackInfo {
        sample_rate,
        codec: AudioCodec::AacLc,
        channels: 2,
        extra_data: Vec::new(),
        duration: None,
        gapless: None,
    }
}

fn packet_duration() -> Duration {
    test_duration(consts::SAMPLE_RATE, u64::from(consts::PACKET_FRAMES))
}

fn regular_seek_frames() -> Vec<BoundaryFrame> {
    (0..consts::PACKET_COUNT)
        .map(|packet_idx| BoundaryFrame {
            pts: test_duration(
                consts::SAMPLE_RATE,
                packet_idx.saturating_mul(u64::from(consts::PACKET_FRAMES)),
            ),
            duration: packet_duration(),
        })
        .collect()
}

fn rounded_past_target_frames(target: Duration) -> Vec<BoundaryFrame> {
    vec![
        BoundaryFrame {
            pts: Duration::ZERO,
            duration: target.saturating_add(Duration::from_nanos(1)),
        },
        BoundaryFrame {
            pts: target,
            duration: packet_duration(),
        },
    ]
}

fn seek_frames_for(target: Duration, layout: SeekTrimLayout) -> Vec<BoundaryFrame> {
    match layout {
        SeekTrimLayout::RegularPackets => regular_seek_frames(),
        SeekTrimLayout::RoundedPastTarget => rounded_past_target_frames(target),
        SeekTrimLayout::UnknownPacketDuration => regular_seek_frames()
            .into_iter()
            .map(|frame| BoundaryFrame {
                duration: Duration::ZERO,
                ..frame
            })
            .collect(),
    }
}

#[kithara::test]
fn pre_target_frames_are_decoded_and_dropped_by_pending_seek_target(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    const FRAME_FRAMES: u32 = 882;

    let codec = CountingCodec::new(
        trim_silence.clone(),
        AudioSpec::new(2, NonZeroU32::new(44_100).expect("test rate")),
        FRAME_FRAMES,
    );
    let calls = Arc::clone(&codec.decode_calls);
    let demuxer = ThreeFrameDemuxer {
        track: empty_track(),
        idx: 0,
        held: zero_packet.to_vec(),
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    let _ = decoder.seek(Duration::from_millis(30)).expect("BUG: seek");

    let outcome = decoder.next_chunk().expect("BUG: next_chunk");
    assert!(
        matches!(outcome, DecoderChunkOutcome::Chunk(_)),
        "expected Chunk, got {outcome:?}"
    );

    // WHY: 2 calls — frame[0] (end=20ms ≤ 30ms target) dropped by the guard; frame[1] (end=40ms) emitted with leading 10ms trimmed.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "decode_frame must be called for pre-target frames so MDCT advances"
    );

    let packet = test_duration(consts::SAMPLE_RATE, 1_024);
    let codec = CountingCodec::new(
        trim_silence.clone(),
        AudioSpec::new(
            consts::CHANNELS,
            NonZeroU32::new(consts::SAMPLE_RATE).expect("test rate"),
        ),
        1_024,
    )
    .with_frames([363]);
    let demuxer = BoundaryFrameDemuxer {
        track: empty_track(),
        held: zero_packet.to_vec(),
        idx: 0,
        frames: vec![
            BoundaryFrame {
                pts: packet,
                duration: packet,
            },
            BoundaryFrame {
                pts: packet.saturating_mul(2),
                duration: packet,
            },
        ],
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());
    let first = decoder.next_chunk().expect("first head-trimmed chunk");
    assert!(matches!(first, DecoderChunkOutcome::Chunk(_)));
    let second = decoder.next_chunk().expect("second full packet");
    let DecoderChunkOutcome::Chunk(second) = second else {
        panic!("expected second PCM chunk, got {second:?}");
    };
    assert_eq!(
        second.meta.frame_offset, 2_048,
        "a decoder head trim must not compress every later packet's frame offset"
    );
    assert_eq!(
        decoder.timeline_gap_frames(),
        661,
        "the decoder must publish forward timestamp gaps without a second frame cursor"
    );
}

#[kithara::test]
fn composed_decoder_observes_head_strip_from_packet_duration(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    const SOURCE_SAMPLE_RATE: u32 = 24_000;
    const OUTPUT_SAMPLE_RATE: u32 = 48_000;
    const PACKET_DURATION: Duration = Duration::from_millis(20);
    const SUPPLIED_FRAMES: u32 = 960;

    assert_eq!(
        test_frames(OUTPUT_SAMPLE_RATE, PACKET_DURATION),
        usize::try_from(SUPPLIED_FRAMES).expect("supplied frames fit in usize")
    );
    assert_eq!(
        test_frames(SOURCE_SAMPLE_RATE, PACKET_DURATION),
        480,
        "the fixture must distinguish source-rate from output-rate frames"
    );

    let codec = CountingCodec::new(
        trim_silence.clone(),
        AudioSpec::new(
            consts::CHANNELS,
            NonZeroU32::new(OUTPUT_SAMPLE_RATE).expect("test rate"),
        ),
        SUPPLIED_FRAMES,
    )
    .with_frames([480, 720]);
    let demuxer = BoundaryFrameDemuxer {
        track: track_with_rate(SOURCE_SAMPLE_RATE),
        held: zero_packet.to_vec(),
        idx: 0,
        frames: vec![
            BoundaryFrame {
                pts: Duration::ZERO,
                duration: PACKET_DURATION,
            };
            3
        ],
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    for packet_idx in 0..3 {
        let outcome = decoder.next_chunk().expect("packet must decode");
        assert!(
            matches!(outcome, DecoderChunkOutcome::Chunk(_)),
            "packet {packet_idx} must emit a chunk, got {outcome:?}"
        );
    }

    assert_eq!(
        decoder.timeline_gap_frames, 0,
        "constant packet PTS must keep the modelled timeline gap out of the result"
    );
    assert_eq!(
        decoder.timeline_gap_frames(),
        720,
        "the composed decoder must publish the head strip measured in output frames"
    );
}

#[kithara::test]
fn fully_trimmed_seek_packet_is_dropped_before_target_chunk(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    const SAMPLE_RATE: u32 = 44_100;
    const PACKET_FRAMES: u32 = 1024;

    let target = test_duration(SAMPLE_RATE, u64::from(PACKET_FRAMES));
    let rounded_past_target = target.saturating_add(Duration::from_nanos(1));
    let codec = CountingCodec::new(
        trim_silence.clone(),
        AudioSpec::new(2, NonZeroU32::new(SAMPLE_RATE).expect("test rate")),
        PACKET_FRAMES,
    );
    let calls = Arc::clone(&codec.decode_calls);
    let demuxer = BoundaryFrameDemuxer {
        track: empty_track(),
        held: zero_packet.to_vec(),
        idx: 0,
        frames: vec![
            BoundaryFrame {
                pts: Duration::ZERO,
                duration: rounded_past_target,
            },
            BoundaryFrame {
                pts: target,
                duration: rounded_past_target,
            },
        ],
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    let _ = decoder.seek(target).expect("BUG: seek");
    let outcome = decoder.next_chunk().expect("BUG: next_chunk");
    let DecoderChunkOutcome::Chunk(chunk) = outcome else {
        panic!("expected Chunk, got {outcome:?}");
    };

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the fully pre-target packet must be decode-discarded before emitting"
    );
    assert_eq!(chunk.meta.frame_offset, u64::from(PACKET_FRAMES));
    assert_eq!(chunk.meta.timestamp, target);
}

#[kithara::test]
fn queue_codec_keeps_seek_target_until_straddling_pcm_arrives(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    const FRAME_FRAMES: u32 = 882;

    let target = Duration::from_millis(10);
    let codec = LaggedQueueCodec::new(
        trim_silence.clone(),
        AudioSpec::new(2, NonZeroU32::new(44_100).expect("test rate")),
        FRAME_FRAMES,
    );
    let demuxer = ThreeFrameDemuxer {
        track: empty_track(),
        idx: 0,
        held: zero_packet.to_vec(),
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    let _ = decoder.seek(target).expect("BUG: seek");
    let outcome = decoder.next_chunk().expect("BUG: next_chunk");
    let DecoderChunkOutcome::Chunk(chunk) = outcome else {
        panic!("expected Chunk, got {outcome:?}");
    };

    assert_eq!(chunk.meta.timestamp, target);
    assert_eq!(chunk.meta.frames, FRAME_FRAMES / 2);
    assert_eq!(
        decoder.timeline_gap_frames(),
        0,
        "buffered PCM must not count as discarded leading frames"
    );
}

#[kithara::test]
#[case::packet_boundary(
    test_duration(consts::SAMPLE_RATE, u64::from(consts::PACKET_FRAMES)),
    SeekTrimLayout::RegularPackets
)]
#[case::mid_packet(
    test_duration(consts::SAMPLE_RATE, u64::from(consts::PACKET_FRAMES) + 512),
    SeekTrimLayout::RegularPackets
)]
#[case::unknown_packet_duration(
    test_duration(consts::SAMPLE_RATE, 512),
    SeekTrimLayout::UnknownPacketDuration
)]
#[case::rounding_hair_past_boundary(
    test_duration(consts::SAMPLE_RATE, u64::from(consts::PACKET_FRAMES))
        .saturating_add(Duration::from_nanos(1)),
    SeekTrimLayout::RoundedPastTarget
)]
#[case::multiple_fully_pre_target_packets(
    test_duration(
        consts::SAMPLE_RATE,
        u64::from(consts::PACKET_FRAMES) * 3 + 512,
    ),
    SeekTrimLayout::RegularPackets
)]
fn seek_trim_lands_first_chunk_on_exact_target_frame(
    #[case] target: Duration,
    #[case] layout: SeekTrimLayout,
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    let codec = CountingCodec::new(
        trim_silence.clone(),
        AudioSpec::new(
            consts::CHANNELS,
            NonZeroU32::new(consts::SAMPLE_RATE).expect("test rate"),
        ),
        consts::PACKET_FRAMES,
    );
    let demuxer = BoundaryFrameDemuxer {
        track: empty_track(),
        held: zero_packet.to_vec(),
        frames: seek_frames_for(target, layout),
        idx: 0,
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    let _ = decoder.seek(target).expect("BUG: seek");
    let outcome = decoder.next_chunk().expect("BUG: next_chunk");
    let DecoderChunkOutcome::Chunk(chunk) = outcome else {
        panic!("expected Chunk, got {outcome:?}");
    };
    let expected_frame = test_frame(consts::SAMPLE_RATE, target);
    let packet_offset = expected_frame % u64::from(consts::PACKET_FRAMES);
    let expected_frames = if packet_offset == 0 {
        consts::PACKET_FRAMES
    } else {
        consts::PACKET_FRAMES - u32::try_from(packet_offset).expect("packet offset fits in u32")
    };

    assert_eq!(chunk.meta.frame_offset, expected_frame);
    assert_eq!(chunk.meta.timestamp, target);
    assert_eq!(
        test_frame(consts::SAMPLE_RATE, chunk.meta.timestamp),
        expected_frame
    );
    assert!(
        chunk.meta.timestamp >= target,
        "first chunk timestamp {:?} before seek target {:?}",
        chunk.meta.timestamp,
        target
    );
    assert_eq!(chunk.meta.frames, expected_frames);
}

#[kithara::test]
fn seek_trim_uses_codec_output_rate_for_resampled_chunks(
    trim_silence: Vec<f32>,
    zero_packet: &'static [u8],
) {
    let source_packet_start = test_duration(consts::SAMPLE_RATE, u64::from(consts::PACKET_FRAMES));
    let target = test_duration(consts::SAMPLE_RATE, u64::from(consts::PACKET_FRAMES) + 512);
    let output_packet_frames = u32::try_from(frames_to_trim(
        Duration::ZERO,
        packet_duration(),
        consts::OUTPUT_SAMPLE_RATE,
    ))
    .expect("output packet frame count fits in u32");
    let codec = CountingCodec::new(
        trim_silence.clone(),
        AudioSpec::new(
            consts::CHANNELS,
            NonZeroU32::new(consts::OUTPUT_SAMPLE_RATE).expect("test rate"),
        ),
        output_packet_frames,
    );
    let demuxer = BoundaryFrameDemuxer {
        track: track_with_rate(consts::SAMPLE_RATE),
        held: zero_packet.to_vec(),
        frames: regular_seek_frames(),
        idx: 0,
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());

    let _ = decoder.seek(target).expect("BUG: seek");
    let outcome = decoder.next_chunk().expect("BUG: next_chunk");
    let DecoderChunkOutcome::Chunk(chunk) = outcome else {
        panic!("expected Chunk, got {outcome:?}");
    };
    let expected_trim = frames_to_trim(source_packet_start, target, consts::OUTPUT_SAMPLE_RATE);
    let expected_frames =
        output_packet_frames - u32::try_from(expected_trim).expect("trim fits in u32");
    let expected_frame = test_frame(consts::OUTPUT_SAMPLE_RATE, target);

    assert_eq!(
        chunk.meta.spec.sample_rate.get(),
        consts::OUTPUT_SAMPLE_RATE
    );
    assert_eq!(chunk.meta.frame_offset, expected_frame);
    assert_eq!(chunk.meta.timestamp, target);
    assert_eq!(chunk.meta.frames, expected_frames);
}
