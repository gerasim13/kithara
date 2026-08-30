use crate::{BytesEncodeRequest, EncodeResult, EncodedBytes};

/// Runtime-polymorphic audio encoder backend.
///
/// Clients obtain an implementation from [`crate::EncoderFactory`] and use it
/// without depending on a concrete backend such as `FFmpeg`.
pub trait InnerEncoder: Send + Sync + 'static {
    /// Encode a finite PCM source into complete encoded bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend cannot encode the provided PCM input.
    fn encode_bytes(&self, request: BytesEncodeRequest<'_>) -> EncodeResult<EncodedBytes>;
}
