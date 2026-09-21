/// One detected beat or downbeat: where it is, and how sure the detector was.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct BeatMark {
    /// Seconds from the start of the analysed audio.
    pub at: f32,
    /// Probability the detector assigned this peak, in `(0, 1)`.
    pub confidence: f32,
}

/// Beat / downbeat marks in seconds, whole-track.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RawBeats {
    pub beats: Vec<BeatMark>,
    pub downbeats: Vec<BeatMark>,
}

impl BeatMark {
    /// A mark at `at` seconds the detector was `confidence` sure of.
    #[must_use]
    pub const fn new(at: f32, confidence: f32) -> Self {
        Self { at, confidence }
    }
}

impl RawBeats {
    /// Marks as a detector reports them: beats, and the downbeats among them.
    #[must_use]
    pub const fn new(beats: Vec<BeatMark>, downbeats: Vec<BeatMark>) -> Self {
        Self { beats, downbeats }
    }
}
