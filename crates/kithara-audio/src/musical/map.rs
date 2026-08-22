/// An invalid session coordinate or coordinate rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum CoordinateError {
    /// The supplied coordinate is `NaN` or infinite.
    #[error("coordinate must be finite")]
    NonFinite,
    /// A coordinate rate cannot define an invertible frame relation.
    #[error("coordinate rate must advance by a finite, positive amount per frame")]
    NonInvertibleRate,
}
