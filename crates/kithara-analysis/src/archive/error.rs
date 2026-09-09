use crate::BlobError;

/// Failure to create, update, or restore an indexed analysis file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnalysisFileError {
    /// A progressive file needs a stable extent before it can size its index.
    #[error("analysis extent is unknown")]
    UnknownExtent,
    /// The supplied axis, extent, chunk size, or fingerprints differ from the file.
    #[error("analysis file configuration does not match")]
    Config,
    /// An update must outrank the latest committed snapshot.
    #[error("analysis revision {incoming} does not outrank stored revision {stored}")]
    StaleRevision { stored: u64, incoming: u64 },
    /// A later snapshot stopped covering a chunk already marked complete.
    #[error("analysis coverage regressed at chunk {chunk}")]
    CoverageRegression { chunk: u64 },
    /// A fixed header cannot hold this fingerprint verbatim.
    #[error("analysis fingerprint length {len} exceeds fixed limit {max}")]
    FingerprintTooLong { len: usize, max: usize },
    /// An offset, index, or payload length cannot be represented safely.
    #[error("analysis file is too large")]
    TooLarge,
    /// The indexed-file version is not current.
    #[error("analysis file version {found} != expected {expected}")]
    Version { found: u32, expected: u32 },
    /// Header, index, or payload bounds are internally inconsistent.
    #[error("analysis file is corrupt")]
    Corrupt,
    /// The latest full snapshot payload is invalid.
    #[error("analysis payload is invalid: {0}")]
    Payload(#[from] BlobError),
}
