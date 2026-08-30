use kithara_bufpool::{HasPool, PoolRegion};
use kithara_stream::AudioCodec;

#[cfg(target_arch = "wasm32")]
use crate::error::EncodeError;
#[cfg(not(target_arch = "wasm32"))]
use crate::offline::OfflineEncoder;
use crate::{
    error::EncodeResult,
    traits::InnerEncoder,
    types::{BytesEncodeRequest, EncodedBytes, EncodedTrack, PackagedEncodeRequest},
};

/// Factory for creating encoded outputs with runtime codec selection.
pub struct EncoderFactory;

impl EncoderFactory {
    /// Create an encoder backend for complete encoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when encoding is unavailable on the current target.
    pub fn create_bytes(target: crate::BytesEncodeTarget) -> EncodeResult<Box<dyn InnerEncoder>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = target;
            Ok(Box::new(OfflineEncoder))
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = target;
            Self::wasm_unsupported()
        }
    }

    /// Encode a finite PCM source into complete encoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the target codec/backend rejects the request.
    pub fn encode_bytes(request: BytesEncodeRequest<'_>) -> EncodeResult<EncodedBytes> {
        Self::create_bytes(request.target)?.encode_bytes(request)
    }

    /// Encode a finite PCM source into packaged access units for downstream muxing.
    ///
    /// # Errors
    ///
    /// Returns an error when `request.media_info.codec` is missing or the codec/backend
    /// rejects the request.
    pub fn encode_packaged<S>(
        pools: &PoolRegion<S>,
        request: &PackagedEncodeRequest<'_>,
    ) -> EncodeResult<EncodedTrack>
    where
        S: HasPool<u8> + HasPool<f32>,
    {
        #[cfg(not(target_arch = "wasm32"))]
        {
            OfflineEncoder::encode_packaged(pools, request)
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pools, request);
            Err(EncodeError::InvalidInput(
                "encoding is not supported on wasm32".to_owned(),
            ))
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
            OfflineEncoder::frame_samples(codec)
        }

        #[cfg(target_arch = "wasm32")]
        {
            let _ = codec;
            Err(EncodeError::InvalidInput(
                "encoding is not supported on wasm32".to_owned(),
            ))
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_unsupported() -> EncodeResult<Box<dyn InnerEncoder>> {
        Err(EncodeError::InvalidInput(
            "encoding is not supported on wasm32".to_owned(),
        ))
    }
}
