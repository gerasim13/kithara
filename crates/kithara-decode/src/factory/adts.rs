use std::io::{Read, Seek, SeekFrom};

use kithara_bufpool::HasPool;
use kithara_resampler::ResamplerBackend;
use symphonia_codec_aac::AdtsReader;
use symphonia_core::{formats::FormatOptions, io::ReadBytes};

use super::{probe::skip_id3_tags, reader::open_reader};
use crate::{
    DecodeError, DecodeResult, DecoderConfig, symphonia::SymphoniaDemuxer, traits::BoxedSource,
};

#[cfg(android_backend)]
pub(super) fn build_adts_decoder<B, S>(
    source: BoxedSource,
    mut config: DecoderConfig<B, S>,
) -> DecodeResult<Box<dyn crate::Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    let (demuxer, extra_data) = open_adts(source, &mut config)?;
    let mut track = crate::demuxer::Demuxer::track_info(&demuxer).clone();
    track.extra_data = extra_data.to_vec();
    let codec = crate::android::AndroidCodec::open_with_config(&track)?;
    super::build::finish(demuxer, codec, config)
}

fn open_adts<B, S>(
    mut source: BoxedSource,
    config: &mut DecoderConfig<B, S>,
) -> DecodeResult<(SymphoniaDemuxer, [u8; 2])>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    let audio_start = skip_id3_tags(&mut source)?;
    let mut header = [0; 7];
    source.read_exact(&mut header)?;
    let config_bytes = audio_specific_config(header)?;
    source.seek(SeekFrom::Start(0))?;
    let demuxer = open_reader(source, config, None, |mut stream| {
        stream
            .ignore_bytes(audio_start)
            .map_err(DecodeError::backend)?;
        Ok(Box::new(
            AdtsReader::try_new(stream, FormatOptions::default()).map_err(DecodeError::backend)?,
        ))
    })?;
    Ok((demuxer, config_bytes))
}

fn audio_specific_config(header: [u8; 7]) -> DecodeResult<[u8; 2]> {
    let sync = u16::from_be_bytes([header[0], header[1]]);
    let frequency = (header[2] >> 2) & 0x0f;
    if sync & 0xfff6 != 0xfff0 || frequency >= 13 {
        return Err(DecodeError::InvalidData {
            detail: "invalid ADTS configuration header",
        });
    }
    let object_type = (header[2] >> 6) + 1;
    let channels = ((header[2] & 1) << 2) | (header[3] >> 6);
    Ok([
        (object_type << 3) | (frequency >> 1),
        (frequency << 7) | (channels << 3),
    ])
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, sync::atomic::AtomicU64};

    use kithara_platform::{sync::Arc, time::Duration};
    use kithara_resampler::NoResamplerBackend;
    use kithara_test_fixtures::assets::signal_aac_sine440_60s;
    use kithara_test_utils::kithara;

    use super::{audio_specific_config, open_adts};
    use crate::{
        CodecPriming, DecoderConfig,
        demuxer::{DemuxOutcome, DemuxSeekOutcome, Demuxer},
        test_pools::{TestPools, pools},
    };

    #[kithara::test(native, flash(false))]
    #[case::known(false)]
    #[case::unknown(true)]
    fn adts_packets_keep_the_core_sample_clock_without_a_published_length(
        #[case] unknown_length: bool,
    ) {
        let source = Box::new(Cursor::new(signal_aac_sine440_60s().bytes()));
        let mut config: DecoderConfig<NoResamplerBackend, TestPools> =
            DecoderConfig::builder().pools(pools()).build();
        if unknown_length {
            config.byte_len_handle = Some(Arc::new(AtomicU64::new(0)));
        }
        let (mut demuxer, asc) = open_adts(source, &mut config).expect("exact ADTS reader");
        let rate = u64::from(demuxer.track_info().sample_rate);
        assert_eq!((asc[0] >> 3) & 0x1f, 2);
        if unknown_length {
            assert!(demuxer.duration().is_none());
        }
        for packet in 0..400 {
            let DemuxOutcome::Frame(frame) = demuxer.next_frame().expect("ADTS access unit") else {
                panic!("resident ADTS input produces an access unit");
            };
            assert!(!frame.data.is_empty());
            let expected = Duration::from_nanos(packet * 1024 * 1_000_000_000 / rate);
            assert!(frame.pts.abs_diff(expected) <= Duration::from_nanos(1));
        }
        let target = Duration::from_secs(1);
        let DemuxSeekOutcome::Landed { landed_at, .. } = demuxer
            .seek(target, CodecPriming::default())
            .expect("ADTS seek")
        else {
            panic!("in-range ADTS seek lands");
        };
        let DemuxOutcome::Frame(frame) = demuxer.next_frame().expect("sought ADTS packet") else {
            panic!("resident ADTS seek produces an access unit");
        };
        assert_eq!(frame.pts, landed_at);
        assert!(frame.pts <= target);
        let source_frame = (frame.pts.as_nanos() * u128::from(rate) + 500_000_000) / 1_000_000_000;
        assert_eq!(
            source_frame % 1024,
            0,
            "seek retains the access-unit sample grid"
        );
    }

    #[kithara::test(native, flash(false))]
    #[case::stereo([0xff, 0xf1, 0x50, 0x80, 0, 0, 0], [0x12, 0x10])]
    #[case::program_config([0xff, 0xf1, 0x50, 0, 0, 0, 0], [0x12, 0])]
    fn adts_configuration_preserves_the_encoded_channel_configuration(
        #[case] header: [u8; 7],
        #[case] expected: [u8; 2],
    ) {
        assert_eq!(
            audio_specific_config(header).expect("valid header configuration"),
            expected
        );
    }

    #[kithara::test(native, flash(false))]
    #[case::sync([0, 0xf1, 0x50, 0x80, 0, 0, 0])]
    #[case::frequency([0xff, 0xf1, 0x74, 0x80, 0, 0, 0])]
    fn invalid_adts_configuration_is_rejected(#[case] header: [u8; 7]) {
        assert!(audio_specific_config(header).is_err());
    }
}
