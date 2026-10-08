use std::ops::Range;

use crate::{SourceError, StreamError, StreamResult, VariantControl};

/// Non-retriable cross-variant boundary signal — the typed payload of
/// the `io::Error` produced by `impl Read for Stream` when the
/// underlying source fenced on a variant change. Decoders that go
/// through `std::io::Read` (Symphonia chain walker) downcast on this
/// type to recover the precise classification without string-matching.
#[derive(Debug, Clone, Copy, derive_more::Display)]
#[display("variant change: decoder recreation required")]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct VariantChangeError;

/// Header byte range for decoder recreate after a format change — the one
/// mapping from an optional [`VariantControl`] to the typed answer. Shared
/// by [`crate::Stream::format_change_segment_range`] and lock-free wrappers that
/// hold the variant-control handle directly.
///
/// # Errors
///
/// `Err(SourceError::FormatChangeNotApplicable)` for sources without a
/// variant surface (non-HLS) or HLS variants activated with
/// `served_from > 0` (init prefix unreachable via Stream reads).
pub fn format_change_segment_range(vc: Option<&dyn VariantControl>) -> StreamResult<Range<u64>> {
    vc.map_or(
        Err(StreamError::Source(SourceError::FormatChangeNotApplicable)),
        VariantControl::format_change_segment_range,
    )
}
