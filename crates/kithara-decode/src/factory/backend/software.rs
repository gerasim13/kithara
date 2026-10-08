#[cfg(feature = "symphonia")]
pub(super) use crate::factory::software::{create, create_segment};

#[cfg(not(feature = "symphonia"))]
mod unavailable {
    use kithara_bufpool::HasPool;
    use kithara_platform::sync::Arc;
    use kithara_resampler::ResamplerBackend;
    use kithara_stream::{AudioCodec, ByteMap, ContainerFormat};

    use crate::{DecodeError, DecodeResult, Decoder, DecoderConfig, traits::BoxedSource};

    pub(in crate::factory::backend) fn create<B, S>(
        _source: BoxedSource,
        codec: AudioCodec,
        _container: Option<ContainerFormat>,
        _config: DecoderConfig<B, S>,
    ) -> DecodeResult<Box<dyn Decoder>>
    where
        B: ResamplerBackend,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    {
        Err(DecodeError::UnsupportedCodec { codec })
    }

    pub(in crate::factory::backend) fn create_segment<B, S>(
        _source: BoxedSource,
        codec: AudioCodec,
        _layout: Arc<dyn ByteMap>,
        _config: DecoderConfig<B, S>,
    ) -> DecodeResult<Box<dyn Decoder>>
    where
        B: ResamplerBackend,
        S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
    {
        Err(DecodeError::UnsupportedCodec { codec })
    }
}

#[cfg(not(feature = "symphonia"))]
pub(super) use unavailable::{create, create_segment};
