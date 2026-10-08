use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;
use kithara_resampler::ResamplerBackend;
use kithara_stream::{AudioCodec, ByteMap, ContainerFormat};

#[cfg(feature = "ape")]
use super::build::create_ape;
use super::{
    build::finish,
    mpeg::build_mpeg_decoder,
    segment::{build_fmp4_segment_decoder, should_use_segment_aware},
};
use crate::{DecodeError, DecodeResult, Decoder, DecoderConfig, traits::BoxedSource};

pub(super) fn create<B, S>(
    source: BoxedSource,
    codec: AudioCodec,
    container: Option<ContainerFormat>,
    config: DecoderConfig<B, S>,
) -> DecodeResult<Box<dyn Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    #[cfg(feature = "ape")]
    if codec == AudioCodec::Ape {
        return create_ape(source, config);
    }
    if codec == AudioCodec::Mp3 && container == Some(ContainerFormat::Wav) {
        let gapless = config.gapless;
        let symphonia_config = crate::symphonia::SymphoniaConfig::builder()
            .gapless(gapless)
            .build();
        return build_mpeg_decoder(source, container, config, |demuxer| {
            use crate::{demuxer::Demuxer, symphonia::SymphoniaCodec};
            SymphoniaCodec::open_with_config(demuxer.track_info(), &symphonia_config)
        });
    }
    if should_use_segment_aware(codec, container, &config)
        && let Some(layout) = config.byte_map.clone()
    {
        return create_segment(source, codec, layout, config);
    }
    create_file_symphonia_universal(source, codec, container, config)
}

fn create_file_symphonia_universal<B, S>(
    mut source: BoxedSource,
    codec: AudioCodec,
    container: Option<ContainerFormat>,
    config: DecoderConfig<B, S>,
) -> DecodeResult<Box<dyn Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    use crate::{
        demuxer::Demuxer,
        gapless::scoped_probe,
        symphonia::{FileOpen, SymphoniaCodec, SymphoniaConfig, SymphoniaDemuxer},
    };

    tracing::debug!(
        ?codec,
        ?container,
        "file-symphonia: dispatching to ComposedDecoder<SymphoniaDemuxer, SymphoniaCodec>"
    );

    let probed_gapless = if config.gapless {
        scoped_probe(&mut *source, codec, &config.pools)?
    } else {
        None
    };

    let (mut demuxer, _byte_len) = SymphoniaDemuxer::open_file(
        source,
        FileOpen {
            container,
            hint: config.hint.clone(),
            byte_len_handle: config.byte_len_handle.clone(),
            byte_map: config.byte_map.clone(),
        },
    )?;
    if probed_gapless.is_some() {
        demuxer.set_gapless(probed_gapless);
    }
    let symphonia_config = SymphoniaConfig::builder().gapless(config.gapless).build();
    let codec_impl = if SymphoniaCodec::supports(codec) {
        SymphoniaCodec::open_with_config(demuxer.track_info(), &symphonia_config)?
    } else {
        SymphoniaCodec::open_native(&demuxer.native_params)?
    };
    finish(demuxer, codec_impl, config)
}

pub(super) fn create_segment<B, S>(
    source: BoxedSource,
    codec: AudioCodec,
    layout: Arc<dyn ByteMap>,
    config: DecoderConfig<B, S>,
) -> DecodeResult<Box<dyn Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    use crate::symphonia::{SymphoniaCodec, SymphoniaConfig};

    tracing::debug!(
        ?codec,
        "fmp4_segment: dispatching to segment-aware Symphonia path"
    );
    match codec {
        AudioCodec::AacLc | AudioCodec::AacHe | AudioCodec::AacHeV2 | AudioCodec::Flac => {
            let symphonia_config = SymphoniaConfig::builder().gapless(config.gapless).build();
            build_fmp4_segment_decoder(source, layout, config, |track| {
                SymphoniaCodec::open_with_config(track, &symphonia_config)
            })
        }
        other => Err(DecodeError::UnsupportedCodec { codec: other }),
    }
}
