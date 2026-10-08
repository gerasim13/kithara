use std::{io::Error as IoError, num::NonZeroUsize};

use crate::{PendingReason, SourcePhase};

/// Real error from [`crate::Stream::try_read`] — the underlying source
/// surfaced an I/O failure.
///
/// Status conditions (seek pending, data not ready, variant change,
/// retry) are **not** errors and are carried in
/// [`StreamReadOutcome::Pending`] with a typed [`PendingReason`]. Only
/// genuine source failures end up here.
#[derive(Debug, derive_more::Display, derive_more::Error)]
#[error(ignore)]
#[non_exhaustive]
pub enum StreamReadError {
    /// Anything surfaced by the underlying [`crate::Source`] as a real error.
    #[display("source error: {_0}")]
    Source(#[error(source)] IoError),
}

/// Outcome of a [`crate::Stream::try_read`] call.
///
/// Mirrors the [`crate::ReadOutcome`] shape from
/// [`Source::read_at`](crate::Source::read_at), but extends each variant
/// with the authoritative `byte_position` from the source cursor for
/// callers that don't want to read it back themselves.
/// `Bytes` carries a [`NonZeroUsize`] count — the type system
/// guarantees forward progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamReadOutcome {
    /// Stream produced `count` bytes. `byte_position` is the new byte
    /// offset **after** the read.
    Bytes {
        count: NonZeroUsize,
        byte_position: u64,
    },
    /// No progress this call. See [`PendingReason`] for the precise
    /// cause and required caller action.
    Pending(PendingReason),
    /// Natural end of stream. `byte_position` is the offset where EOF
    /// was observed (typically the source length).
    Eof { byte_position: u64 },
}

/// Typed payload of an `io::Error` (kind [`std::io::ErrorKind::Interrupted`])
/// emitted by `impl Read for Stream` when the underlying source could
/// not satisfy the read this call. Both `SeekPending` and
/// `NotReady`/`Retry` surface as `Interrupted` so demuxers (notably
/// Symphonia's fragmented MP4 reader) treat the pause as a transient
/// cooperative interruption and let `kithara-decode::is_seek_pending_io`
/// classify the failure correctly — the previous `WouldBlock` mapping
/// was treated as a hard "would block" by Symphonia's seek path and
/// corrupted the demuxer cursor on partial reads. Carries the
/// [`PendingReason`] verbatim plus a snapshot of source/timeline state
/// at the wrap site, so callers downcasting from `io::Error` recover
/// both *what* stalled and *why* without having to instrument their
/// own decoder.
#[derive(Debug, Clone, Copy, derive_more::Display)]
#[display(
    "{reason}: pos={pos} want={want} len={len:?} phase={phase:?} epoch={epoch} flushing={flushing}"
)]
#[non_exhaustive]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct StreamPending {
    pub(crate) len: Option<u64>,
    pub(crate) reason: PendingReason,
    pub(crate) phase: SourcePhase,
    pub(crate) flushing: bool,
    pub(crate) epoch: u64,
    pub(crate) pos: u64,
    pub(crate) want: usize,
}

impl StreamPending {
    /// Build the typed payload for a transient "data not ready" read.
    #[must_use]
    pub const fn new(
        reason: PendingReason,
        pos: u64,
        want: usize,
        len: Option<u64>,
        phase: SourcePhase,
        epoch: u64,
        flushing: bool,
    ) -> Self {
        Self {
            len,
            reason,
            phase,
            flushing,
            epoch,
            pos,
            want,
        }
    }

    /// Typed reason decoders downcast on to classify a transient stall.
    #[must_use]
    pub const fn reason(&self) -> PendingReason {
        self.reason
    }
}
