use thiserror::Error;

/// Why a beat grid document does not describe a grid anyone can play to.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
#[non_exhaustive]
pub enum BeatGridError {
    /// A downbeat names a beat the grid does not list, or lists elsewhere.
    #[error("the downbeat on beat {ordinal} does not match the beat listed there")]
    Anchor { ordinal: i64 },
    #[error("{bpm} is not a positive finite tempo")]
    Bpm { bpm: f64 },
    #[error("{confidence} is outside the 0..=1 a detector can report")]
    Confidence { confidence: f32 },
    /// The grid cannot be recognised across revisions without a name.
    #[error("the grid carries no model id")]
    ModelId,
    /// A bar of that length cannot start on that beat, so the phase the
    /// downbeats state and the phase the meter states disagree.
    #[error("a bar of {beats_per_bar} beats cannot start on beat {ordinal}")]
    Meter { beats_per_bar: u16, ordinal: i64 },
    /// Times and ordinals both run strictly forward, so a marker that does not
    /// advance leaves two readings of the same beat.
    #[error("beat {ordinal} at {seconds} s does not follow the marker before it")]
    Order { ordinal: i64, seconds: f64 },
    #[error("beat {ordinal} at {seconds} s lies past the stated {duration} s")]
    PastDuration {
        ordinal: i64,
        seconds: f64,
        duration: f64,
    },
    #[error("schema {found} is not the schema {expected} this build reads")]
    Schema { expected: u32, found: u32 },
    #[error("{seconds} s is not a position on a media timeline")]
    Time { seconds: f64 },
}
