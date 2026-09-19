//! How the platform owner's types reach decode's own vocabulary.

use kithara_android::{AndroidBackendError, media::OutputFormat};
use kithara_signal::AudioSpec;

use crate::{
    error::{DecodeError, DecodeResult},
    types::checked_audio_spec,
};

/// Signal spec of a codec or extractor output format.
pub(crate) fn output_spec(output: &OutputFormat) -> DecodeResult<AudioSpec> {
    checked_audio_spec(output.channels, output.sample_rate, "android.codec.output")
}

impl From<AndroidBackendError> for DecodeError {
    fn from(err: AndroidBackendError) -> Self {
        match err {
            AndroidBackendError::Status { operation, status } => Self::BackendStatus {
                code: status,
                op: operation,
            },
            source => Self::backend(source),
        }
    }
}
