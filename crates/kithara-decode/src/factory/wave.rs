use std::{
    io::{Seek, SeekFrom},
    sync::atomic::Ordering,
};

use kithara_bufpool::HasPool;
use kithara_resampler::ResamplerBackend;
use symphonia_core::{
    formats::FormatOptions,
    io::{MediaSourceStream, MediaSourceStreamOptions, ReadBytes},
};
use symphonia_format_riff::WavReader;

use super::{
    build::finish,
    probe::{skip_id3_tags, sniff_wav_format},
};
use crate::{
    DecodeError, DecodeResult, Decoder, DecoderConfig,
    pcm::PcmCodec,
    symphonia::{SymphoniaDemuxer, adapter::ReadSeekAdapter},
    traits::BoxedSource,
};

pub(super) fn build_wave_decoder(
    mut source: BoxedSource,
    mut config: DecoderConfig<
        impl ResamplerBackend,
        impl HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    >,
) -> DecodeResult<Box<dyn Decoder>> {
    let audio_start = skip_id3_tags(&mut source)?;
    let (_, block_align) = sniff_wav_format(&mut source)?;
    source.seek(SeekFrom::Start(0))?;
    let control = config
        .reader_control
        .clone()
        .unwrap_or_else(|| kithara_stream::ReaderControl::new(None));
    control.require_blocking();
    let adapter = ReadSeekAdapter::new(source, config.byte_len_handle.take(), false)
        .with_control(control.clone());
    let byte_len = adapter.byte_len_handle();
    let byte_pos = adapter.byte_pos_handle();
    let seek_enabled = adapter.seek_enabled_handle();
    let mut stream = MediaSourceStream::new(Box::new(adapter), MediaSourceStreamOptions::default());
    stream
        .ignore_bytes(audio_start)
        .map_err(DecodeError::backend)?;
    let reader =
        WavReader::try_new(stream, FormatOptions::default()).map_err(DecodeError::backend)?;
    seek_enabled.store(true, Ordering::Release);
    let demuxer = SymphoniaDemuxer::from_reader_with_layout(
        Box::new(reader),
        Some(byte_pos),
        config.byte_map.take(),
        Some(block_align),
        control,
    )?;
    let codec = PcmCodec::open(&demuxer.native_params)?;
    config.byte_len_handle = Some(byte_len);
    finish(demuxer, codec, config)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use kithara_platform::time::Duration;
    use kithara_resampler::NoResamplerBackend;
    use kithara_test_fixtures::assets::signal_profile_wav_pcm_f32le_192000_2ch_32bit;
    use kithara_test_utils::kithara;

    use super::build_wave_decoder;
    use crate::{
        DecoderChunkOutcome, DecoderConfig,
        factory::probe::wav_data_range,
        test_pools::{TestPools, pools},
        traits::BoxedSource,
    };

    #[kithara::test(native, flash(false))]
    fn native_wave_preserves_every_float_bit_and_seek_position() {
        let bytes = signal_profile_wav_pcm_f32le_192000_2ch_32bit().bytes();
        let mut source: BoxedSource = Box::new(Cursor::new(bytes));
        let range = wav_data_range(&mut source).expect("immutable WAV payload");
        let payload =
            &bytes[usize::try_from(range.start).unwrap()..usize::try_from(range.end).unwrap()];
        let config: DecoderConfig<NoResamplerBackend, TestPools> =
            DecoderConfig::builder().pools(pools()).build();
        let mut decoder = build_wave_decoder(source, config).expect("native lossless WAV");
        let spec = decoder.spec();
        assert_eq!(spec.sample_rate.get(), 192_000);
        assert_eq!(spec.channels, 2);
        let mut offset = 0;
        loop {
            match decoder.next_chunk().expect("lossless PCM") {
                DecoderChunkOutcome::Chunk(chunk) => {
                    assert_eq!(chunk.meta.frame_offset, u64::try_from(offset / 8).unwrap());
                    for sample in chunk.samples.iter() {
                        assert_eq!(
                            sample.to_bits(),
                            u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap())
                        );
                        offset += 4;
                    }
                }
                DecoderChunkOutcome::Pending(_) => panic!("resident PCM must be ready"),
                DecoderChunkOutcome::Eof => break,
            }
        }
        assert_eq!(offset, payload.len());
        let target = Duration::from_secs(1);
        decoder.seek(target).expect("native PCM seek");
        let DecoderChunkOutcome::Chunk(sought) = decoder.next_chunk().expect("sought PCM") else {
            panic!("in-range PCM seek produces samples");
        };
        let frame = spec.frame_at(sought.meta.timestamp).expect("native frame");
        assert_eq!(frame, 192_000);
        let start = usize::try_from(frame).unwrap() * 8;
        for (index, sample) in sought.samples.iter().enumerate() {
            let at = start + index * 4;
            assert_eq!(
                sample.to_bits(),
                u32::from_le_bytes(payload[at..at + 4].try_into().unwrap())
            );
        }
    }
}
