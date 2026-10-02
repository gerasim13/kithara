use kithara_config::Config;
use kithara_dsp::spectrum::FftLen;

use crate::{Band, consts};

/// FFT / band-split / reduction tunables. One home for the constants.
#[derive(Config, Clone, Copy, Debug)]
#[config(default, builder(state_mod(vis = "pub")), fields(value, get(copy)))]
#[non_exhaustive]
pub struct AnalysisParams {
    /// Per-band perceptual gain (`[low, mid, high]`) applied to magnitudes
    /// before shared normalization. Music tilts energy toward the low end, so
    /// without lifting mid/high the upper bands render as invisible slivers.
    /// This is the balance knob, not a color: low stays the dominant hull.
    #[config(builder(default = consts::BAND_GAIN))]
    band_gain: [f32; Band::COUNT],
    /// Per-window RMS gate; windows below it contribute no band energy.
    #[config(builder(default = consts::ENERGY_FLOOR))]
    energy_floor: f32,
    /// Low/mid crossover in Hz.
    #[config(builder(default = consts::LOW_MID_HZ))]
    low_mid_hz: f32,
    /// Mid/high crossover in Hz.
    #[config(builder(default = consts::MID_HIGH_HZ))]
    mid_high_hz: f32,
    /// FFT window length (real input); band bins span `0..=fft_size/2`.
    #[config(builder(default = consts::FFT_SIZE))]
    fft_size: FftLen,
}
