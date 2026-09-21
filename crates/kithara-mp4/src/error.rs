use std::fmt;

/// Why a box walk could not deliver what was asked of it.
///
/// The detail is a fixed phrase, not a formatted message: a caller wraps it
/// in its own error type and keeps its own vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mp4Error {
    detail: &'static str,
}

impl Mp4Error {
    pub(crate) const fn new(detail: &'static str) -> Self {
        Self { detail }
    }

    /// What went wrong, as a fixed phrase.
    #[must_use]
    pub const fn detail(&self) -> &'static str {
        self.detail
    }
}

impl fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.detail)
    }
}

impl std::error::Error for Mp4Error {}
