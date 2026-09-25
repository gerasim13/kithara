pub(super) mod frames {
    /// Analysis window, 46.4 ms.
    pub(crate) const FRAME: usize = 1024;
    /// Hop, 11.61 ms: the detection-function resolution the papers fix.
    pub(crate) const HOP: usize = 256;
    /// The rate the crate contract fixes.
    pub(crate) const RATE: f32 = 22_050.0;
}

pub(super) mod novelty {
    use super::frames;

    pub(crate) const HANN_A0: f32 = 0.5;
    /// Analysis stride, 23.2 ms: the rate the difference is actually
    /// measured at.
    pub(crate) const STRIDE: usize = 2 * frames::HOP;
}

pub(super) mod tempo {
    /// Fastest tempo the detector tracks.
    pub(crate) const BAND_HIGH_BPM: f32 = 185.0;
    /// Slowest tempo the detector tracks.
    pub(crate) const BAND_LOW_BPM: f32 = 48.0;
    /// The tempo the periodicity stage prefers inside the band.
    pub(crate) const PRIOR_BPM: f32 = 120.0;
    /// Deviation allowed between consecutive beats, in seconds.
    pub(crate) const TOLERANCE_SECONDS: f32 = 0.025;
}

pub(super) mod period {
    /// Periodicity window, 512 detection-function frames (5.94 s).
    pub(crate) const ACF_FRAME: usize = 512;
    /// One beat-period estimate every 128 frames (1.49 s), a 75% overlap.
    pub(crate) const ACF_STEP: usize = 128;
    /// Comb elements each hypothesis is scored over.
    pub(crate) const COMB_HARMONICS: usize = 4;
    /// Hypothesis `i` is a period of `i + 1` lags; one per possible lag up
    /// to the estimate spacing.
    pub(crate) const HYPOTHESES: usize = ACF_STEP;
    /// Widest comb element reaches 3 lags below its harmonic, and the top
    /// hypothesis is where its widest element still reads inside the window.
    pub(crate) const PERIOD_INDEX: std::ops::RangeInclusive<usize> =
        (COMB_HARMONICS - 1)..=((ACF_FRAME - (COMB_HARMONICS - 1)) / COMB_HARMONICS - 1);
    /// Adaptive-threshold half window, 0.1 s of detection frames.
    pub(crate) const SMOOTH_HALF: usize = 8;
    /// Between-estimate spread of the period at the default drift, in lags.
    pub(crate) const TRANSITION_SIGMA: f32 = 8.0;
    /// The Gaussian transition's support, in standard deviations.
    pub(crate) const TRANSITION_SUPPORT_SIGMAS: f32 = 4.0;
}

pub(super) mod decode {
    /// Height scale of the interval density: the Gaussian claims about 0.43
    /// of each state's transition mass, keeping every beat transition soft.
    pub(crate) const DENSITY_SCALE: f32 = 0.005;
    pub(crate) const EPSILON: f32 = 1e-6;
    /// Observations top out below one, so a skipped peak stays payable.
    pub(crate) const OBSERVED_CEILING: f32 = 0.99;
    /// How far past the longest period the state space reaches, in standard
    /// deviations: the longest wait the decoder can express.
    pub(crate) const STATE_MARGIN: f32 = 3.0;
    /// The interval density's support, in standard deviations.
    pub(crate) const SUPPORT: f32 = 4.0;
}

pub(super) mod tracker {
    /// A mark is never a certainty, and never nothing.
    pub(crate) const CONFIDENCE_BOUNDS: (f32, f32) = (0.001, 0.999);
}
