use kithara_bufpool::SampleBuffer;
use kithara_platform::time::Duration;
use kithara_signal::AudioSpec;
use kithara_stream::PrerollHint;
use kithara_test_fixtures::mock_fixtures::zero_packet;
use kithara_test_utils::kithara;

use super::{
    ComposedDecoder, DecoderRuntime,
    seek_trim_tests::{BoundaryFrame, BoundaryFrameDemuxer, test_spec, track_with_rate},
    test_counting_codec::CountingCodec,
};
use crate::{
    CodecPriming, DecodeResult, Decoder, DecoderChunkOutcome,
    codec::FrameCodec,
    demuxer::{DemuxOutcome, DemuxSeekOutcome, Demuxer, TrackInfo},
};

struct NativeClock {
    inner: BoundaryFrameDemuxer,
    media_origin: Option<Duration>,
}

impl Demuxer for NativeClock {
    fn duration(&self) -> Option<crate::DecoderDuration> {
        Some(crate::DecoderDuration::MediaEnd(Duration::from_secs(2)))
    }
    fn media_origin(&self) -> Option<Duration> {
        self.media_origin
    }

    delegate::delegate! {
        to self.inner {
            fn next_frame(&mut self) -> DecodeResult<DemuxOutcome<'_>>;
            fn track_info(&self) -> &TrackInfo;
        }
    }

    fn seek(&mut self, target: Duration, _priming: CodecPriming) -> DecodeResult<DemuxSeekOutcome> {
        self.inner.idx = 0;
        Ok(DemuxSeekOutcome::Landed {
            landed_at: target,
            landed_byte: None,
            preroll: PrerollHint::NotNeeded,
        })
    }
}

struct RebasedCodec {
    inner: CountingCodec,
    rebase: Option<Duration>,
}

impl FrameCodec for RebasedCodec {
    fn media_time_rebase(&self) -> Option<Duration> {
        self.rebase
    }

    delegate::delegate! {
        to self.inner {
                fn decode_frame(
                    &mut self,
                    frame: crate::demuxer::Frame<'_>,
            metadata: crate::demuxer::PacketMetadata,
                    out: &mut SampleBuffer,
                ) -> DecodeResult<crate::codec::DecodedPacket>;
                fn flush(&mut self) -> DecodeResult<()>;
                fn spec(&self) -> AudioSpec;
        }
    }
}

#[kithara::test]
#[case::native_identity(Some(0), Some(0), Some(0))]
#[case::native_mpeg_shift(Some(1_105), Some(0), Some(1_105))]
#[case::codec_rebases_removed_head(Some(1_105), Some(529), Some(576))]
#[case::unknown_native_translation(None, Some(0), None)]
#[case::unknown_codec_translation(Some(1_105), None, None)]
#[case::unrepresentable_media_zero(Some(0), Some(529), None)]
fn media_zero_uses_native_translation_and_codec_rebase_without_observing_pcm(
    zero_packet: &'static [u8],
    #[case] native: Option<u64>,
    #[case] removed: Option<u64>,
    #[case] expected: Option<u64>,
) {
    let spec = test_spec(1_000);
    let samples = [0.25, -0.5];
    let demuxer = NativeClock {
        inner: BoundaryFrameDemuxer {
            track: track_with_rate(1_000),
            frames: vec![BoundaryFrame {
                pts: Duration::from_millis(1_200),
                duration: Duration::from_millis(2),
            }],
            held: zero_packet.to_vec(),
            idx: 0,
        },
        media_origin: native.map(Duration::from_millis),
    };
    let codec = RebasedCodec {
        inner: CountingCodec::new(samples.to_vec(), spec, 2),
        rebase: removed.map(Duration::from_millis),
    };
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());
    let expected = expected.map(Duration::from_millis);
    assert_eq!(decoder.media_origin(), expected);
    assert_eq!(
        decoder.duration(),
        Some(crate::DecoderDuration::MediaEnd(Duration::from_secs(2)))
    );
    assert_eq!(decoder.initial_pcm_origin(), None);
    assert_eq!(
        decoder.duration(),
        Some(crate::DecoderDuration::MediaEnd(Duration::from_secs(2)))
    );

    decoder
        .seek(Duration::from_millis(1_200))
        .expect("seek before original PCM");
    assert_eq!(decoder.media_origin(), expected);
    assert_eq!(decoder.initial_pcm_origin(), None);
    let DecoderChunkOutcome::Chunk(first) = decoder.next_chunk().expect("sought PCM") else {
        panic!("sought PCM chunk");
    };
    assert_eq!(first.meta.media_origin, expected);
    assert_eq!(first.meta.timestamp, Duration::from_millis(1_200));
    assert_eq!(first.meta.frame_offset, 1_200);
    assert_eq!(first.samples.as_ref(), samples.as_slice());
    assert_eq!(decoder.initial_pcm_origin(), None);

    decoder
        .seek(Duration::ZERO)
        .expect("return to original coordinate");
    let DecoderChunkOutcome::Chunk(replayed) = decoder.next_chunk().expect("replayed PCM") else {
        panic!("replayed PCM chunk");
    };
    assert_eq!(decoder.media_origin(), expected);
    assert_eq!(replayed.meta.media_origin, expected);
    assert_eq!(replayed.meta.timestamp, first.meta.timestamp);
    assert_eq!(replayed.samples.as_ref(), first.samples.as_ref());
}

