use std::num::NonZeroU64;

/// The shape of input a reader needs before it can be constructed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReaderInput {
    /// Init-bearing container: the init header must be buffered before
    /// construction; media bytes are read later.
    InitOnly,
    /// Self-framing input: nothing is gated before construction.
    Incremental,
}

/// Bytes that must remain readable behind the reader's current position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReaderWarmup {
    /// No bytes behind the current position are required.
    None,
    /// Keep at most `max_bytes` readable behind the current position.
    ReadBehind {
        /// Maximum retained byte distance behind the current position.
        max_bytes: NonZeroU64,
    },
}

impl ReaderWarmup {
    /// Return the retained byte distance, or `None` when no warmup is needed.
    #[must_use]
    pub const fn max_bytes(self) -> Option<NonZeroU64> {
        match self {
            Self::None => None,
            Self::ReadBehind { max_bytes } => Some(max_bytes),
        }
    }
}

/// Byte-access requirements for one reader implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ReaderProfile {
    input: ReaderInput,
    warmup: ReaderWarmup,
    read_ahead_bytes: NonZeroU64,
}

impl ReaderProfile {
    /// Create a reader profile from its construction, warmup, and read-ahead
    /// requirements.
    #[must_use]
    pub const fn new(
        input: ReaderInput,
        warmup: ReaderWarmup,
        read_ahead_bytes: NonZeroU64,
    ) -> Self {
        Self {
            input,
            warmup,
            read_ahead_bytes,
        }
    }

    /// Return the reader's construction-time input requirement.
    #[must_use]
    pub const fn input(&self) -> ReaderInput {
        self.input
    }

    /// Return the reader's behind-position warmup requirement.
    #[must_use]
    pub const fn warmup(&self) -> ReaderWarmup {
        self.warmup
    }

    /// Return the forward byte window the reader may consume.
    #[must_use]
    pub const fn read_ahead_bytes(&self) -> NonZeroU64 {
        self.read_ahead_bytes
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn profile_exposes_reader_requirements() {
        let warmup_bytes = NonZeroU64::new(4_096).expect("non-zero warmup");
        let read_ahead_bytes = NonZeroU64::new(32 * 1_024).expect("non-zero read ahead");
        let profile = ReaderProfile::new(
            ReaderInput::InitOnly,
            ReaderWarmup::ReadBehind {
                max_bytes: warmup_bytes,
            },
            read_ahead_bytes,
        );

        assert_eq!(profile.input(), ReaderInput::InitOnly);
        assert_eq!(
            profile.warmup(),
            ReaderWarmup::ReadBehind {
                max_bytes: warmup_bytes,
            }
        );
        assert_eq!(profile.warmup().max_bytes(), Some(warmup_bytes));
        assert_eq!(profile.read_ahead_bytes(), read_ahead_bytes);
    }
}
