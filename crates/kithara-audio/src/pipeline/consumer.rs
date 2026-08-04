/// How a consumer's stream ended in failure.
///
/// The three ways a PCM stream can die are three different bugs, and one
/// message for all of them tells a reader nothing: a producer that reported a
/// decode error is not a producer whose channel vanished without a word. A
/// seek storm that kills the stream has to say which one it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureSource {
    /// The worker pushed a failure marker: decode or source gave up.
    Producer,
    /// The same marker, taken on the path that stages the first fetch of a
    /// seek epoch.
    ProducerAfterSeek,
    /// The channel closed with no marker at all — the worker is gone.
    ChannelClosed,
}

impl FailureSource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Producer => "pcm producer reported a failure",
            Self::ProducerAfterSeek => "pcm producer reported a failure while staging a seek",
            Self::ChannelClosed => "pcm channel closed with no failure marker",
        }
    }
}

/// Consumer-side phase for `Audio<S>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsumerPhase {
    Buffering,
    Playing,
    SeekPending { epoch: u64 },
    AtEof,
    Failed { source: FailureSource },
}

impl ConsumerPhase {
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::AtEof | Self::Failed { .. })
    }
}