/// Packet times of a track whose container timescale differs from its sample
/// rate: each one is rounded to a whole tick, so it sits up to half a tick off
/// the PCM lattice.
fn tick_rounded_frames(
    rate: u64,
    timescale: u64,
    packet_frames: u64,
    packets: u64,
) -> Vec<BoundaryFrame> {
    let ticks_to_time = |ticks: u64| Duration::from_nanos(ticks * 1_000_000_000 / timescale);
    let ticks_at = |packet: u64| (packet * packet_frames * timescale + rate / 2) / rate;
    (0..packets)
        .map(|packet| BoundaryFrame {
            pts: ticks_to_time(ticks_at(packet)),
            duration: ticks_to_time(ticks_at(packet + 1) - ticks_at(packet)),
        })
        .collect()
}

#[kithara::test]
#[case::aac_44k_in_90k(44_100, 90_000, 1_024, None)]
#[case::aac_44k_in_90k_after_seek(44_100, 90_000, 1_024, Some(Duration::from_millis(1_234)))]
#[case::aac_48k_in_90k(48_000, 90_000, 1_024, None)]
#[case::aac_48k_in_90k_after_seek(48_000, 90_000, 1_024, Some(Duration::from_millis(987)))]
#[case::aac_22k_in_90k(22_050, 90_000, 1_024, None)]
#[case::mp3_44k_in_90k_after_seek(44_100, 90_000, 1_152, Some(Duration::from_millis(2_001)))]
#[case::flac_44k_in_90k(44_100, 90_000, 4_096, None)]
#[case::opus_48k_in_48k(48_000, 48_000, 960, Some(Duration::from_millis(1_500)))]
fn tick_rounded_packet_times_keep_chunks_on_one_source_timeline(
    zero_packet: &'static [u8],
    #[case] rate: u32,
    #[case] timescale: u64,
    #[case] packet_frames: u32,
    #[case] seek: Option<Duration>,
) {
    const PACKETS: u64 = 256;
    let spec = test_spec(rate);
    let demuxer = BoundaryFrameDemuxer {
        track: track_with_rate(rate),
        frames: tick_rounded_frames(
            u64::from(rate),
            timescale,
            u64::from(packet_frames),
            PACKETS,
        ),
        held: zero_packet.to_vec(),
        idx: 0,
    };
    let codec = CountingCodec::new(
        vec![0.0; usize::try_from(packet_frames).expect("packet fits usize")],
        spec,
        packet_frames,
    );
    let mut decoder = ComposedDecoder::new(demuxer, codec, DecoderRuntime::for_test());
    if let Some(target) = seek {
        decoder.seek(target).expect("seek lands");
    }
    let mut previous: Option<kithara_signal::AudioChunkInfo> = None;
    let mut joined = 0;
    while let DecoderChunkOutcome::Chunk(chunk) = decoder.next_chunk().expect("decoded PCM") {
        if let Some(previous) = previous {
            let span = previous.source_geometry().expect("previous geometry");
            let next = chunk.meta.source_geometry().expect("next geometry");
            assert!(
                span.followed_by(next).is_some(),
                "chunk at frame {} ({:?}) does not continue the chunk at frame {} ({:?})",
                chunk.meta.frame_offset,
                chunk.meta.timestamp,
                previous.frame_offset,
                previous.timestamp,
            );
            joined += 1;
        }
        previous = Some(chunk.meta);
    }
    assert!(joined > 100, "only {joined} chunk joins were checked");
}
