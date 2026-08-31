use kithara_stream::AudioCodec;

#[cfg(not(target_arch = "wasm32"))]
use crate::offline::OfflineEncoder;
use crate::{
    error::{EncodeError, EncodeResult},
    types::{BytesEncodeRequest, EncodedBytes, EncodedTrack, PackagedEncodeRequest},
};

/// Entry point for encoded outputs with runtime codec selection.
pub struct EncoderFactory;

impl EncoderFactory {
    /// Encode a finite PCM source into complete encoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the target codec/backend rejects the request.
    pub fn encode_bytes(request: &BytesEncodeRequest<'_>) -> EncodeResult<EncodedBytes> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            OfflineEncoder::encode_bytes(request)
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = request;
            Err(Self::wasm_unsupported())
        }
    }

    /// Encode a finite PCM source into packaged access units for downstream muxing.
    ///
    /// # Errors
    ///
    /// Returns an error when `request.media_info.codec` is missing or the codec/backend
    /// rejects the request.
    pub fn encode_packaged(request: &PackagedEncodeRequest<'_>) -> EncodeResult<EncodedTrack> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let codec = request
                .media_info
                .codec
                .ok_or(EncodeError::InvalidMediaInfo("codec"))?;
            OfflineEncoder::packaged_frame_samples(codec)?;
            OfflineEncoder::encode_packaged(request)
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = request;
            Err(Self::wasm_unsupported())
        }
    }

    /// Return the natural frame size for packaged encoding of `codec`.
    ///
    /// # Errors
    ///
    /// Returns an error when the codec does not support packaged encoding.
    pub fn frame_samples(codec: AudioCodec) -> EncodeResult<usize> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            OfflineEncoder::packaged_frame_samples(codec)
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = codec;
            Err(Self::wasm_unsupported())
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_unsupported() -> EncodeError {
        EncodeError::InvalidInput("encoding is not supported on wasm32".to_owned())
    }
}
