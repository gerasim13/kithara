/// Why a decoder generation stopped producing PCM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GaplessEnd {
    /// The generation ended mid-track — the logical track continues in the
    /// next one, so nothing that trims the end of a track applies.
    FormatBoundary,
    /// The track itself ended.
    TrackEof,
}
