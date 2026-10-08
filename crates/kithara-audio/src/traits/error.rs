use kithara_decode::DecodeError;

use crate::TrackFailureKind;

/// The consumer observation point of a terminal PCM failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FailureSource {
    /// A terminal marker consumed during normal playback.
    #[error("PCM producer reported a failure: {failure}")]
    Producer {
        /// The producer's preserved terminal classification.
        failure: TrackFailureKind,
    },
    /// A terminal marker consumed while staging a seek.
    #[error("PCM producer reported a failure while staging a seek: {failure}")]
    ProducerAfterSeek {
        /// The producer's preserved terminal classification.
        failure: TrackFailureKind,
    },
    /// The producer disappeared without a terminal marker.
    #[error("PCM channel closed with no failure marker")]
    ChannelClosed,
}

/// A decoded-audio read failure; stream classifications carry no heap payload.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AudioReadError {
    /// An error returned directly by a decoded-audio reader.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// A terminal stream error with an inline classification.
    #[error("{what}: {source}")]
    Stream {
        /// The operation that observed the failure.
        what: &'static str,
        /// The preserved cause and consumer observation point.
        source: FailureSource,
    },
}
