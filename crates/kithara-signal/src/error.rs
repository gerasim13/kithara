use std::error::Error as StdError;

/// Checked signal-shape and conversion failures.
#[derive(Clone, Copy, Debug, derive_more::Display, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignalError {
    /// An operation requires at least one channel.
    #[display("audio channel count must be non-zero")]
    ChannelCountZero,
    /// A frame-to-sample conversion exceeded the addressable sample count.
    #[display("{frames} frames at {channels} channels exceed the sample address space")]
    SampleCountOverflow { frames: usize, channels: usize },
    /// Borrowed storage does not exactly match its declared signal shape.
    #[display(
        "signal shape requires {expected_samples} samples but storage contains {actual_samples}"
    )]
    Shape {
        expected_samples: usize,
        actual_samples: usize,
    },
    /// Interleaved storage ends with an incomplete frame.
    #[display("{samples} samples do not form complete {channels}-channel frames")]
    IncompleteFrame { samples: usize, channels: usize },
    /// A requested frame range is inverted or exceeds the view.
    #[display("frame range {start}..{end} exceeds {frames} available frames")]
    FrameRange {
        start: usize,
        end: usize,
        frames: usize,
    },
    /// A requested channel does not exist.
    #[display("channel {channel} exceeds {channels} available channels")]
    ChannelRange { channel: usize, channels: usize },
    /// Caller-provided planar channels do not match the signal channel count.
    #[display("signal requires {expected} channels but caller provided {actual}")]
    ChannelCount { expected: usize, actual: usize },
    /// Caller-provided planar channels do not have one common frame extent.
    #[display("channel {channel} has {actual} frames; expected {expected}")]
    ChannelFrames {
        channel: usize,
        expected: usize,
        actual: usize,
    },
    /// Caller-provided output storage is too small.
    #[display(
        "caller storage contains {available_samples} samples but {required_samples} are required"
    )]
    Capacity {
        required_samples: usize,
        available_samples: usize,
    },
    /// The injected pool region cannot reserve the requested storage.
    #[display("pool region cannot reserve {required_samples} samples")]
    PoolCapacity { required_samples: usize },
    /// A frame-to-duration conversion exceeds the duration representation.
    #[display("duration for {frames} frames at {sample_rate} Hz is out of range")]
    DurationOverflow { frames: u64, sample_rate: u32 },
    /// A duration-to-frame conversion exceeds the requested frame representation.
    #[display("frame count for {duration_nanos} ns at {sample_rate} Hz is out of range")]
    FrameCountOverflow {
        duration_nanos: u128,
        sample_rate: u32,
    },
}

impl StdError for SignalError {}
