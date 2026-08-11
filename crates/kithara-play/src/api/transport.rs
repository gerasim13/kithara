use std::num::NonZeroU64;

const SECONDS_PER_MINUTE: f64 = 60.0;

/// A musical tempo in beats per minute, inside the range the session clock can
/// carry.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, fieldwork::Fieldwork)]
#[fieldwork(get)]
pub struct Tempo(
    /// Returns the tempo in beats per minute.
    #[field(get = beats_per_minute, copy)]
    f64,
);

impl Tempo {
    /// The slowest accepted tempo.
    pub const MIN_BEATS_PER_MINUTE: f64 = 1.0;

    /// The fastest accepted tempo. The upper bound is what keeps the anchor
    /// arithmetic finite: an unbounded tempo overflows the beat span of a
    /// single block, and the resulting failure strands the transport with an
    /// active commit and no anchor.
    pub const MAX_BEATS_PER_MINUTE: f64 = 1_000.0;

    /// Creates a tempo, rejecting non-finite and out-of-range values.
    pub fn new(beats_per_minute: f64) -> Result<Self, TempoError> {
        if (Self::MIN_BEATS_PER_MINUTE..=Self::MAX_BEATS_PER_MINUTE).contains(&beats_per_minute) {
            Ok(Self(beats_per_minute))
        } else {
            Err(TempoError { beats_per_minute })
        }
    }

    pub(crate) fn beats_per_second(self) -> f64 {
        self.0 / SECONDS_PER_MINUTE
    }
}

impl TryFrom<f64> for Tempo {
    type Error = TempoError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// The value supplied for a musical tempo was invalid.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[error(
    "tempo must be between {min} and {max} beats per minute, got {beats_per_minute}",
    min = Tempo::MIN_BEATS_PER_MINUTE,
    max = Tempo::MAX_BEATS_PER_MINUTE,
)]
#[non_exhaustive]
pub struct TempoError {
    beats_per_minute: f64,
}

/// A continuous beat coordinate on the session transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, derive_more::Into)]
pub struct SessionBeat(f64);

impl SessionBeat {
    /// Creates a finite session-beat coordinate. Negative beats are valid.
    pub const fn new(value: f64) -> Result<Self, SessionBeatError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(SessionBeatError { value })
        }
    }
}

impl TryFrom<f64> for SessionBeat {
    type Error = SessionBeatError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// The value supplied for a session-beat coordinate was invalid.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[error("session beat must be finite, got {value}")]
#[non_exhaustive]
pub struct SessionBeatError {
    value: f64,
}

/// Monotonic generation of a committed session transport configuration.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct TransportRevision(NonZeroU64);

impl TransportRevision {
    pub(crate) const FIRST: Self = Self(NonZeroU64::MIN);

    pub(crate) fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
    }
}

/// The last session transport position processed by the audio graph.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct SessionTransportSnapshot {
    /// Returns the processed position on the session beat grid.
    #[field(get, copy)]
    position: SessionBeat,
    /// Returns the tempo that produced this processed position.
    #[field(get, copy)]
    tempo: Tempo,
    /// Returns the monotonic revision of the committed transport configuration.
    #[field(get, copy)]
    revision: TransportRevision,
    /// Returns whether the processed session transport is playing.
    #[field(get = is_playing, copy)]
    playing: bool,
}

impl SessionTransportSnapshot {
    pub(crate) const fn new(
        position: SessionBeat,
        playing: bool,
        tempo: Tempo,
        revision: TransportRevision,
    ) -> Self {
        Self {
            position,
            tempo,
            revision,
            playing,
        }
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::SessionBeat;

    #[kithara::test]
    fn accepts_negative_and_zero_coordinates() {
        let negative = SessionBeat::new(-1.5).expect("invariant: finite negative beat is valid");
        let zero = SessionBeat::new(0.0).expect("invariant: zero beat is valid");

        assert_eq!(f64::from(negative), -1.5);
        assert_eq!(f64::from(zero), 0.0);
    }

    #[kithara::test]
    fn rejects_non_finite_coordinates() {
        assert!(SessionBeat::new(f64::NAN).is_err());
        assert!(SessionBeat::new(f64::INFINITY).is_err());
        assert!(SessionBeat::new(f64::NEG_INFINITY).is_err());
    }
}
