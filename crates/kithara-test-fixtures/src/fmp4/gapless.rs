/// How the fMP4 mux should encode `encoder_delay` / `trailing_delay` into the
/// init segment. Decoders can read either path; real-world players rely on
/// different sources (`AVPlayer` reads `iTunSMPB`, our decoder prefers `elst`),
/// so a caller must be able to pin which path a body exercises.
///
/// Deliberately exhaustive: fMP4 has these two gapless paths and no third, so
/// the four combinations are the whole set, and the fixture protocol matches
/// on them to map the enum onto its wire strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GaplessEncoding {
    /// Don't write any gapless metadata. Decoders fall back to heuristics or
    /// pass through the full PCM (priming + trailing included).
    None,
    /// Write only an edit list (`edts`/`elst`) inside `trak`.
    #[default]
    Edts,
    /// Write only an iTunes freeform tag (`udta`/`meta`/`ilst`/`----` with
    /// `iTunSMPB`) inside `moov`.
    ItunSmpb,
    /// Write both `edts` and `iTunSMPB`. The decoder contract is that `elst`
    /// wins over `iTunSMPB` here.
    Both,
}

impl GaplessEncoding {
    #[must_use]
    pub const fn writes_edts(self) -> bool {
        matches!(self, Self::Edts | Self::Both)
    }

    #[must_use]
    pub const fn writes_itunsmpb(self) -> bool {
        matches!(self, Self::ItunSmpb | Self::Both)
    }
}
