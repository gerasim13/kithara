use bytes::Bytes;

/// One latest-snapshot payload write in an analysis-file update.
#[derive(Debug)]
#[non_exhaustive]
pub struct AnalysisFileWrite {
    bytes: Bytes,
    offset: u64,
}

impl AnalysisFileWrite {
    pub(super) const fn new(offset: u64, bytes: Bytes) -> Self {
        Self { bytes, offset }
    }

    /// Complete versioned `AnalysisProgress` payload.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Absolute destination offset.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }
}

/// One absolute fixed-header or fixed-index patch.
#[derive(Debug)]
#[non_exhaustive]
pub struct AnalysisFilePatch {
    bytes: Bytes,
    offset: u64,
}

impl AnalysisFilePatch {
    pub(super) const fn new(offset: u64, bytes: Bytes) -> Self {
        Self { bytes, offset }
    }

    /// Replacement bytes for this fixed location.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Absolute destination offset.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }
}

/// Ordered writes and patches for one atomic analysis-file replacement.
#[derive(Debug)]
#[non_exhaustive]
pub struct AnalysisFileUpdate {
    payload: AnalysisFileWrite,
    initial: Option<Bytes>,
    patches: Vec<AnalysisFilePatch>,
    final_len: u64,
}

impl AnalysisFileUpdate {
    pub(super) const fn new(
        initial: Option<Bytes>,
        payload: AnalysisFileWrite,
        patches: Vec<AnalysisFilePatch>,
        final_len: u64,
    ) -> Self {
        Self {
            payload,
            initial,
            patches,
            final_len,
        }
    }

    /// Exact length passed to the storage commit boundary.
    #[must_use]
    pub const fn final_len(&self) -> u64 {
        self.final_len
    }

    /// Header plus zeroed fixed index for a brand-new file. `None` means the
    /// writer seeds that fixed prefix from the prior committed generation.
    #[must_use]
    pub fn initial_bytes(&self) -> Option<&[u8]> {
        self.initial.as_deref()
    }

    /// Ordered absolute index/header patches applied after the payload write.
    #[must_use]
    pub fn patches(&self) -> &[AnalysisFilePatch] {
        &self.patches
    }

    /// The complete latest progress publication written at the fixed payload offset.
    #[must_use]
    pub const fn payload(&self) -> &AnalysisFileWrite {
        &self.payload
    }
}
