use bon::Builder;
use kithara_encode::EncodeConfig;

/// Configuration for one independently playable recording part.
#[derive(Clone, Debug, Builder)]
#[non_exhaustive]
pub struct RecordingConfig {
    encode: EncodeConfig,
}

impl RecordingConfig {
    /// Encoding profile for this part.
    #[must_use]
    pub const fn encode(&self) -> &EncodeConfig {
        &self.encode
    }
}
