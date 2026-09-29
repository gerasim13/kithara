use std::f64::consts::TAU;

use kithara_test_fixtures::signal::Wave;
use num_traits::ToPrimitive;

/// Least ratio of spectrum energy to error energy against the direct DFT,
/// in decibels.
pub(crate) const DFT_SNR_DB: f64 = 110.0;
const RATE: u32 = 48_000;
const SINE: Wave = Wave::Sine {
    hz: 440.0,
    peak: i16::MAX,
};

/// A sine under a sawtooth: energy in every bin.
pub(crate) fn mix(len: usize) -> Vec<f32> {
    (0..len)
        .map(|frame| {
            let sine = f32::from(SINE.sample(frame, RATE));
            let saw = f32::from(Wave::Sawtooth.sample(frame, RATE));
            sine.mul_add(0.5, saw) / 65_536.0
        })
        .collect()
}

/// Bins `0..=N/2` of `signal` by the defining sum, in `f64`. The twiddles
/// `e^(−2πim/N)` are tabulated once and indexed by `k·n mod N`, so a 4096
/// frame costs multiply-adds, not sixteen million `sin_cos` calls.
pub(crate) fn dft(signal: &[f32]) -> Vec<(f64, f64)> {
    let len = signal.len();
    let step = TAU / len.to_f64().unwrap_or(f64::NAN);
    let twiddles: Vec<(f64, f64)> = (0..len)
        .map(|turn| {
            let (sin, cos) = (step * turn.to_f64().unwrap_or(f64::NAN)).sin_cos();
            (cos, -sin)
        })
        .collect();
    (0..=len / 2)
        .map(|bin| {
            signal
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(re, im), (n, sample)| {
                    let (cos, sin) = bin
                        .checked_mul(n)
                        .and_then(|turn| turn.checked_rem(len))
                        .and_then(|turn| twiddles.get(turn))
                        .copied()
                        .unwrap_or((f64::NAN, f64::NAN));
                    let sample = f64::from(*sample);
                    (sample.mul_add(cos, re), sample.mul_add(sin, im))
                })
        })
        .collect()
}

/// Energy of `want` over the energy of `re + i·im − want`, in decibels.
pub(crate) fn snr([re, im]: [&[f32]; 2], want: &[(f64, f64)]) -> f64 {
    let (signal, error) = re.iter().zip(im).zip(want).fold(
        (0.0_f64, 0.0_f64),
        |(signal, error), ((re, im), (want_re, want_im))| {
            let (error_re, error_im) = (f64::from(*re) - want_re, f64::from(*im) - want_im);
            (
                want_re.mul_add(*want_re, want_im.mul_add(*want_im, signal)),
                error_re.mul_add(error_re, error_im.mul_add(error_im, error)),
            )
        },
    );
    10.0 * (signal / error).log10()
}
