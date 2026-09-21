/// Which artifacts one pass is opened to produce.
///
/// A caller that already holds a prepared artifact asks for the rest; the
/// analyzers a configuration enables never change, only what a single pass is
/// opened for. A pass reports the fingerprint of what it actually produced, so
/// a narrowed result is never mistaken for a full one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalysisDemand {
    beat: bool,
    waveform: bool,
}

impl AnalysisDemand {
    /// Everything the configuration can produce.
    pub const ALL: Self = Self {
        beat: true,
        waveform: true,
    };

    /// Ask for exactly these artifacts.
    #[must_use]
    pub const fn new(beat: bool, waveform: bool) -> Self {
        Self { beat, waveform }
    }

    /// Whether a beat artifact is asked for.
    #[must_use]
    pub const fn beat(self) -> bool {
        self.beat
    }

    /// Whether a waveform is asked for.
    #[must_use]
    pub const fn waveform(self) -> bool {
        self.waveform
    }

    /// Whether nothing is left to produce, so no pass is worth opening.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        !self.beat && !self.waveform
    }
}

impl Default for AnalysisDemand {
    fn default() -> Self {
        Self::ALL
    }
}
