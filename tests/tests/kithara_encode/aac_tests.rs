use kithara::{
    self,
    bufpool::{BytePool, SamplePool},
    stream::{AudioCodec, ContainerFormat, MediaInfo},
};
use kithara_encode::{EncoderFactory, PackagedEncodeRequest};
use kithara_integration_tests::encode_test_pcm::SawtoothPcmFixture;

#[kithara::test]
fn encode_packaged_aac_happy_path_emits_monotonic_access_units() {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;

    let frame_samples = EncoderFactory::frame_samples(AudioCodec::AacLc)
        .expect("BUG: AacLc must be supported by the packaged encoder");
    let total_frames = 4 * frame_samples;
    let pcm = SawtoothPcmFixture::new(total_frames, SAMPLE_RATE, CHANNELS);
    let media_info = MediaInfo::builder()
        .codec(AudioCodec::AacLc)
        .container(ContainerFormat::Fmp4)
        .build();

    let encoded = EncoderFactory::encode_packaged(
        &PackagedEncodeRequest::for_pools(BytePool::default(), SamplePool::default())
            .media_info(media_info)
            .pcm(&pcm)
            .timescale(SAMPLE_RATE)
            .bit_rate(128_000)
            .packets_per_segment(2)
            .encoder_delay(0)
            .trailing_delay(0)
            .build(),
    )
    .unwrap_or_else(|error| panic!("encode_packaged(AacLc) failed: {error}"));

    assert_eq!(encoded.media_info.codec, Some(AudioCodec::AacLc));
    assert_eq!(encoded.media_info.container, Some(ContainerFormat::Fmp4));
    assert_eq!(encoded.media_info.sample_rate, Some(SAMPLE_RATE));
    assert_eq!(encoded.media_info.channels, Some(CHANNELS));
    assert_eq!(encoded.timescale, SAMPLE_RATE);
    assert_eq!(encoded.bit_rate, 128_000);
    assert_eq!(encoded.packets_per_segment, 2);
    assert!(encoded.codec_config.is_empty());
    assert!(
        encoded.access_units.len() >= 2,
        "expected multiple AAC access units, got {}",
        encoded.access_units.len()
    );

    let mut expected_pts = None;
    for unit in &encoded.access_units {
        assert!(!unit.bytes.is_empty(), "access unit payload is empty");
        assert_eq!(unit.pts, unit.dts, "AAC should not reorder audio packets");
        assert_eq!(
            unit.duration,
            u32::try_from(frame_samples).expect("AAC frame size fits u32"),
            "AAC-LC packets should use the natural frame duration"
        );

        if let Some(expected_pts) = expected_pts {
            assert_eq!(
                unit.pts, expected_pts,
                "AAC packet timestamps should be contiguous"
            );
        } else {
            assert_eq!(unit.pts, 0, "AAC timeline should start at zero");
        }
        expected_pts = Some(unit.pts + u64::from(unit.duration));
    }
}

#[kithara::test]
fn encode_packaged_aac_he_reuses_injected_byte_pool() {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;

    let frame_samples = EncoderFactory::frame_samples(AudioCodec::AacHe)
        .expect("BUG: AacHe must be supported by the packaged encoder");
    let pcm = SawtoothPcmFixture::new(4 * frame_samples, SAMPLE_RATE, CHANNELS);
    let byte_pool = BytePool::new(1, 0);
    let encode = || {
        EncoderFactory::encode_packaged(
            &PackagedEncodeRequest::for_pools(byte_pool.clone(), SamplePool::default())
                .pcm(&pcm)
                .media_info(
                    MediaInfo::builder()
                        .codec(AudioCodec::AacHe)
                        .container(ContainerFormat::Fmp4)
                        .build(),
                )
                .timescale(SAMPLE_RATE)
                .bit_rate(64_000)
                .packets_per_segment(2)
                .encoder_delay(0)
                .trailing_delay(0)
                .build(),
        )
        .unwrap_or_else(|error| panic!("encode_packaged(AacHe) failed: {error}"))
    };

    let first = encode();
    let after_first = byte_pool.stats();
    let second = encode();
    let after_second = byte_pool.stats();

    assert!(!first.access_units.is_empty());
    assert!(!second.access_units.is_empty());
    assert_eq!(after_second.alloc_misses, after_first.alloc_misses);
    assert!(
        after_second.home_hits + after_second.steal_hits
            > after_first.home_hits + after_first.steal_hits,
        "second encode did not reuse the injected byte pool"
    );
}

#[kithara::test]
fn encode_packaged_aac_lc_reuses_injected_conversion_pools() {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;

    let frame_samples = EncoderFactory::frame_samples(AudioCodec::AacLc)
        .expect("BUG: AacLc must be supported by the packaged encoder");
    let pcm = SawtoothPcmFixture::new(4 * frame_samples, SAMPLE_RATE, CHANNELS);
    let byte_pool = BytePool::new(1, 0);
    let sample_pool = SamplePool::new(1, 0);
    let encode = || {
        EncoderFactory::encode_packaged(
            &PackagedEncodeRequest::for_pools(byte_pool.clone(), sample_pool.clone())
                .pcm(&pcm)
                .media_info(
                    MediaInfo::builder()
                        .codec(AudioCodec::AacLc)
                        .container(ContainerFormat::Fmp4)
                        .build(),
                )
                .timescale(SAMPLE_RATE)
                .bit_rate(128_000)
                .packets_per_segment(2)
                .encoder_delay(0)
                .trailing_delay(0)
                .build(),
        )
        .unwrap_or_else(|error| panic!("encode_packaged(AacLc) failed: {error}"))
    };

    let first = encode();
    let byte_after_first = byte_pool.stats();
    let sample_after_first = sample_pool.stats();
    let second = encode();
    let byte_after_second = byte_pool.stats();
    let sample_after_second = sample_pool.stats();

    assert!(!first.access_units.is_empty());
    assert!(!second.access_units.is_empty());
    assert_eq!(
        byte_after_second.alloc_misses,
        byte_after_first.alloc_misses
    );
    assert_eq!(
        sample_after_second.alloc_misses,
        sample_after_first.alloc_misses
    );
    assert!(
        byte_after_second.home_hits + byte_after_second.steal_hits
            > byte_after_first.home_hits + byte_after_first.steal_hits,
        "second encode did not reuse the injected byte pool"
    );
    assert!(
        sample_after_second.home_hits + sample_after_second.steal_hits
            > sample_after_first.home_hits + sample_after_first.steal_hits,
        "second encode did not reuse the injected sample pool"
    );
}
