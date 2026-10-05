use kithara_bufpool::HasPool;
use kithara_resampler::ResamplerBackend;
use kithara_stream::{AudioCodec, ContainerFormat};

use super::super::{
    build::finish,
    mpeg::build_mpeg_decoder,
    segment::{build_fmp4_segment_decoder, should_use_segment_aware},
};
use crate::{DecodeError, DecodeResult, Decoder, DecoderConfig, traits::BoxedSource};

pub(in crate::factory) fn create<B, S>(
    source: BoxedSource,
    codec: AudioCodec,
    container: Option<ContainerFormat>,
    config: DecoderConfig<B, S>,
) -> DecodeResult<Box<dyn Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    use crate::android::AndroidCodec;

    if should_use_segment_aware(codec, container, &config)
        && let Some(layout) = config.byte_map.clone()
    {
        if AndroidCodec::supports(codec) {
            tracing::debug!(
                ?codec,
                "fmp4_segment: dispatching to segment-aware Android HW codec path"
            );
            return build_fmp4_segment_decoder(source, layout, config, |track| {
                AndroidCodec::open_with_config(track)
            });
        }
        return Err(DecodeError::UnsupportedCodec { codec });
    }

    if android_standalone_supports(codec, container) {
        tracing::debug!(
            ?codec,
            ?container,
            "android-standalone: routing via AMediaExtractor"
        );
        return build_android_standalone_decoder(source, codec, container, config);
    }

    Err(DecodeError::UnsupportedCodec { codec })
}

fn android_standalone_supports(codec: AudioCodec, container: Option<ContainerFormat>) -> bool {
    matches!(
        (codec, container),
        (AudioCodec::Pcm, Some(ContainerFormat::Wav))
            | (
                AudioCodec::Mp3,
                Some(ContainerFormat::MpegAudio | ContainerFormat::Wav)
            )
            | (AudioCodec::Alac, Some(ContainerFormat::Mp4))
            | (
                AudioCodec::AacLc | AudioCodec::AacHe | AudioCodec::AacHeV2,
                Some(ContainerFormat::Adts | ContainerFormat::Mp4 | ContainerFormat::Fmp4)
            )
            | (
                AudioCodec::Flac,
                Some(ContainerFormat::Flac | ContainerFormat::Mp4 | ContainerFormat::Fmp4)
            )
    )
}

fn build_android_standalone_decoder<B, S>(
    mut source: BoxedSource,
    codec: AudioCodec,
    container: Option<ContainerFormat>,
    config: DecoderConfig<B, S>,
) -> DecodeResult<Box<dyn Decoder>>
where
    B: ResamplerBackend,
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    use kithara_android::media::sys::{KEY_ENCODER_DELAY, KEY_ENCODER_PADDING};

    use crate::{
        android::{AndroidCodec, AndroidMediaExtractorDemuxer},
        demuxer::Demuxer,
        gapless::probe_mp4_gapless,
    };
    if codec == AudioCodec::Mp3 {
        return build_mpeg_decoder(source, container, config, |demuxer| {
            AndroidCodec::open_with_config(demuxer.track_info())
        });
    }
    let gapless = if config.gapless
        && matches!(
            container,
            Some(ContainerFormat::Mp4 | ContainerFormat::Fmp4)
        ) {
        probe_mp4_gapless(&mut source, &config.pools)?
    } else {
        None
    };
    let init_end = (container == Some(ContainerFormat::Wav))
        .then(|| config.byte_map.as_ref().map(|map| map.init_segment_range()))
        .flatten()
        .filter(|range| range.start == 0 && !range.is_empty())
        .map(|range| range.end);
    let (demuxer, mut format) = AndroidMediaExtractorDemuxer::open(
        source,
        codec,
        config.byte_map.clone(),
        config.byte_len_handle.clone(),
        init_end,
    )?;
    let mut track = demuxer.track_info().clone();
    track.gapless = gapless;
    if gapless.is_some() || !config.gapless {
        format.set_i32(KEY_ENCODER_DELAY, 0);
        format.set_i32(KEY_ENCODER_PADDING, 0);
    }
    let codec_impl = AndroidCodec::open_with_format(&track, &format)?;
    finish(demuxer, codec_impl, config)
}
