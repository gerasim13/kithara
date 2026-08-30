use kithara_bufpool::{HasPool, PoolRegion};
use kithara_stream::AudioCodec;

#[cfg(feature = "fdk-aac")]
use crate::fdk::aac_he::{AacHeEncoder, AacHeProfile};
#[cfg(feature = "ffmpeg")]
use crate::ffmpeg::{aac::AacFFmpegEncoder, flac::FlacFFmpegEncoder};
use crate::{
    BytesEncodeRequest, EncodeError, EncodeResult, EncodedBytes, EncodedTrack, InnerEncoder,
    PackagedEncodeRequest,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct OfflineEncoder;

impl InnerEncoder for OfflineEncoder {
    fn encode_bytes(&self, request: BytesEncodeRequest<'_>) -> EncodeResult<EncodedBytes> {
        super::bytes::encode(&request)
    }
}

impl OfflineEncoder {
    pub(crate) fn encode_packaged<S>(
        pools: &PoolRegion<S>,
        request: &PackagedEncodeRequest<'_>,
    ) -> EncodeResult<EncodedTrack>
    where
        S: HasPool<u8> + HasPool<f32>,
    {
        let codec = request
            .media_info
            .codec
            .ok_or(EncodeError::InvalidMediaInfo("codec"))?;
        match codec {
            #[cfg(feature = "ffmpeg")]
            AudioCodec::AacLc => AacFFmpegEncoder::encode(pools, request),
            #[cfg(feature = "fdk-aac")]
            AudioCodec::AacHe => AacHeEncoder::encode(pools, request, AacHeProfile::V1),
            #[cfg(feature = "fdk-aac")]
            AudioCodec::AacHeV2 => AacHeEncoder::encode(pools, request, AacHeProfile::V2),
            #[cfg(feature = "ffmpeg")]
            AudioCodec::Flac => FlacFFmpegEncoder::encode(request),
            codec => Err(EncodeError::UnsupportedCodec(codec)),
        }
    }

    pub(crate) fn frame_samples(codec: AudioCodec) -> EncodeResult<usize> {
        match codec {
            #[cfg(feature = "ffmpeg")]
            AudioCodec::AacLc => Ok(AacFFmpegEncoder::frame_samples()),
            #[cfg(feature = "fdk-aac")]
            AudioCodec::AacHe | AudioCodec::AacHeV2 => Ok(AacHeEncoder::frame_samples()),
            #[cfg(feature = "ffmpeg")]
            AudioCodec::Flac => Ok(FlacFFmpegEncoder::frame_samples()),
            codec => Err(EncodeError::UnsupportedCodec(codec)),
        }
    }
}
