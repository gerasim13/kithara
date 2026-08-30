use kithara_bufpool::{OverallBudget, PoolConfig, PoolRegion, pool_schema};
use kithara_encode::{EncoderFactory, PackagedEncodeRequest};
use kithara_stream::{AudioCodec, ContainerFormat, MediaInfo};
use kithara_test_macros as kithara;

use crate::{
    fmp4::{GaplessEncoding, mux_audio_track},
    signal::{Pcm, Wave},
};

struct Consts;

pool_schema! {
    FixturePools {
        bytes: u8,
        samples: f32,
    }
}

fn pools() -> PoolRegion<FixturePools> {
    FixturePools::builder(OverallBudget(64 * 1024 * 1024))
        .bytes(PoolConfig::builder().max_buffers(32).build())
        .samples(PoolConfig::builder().max_buffers(8).build())
        .build()
        .unwrap_or_else(|error| panic!("kithara-test-fixtures: pool region failed: {error}"))
}

impl Consts {
    const AAC_HE_BIT_RATE: u64 = 64_000;
    const AAC_HE_V2_BIT_RATE: u64 = 32_000;
    const CHANNELS: u16 = 2;
    const ONE_SECOND_FRAMES: usize = 44_100;
    const SAMPLE_RATE: u32 = 44_100;
}

/// A saw packaged as a single-segment fMP4 body, the shape a browser hands to
/// `WebCodecs` when it opens an HE-AAC stream: init segment followed by the
/// media segment, concatenated.
///
/// Embedded because the browser suite reads it and wasm has no store.
#[kithara::asset(ext = "mp4", content_type = "audio/mp4", embed)]
#[case::v1(AudioCodec::AacHe, Consts::AAC_HE_BIT_RATE)]
#[case::v2(AudioCodec::AacHeV2, Consts::AAC_HE_V2_BIT_RATE)]
fn he_aac(codec: AudioCodec, bit_rate: u64) -> Vec<u8> {
    let frame_samples = EncoderFactory::frame_samples(codec).unwrap_or_else(|error| {
        panic!("kithara-test-fixtures: {codec:?} has no packaged frame size: {error}")
    });
    let packets = Consts::ONE_SECOND_FRAMES.div_ceil(frame_samples);
    let pcm = Pcm::new(
        Consts::SAMPLE_RATE,
        Consts::CHANNELS,
        packets * frame_samples,
        Wave::Sawtooth,
    );
    let media_info = MediaInfo::builder()
        .codec(codec)
        .container(ContainerFormat::Fmp4)
        .sample_rate(Consts::SAMPLE_RATE)
        .channels(Consts::CHANNELS)
        .build();

    let track = EncoderFactory::encode_packaged(
        &pools(),
        &PackagedEncodeRequest::builder()
            .pcm(&pcm)
            .media_info(media_info)
            .timescale(Consts::SAMPLE_RATE)
            .bit_rate(bit_rate)
            .packets_per_segment(packets)
            .encoder_delay(0)
            .trailing_delay(0)
            .build(),
    )
    .unwrap_or_else(|error| panic!("kithara-test-fixtures: {codec:?} encode failed: {error}"));

    Vec::from(
        mux_audio_track(&track, GaplessEncoding::None).unwrap_or_else(|error| {
            panic!("kithara-test-fixtures: {codec:?} fMP4 mux failed: {error}")
        }),
    )
}
