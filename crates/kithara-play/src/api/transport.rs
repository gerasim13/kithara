/// A continuous beat coordinate on the session transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct SessionBeat(f64);

impl SessionBeat {
    /// Creates a finite session-beat coordinate. Negative beats are valid.
    pub fn new(value: f64) -> Result<Self, SessionBeatError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(SessionBeatError { value })
        }
    }

    #[must_use]
    /// Returns the continuous beat coordinate.
    pub fn get(self) -> f64 {
        self.0
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

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::SessionBeat;

    #[kithara::test]
    fn accepts_negative_and_zero_coordinates() {
        let negative = SessionBeat::new(-1.5).expect("invariant: finite negative beat is valid");
        let zero = SessionBeat::new(0.0).expect("invariant: zero beat is valid");

        assert_eq!(negative.get(), -1.5);
        assert_eq!(zero.get(), 0.0);
    }

    #[kithara::test]
    fn rejects_non_finite_coordinates() {
        assert!(SessionBeat::new(f64::NAN).is_err());
        assert!(SessionBeat::new(f64::INFINITY).is_err());
        assert!(SessionBeat::new(f64::NEG_INFINITY).is_err());
    }
}
