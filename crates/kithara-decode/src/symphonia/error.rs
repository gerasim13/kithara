use std::io::ErrorKind;

use kithara_stream::{PendingReason, StreamSeekPastEof};
use symphonia_core::errors::{Error as SymphoniaError, SeekErrorKind};

use crate::DecodeError;

/// The typed payload behind `Interrupted` is lost by the time it reaches here, so only its variant
/// name can be logged.
pub(super) fn classify_seek_err(err: &SymphoniaError) -> DecodeError {
    match err {
        SymphoniaError::SeekError(SeekErrorKind::OutOfRange) => DecodeError::SeekOutOfRange {
            detail: "seek target past indexed sample range",
        },
        SymphoniaError::IoError(io_err)
            if io_err.get_ref().is_some_and(
                <dyn std::error::Error + Send + Sync + 'static>::is::<StreamSeekPastEof>,
            ) =>
        {
            DecodeError::SeekOutOfRange {
                detail: "seek past end of stream",
            }
        }
        SymphoniaError::IoError(e) if e.kind() == ErrorKind::UnexpectedEof => {
            DecodeError::SeekOutOfRange {
                detail: "seek hit unexpected end of stream",
            }
        }
        SymphoniaError::IoError(io_err)
            if matches!(
                io_err.kind(),
                ErrorKind::Interrupted | ErrorKind::WouldBlock
            ) || crate::error::pending_input(io_err) == Some(PendingReason::SessionRetired) =>
        {
            tracing::debug!(error = ?io_err, "demuxer seek interrupted");
            DecodeError::Interrupted
        }
        SymphoniaError::DecodeError(detail)
        | SymphoniaError::Unsupported(detail)
        | SymphoniaError::LimitError(detail) => DecodeError::SeekFailed { detail },
        SymphoniaError::SeekError(SeekErrorKind::ForwardOnly) => DecodeError::SeekFailed {
            detail: "a forward-only source refused a backward seek",
        },
        SymphoniaError::SeekError(SeekErrorKind::Unseekable) => DecodeError::SeekFailed {
            detail: "the source cannot seek",
        },
        SymphoniaError::SeekError(SeekErrorKind::InvalidTrack) => DecodeError::SeekFailed {
            detail: "the seek named a track the source does not carry",
        },
        SymphoniaError::ResetRequired => DecodeError::SeekFailed {
            detail: "the seek left the reader needing a reset",
        },
        SymphoniaError::IoError(_) => DecodeError::SeekFailed {
            detail: "the seek failed on an i/o error",
        },
        _ => DecodeError::SeekFailed {
            detail: "symphonia seek failed",
        },
    }
}

pub(super) fn pending_reason(error: &SymphoniaError) -> Option<PendingReason> {
    let SymphoniaError::IoError(error) = error else {
        return None;
    };
    crate::error::pending_input(error)
}

#[cfg(test)]
mod tests {
    use std::io;

    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn a_retired_input_keeps_its_typed_reason_without_an_interrupted_io_kind() {
        let error = SymphoniaError::IoError(io::Error::new(
            ErrorKind::ConnectionAborted,
            PendingReason::SessionRetired,
        ));
        assert_eq!(pending_reason(&error), Some(PendingReason::SessionRetired));
        assert!(matches!(
            classify_seek_err(&error),
            DecodeError::Interrupted
        ));
        let unrelated = SymphoniaError::IoError(io::Error::new(
            ErrorKind::ConnectionAborted,
            "connection failed",
        ));
        assert_eq!(pending_reason(&unrelated), None);
        assert!(matches!(
            classify_seek_err(&unrelated),
            DecodeError::SeekFailed { .. }
        ));
    }
}
