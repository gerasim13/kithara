use kithara_bufpool::{HasPool, PoolError, PoolRegion, SampleBuffer};
use kithara_dsp::spectrum::{self, Fft, Spectrum, SpectrumError};

use super::consts;

/// Complex spectral difference, one value per [`consts::FRAMES_HOP`].
pub(crate) struct Novelty<S>
where
    S: HasPool<f32>,
{
    fft: Fft,
    pools: PoolRegion<S>,
}

/// What one curve needs while it runs. A spectrum is predicted from the frame
/// before it, so these carry across frames within a call and nothing beyond it.
struct Frames {
    spectrum: Spectrum,
    observed: SampleBuffer,
    angle: SampleBuffer,
    magnitude: SampleBuffer,
    phase: SampleBuffer,
    phase_step: SampleBuffer,
}

impl<S> Novelty<S>
where
    S: HasPool<f32>,
{
    pub(crate) fn new(pools: PoolRegion<S>) -> Result<Self, SpectrumError> {
        Ok(Self {
            fft: Fft::new(consts::NOVELTY_FFT)?,
            pools,
        })
    }

    /// The difference is measured every [`consts::NOVELTY_STRIDE`]
    /// samples and interpolated onto the [`consts::FRAMES_HOP`] grid,
    /// the resolution the reference reaches the same way. The last window
    /// is filled out with zeros, and the first window that needs that is
    /// the last.
    pub(crate) fn curve(&self, mono: &[f32]) -> Result<SampleBuffer, PoolError> {
        if mono.len() < consts::FRAMES_FRAME {
            return Ok(self.pools.get::<f32>());
        }
        let frames = (mono.len() - consts::FRAMES_FRAME) / consts::NOVELTY_STRIDE + 2;
        let mut coarse = self.pools.get_with_len::<f32>(frames)?;
        let bins = consts::NOVELTY_FFT.bins();
        let mut work = Frames {
            spectrum: self.fft.spectrum(&self.pools)?,
            observed: self.pools.get_with_len::<f32>(bins)?,
            angle: self.pools.get_with_len::<f32>(bins)?,
            magnitude: self.pools.get_with_len::<f32>(bins)?,
            phase: self.pools.get_with_len::<f32>(bins)?,
            phase_step: self.pools.get_with_len::<f32>(bins)?,
        };
        for (index, slot) in coarse.iter_mut().enumerate() {
            let at = index * consts::NOVELTY_STRIDE;
            let end = (at + consts::FRAMES_FRAME).min(mono.len());
            *slot = work.difference(&self.fft, &mono[at..end]);
        }
        let mut curve = self.pools.get_with_len::<f32>(
            (coarse.len() - 1) * (consts::NOVELTY_STRIDE / consts::FRAMES_HOP) + 1,
        )?;
        for (index, slot) in curve.iter_mut().enumerate() {
            let (whole, half) = (index / 2, index % 2 == 1);
            *slot = if half {
                (coarse[whole] + coarse[whole + 1]) / 2.0
            } else {
                coarse[whole]
            };
        }
        Ok(curve)
    }
}

impl Frames {
    /// Distance between this frame's spectrum and the one predicted from the
    /// two before it; `0.0` when the frame does not fit the transform.
    fn difference(&mut self, fft: &Fft, frame: &[f32]) -> f32 {
        if fft.forward(frame, &mut self.spectrum).is_err() {
            return 0.0;
        }
        let (re, im) = (self.spectrum.re(), self.spectrum.im());
        let _ = spectrum::magnitude(re, im, &mut self.observed);
        let _ = spectrum::phase(re, im, &mut self.angle);
        let mut total = 0.0;
        for ((((re, im), (observed, angle)), (magnitude, phase)), step) in re
            .iter()
            .zip(im)
            .zip(self.observed.iter().zip(self.angle.iter()))
            .zip(self.magnitude.iter_mut().zip(self.phase.iter_mut()))
            .zip(self.phase_step.iter_mut())
        {
            let (sin, cos) = wrap(*phase + *step).sin_cos();
            total += (re - *magnitude * cos).hypot(im - *magnitude * sin);
            *step = wrap(angle - *phase);
            *phase = *angle;
            *magnitude = *observed;
        }
        total
    }
}

fn wrap(angle: f32) -> f32 {
    let turn = std::f32::consts::TAU;
    angle - turn * (angle / turn).round()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use kithara_test_fixtures::unit_fixtures::{click_silence_4s, clicks_120_4s};
    use kithara_test_utils::kithara;
    use num_traits::cast::ToPrimitive;

    use super::*;
    use crate::{
        dsp::{clicks, frames},
        test_pools::pools,
    };

    fn peaks(curve: &[f32]) -> Vec<usize> {
        let ceiling = curve.iter().copied().fold(0.0f32, f32::max);
        (1..curve.len().saturating_sub(1))
            .filter(|&i| {
                curve[i] > ceiling * 0.4 && curve[i] >= curve[i - 1] && curve[i] > curve[i + 1]
            })
            .collect()
    }

    #[kithara::test(native, flash(false))]
    fn clicks_raise_peaks_where_the_clicks_are(clicks_120_4s: Vec<f32>) {
        let pcm = clicks_120_4s;
        let curve = Novelty::new(pools())
            .expect("the novelty FFT length is supported")
            .curve(&pcm)
            .expect("the curve fits the region");

        let found: Vec<f32> = peaks(&curve)
            .into_iter()
            .map(|i| frames::seconds(i.to_f32().unwrap_or(0.0)))
            .collect();
        // The click at zero lands on the first frame, where a peak has no
        // left neighbour to stand above.
        let first_observable = consts::FRAMES_FRAME.to_f32().unwrap_or(0.0) / consts::FRAMES_RATE;
        let expected: Vec<f32> = clicks::positions(4.0, 0.5)
            .into_iter()
            .filter(|at| *at >= first_observable)
            .collect();
        assert_eq!(
            found.len(),
            expected.len(),
            "one novelty peak per click: {found:?} vs {expected:?}"
        );
        for (got, want) in found.iter().zip(expected.iter()) {
            assert!(
                (got - want).abs() < 0.03,
                "peak at {got} s should land on the click at {want} s"
            );
        }
    }

    #[kithara::test(native, flash(false))]
    fn silence_is_flat(click_silence_4s: Vec<f32>) {
        let curve = Novelty::new(pools())
            .expect("the novelty FFT length is supported")
            .curve(&click_silence_4s)
            .expect("the curve fits the region");
        assert!(!curve.is_empty(), "silence still yields a curve");
        assert!(
            curve.iter().all(|&v| v == 0.0),
            "silence has no spectral change"
        );
    }

    fn complete_curve_bits<S: HasPool<f32>>(novelty: &Novelty<S>, pcm: &[f32]) -> Vec<u32> {
        let curve = novelty
            .curve(pcm)
            .expect("the complete curve fits the region");
        assert!(curve.iter().all(|value| value.is_finite()));
        curve.iter().map(|value| value.to_bits()).collect()
    }

    #[kithara::test(native)]
    fn complete_curves_are_independent_of_previous_audio(
        clicks_120_4s: Vec<f32>,
        click_silence_4s: Vec<f32>,
    ) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/track_excerpt_mono_22050.f32le");
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let (words, remainder) = bytes.as_chunks::<4>();
        assert!(
            remainder.is_empty(),
            "the fixture contains complete samples"
        );
        let frames = 22_050 * 30;
        assert!(
            words.len() >= frames,
            "the full benchmark excerpt is present"
        );
        let track: Vec<f32> = words[..frames]
            .iter()
            .map(|word| f32::from_le_bytes(*word))
            .collect();
        assert_eq!(clicks_120_4s.len(), 22_050 * 4);
        assert_eq!(click_silence_4s.len(), 22_050 * 4);

        let novelty = Novelty::new(pools()).expect("the novelty FFT length is supported");
        for (name, pcm, other, expected_len) in [
            (
                "track_30s",
                track.as_slice(),
                clicks_120_4s.as_slice(),
                2_581,
            ),
            (
                "clicks_120_4s",
                clicks_120_4s.as_slice(),
                click_silence_4s.as_slice(),
                343,
            ),
            (
                "silence_4s",
                click_silence_4s.as_slice(),
                track.as_slice(),
                343,
            ),
        ] {
            let first = complete_curve_bits(&novelty, pcm);
            // Drop pooled curves before the next call so their storage can be reused.
            let _ = complete_curve_bits(&novelty, other);
            let reused = complete_curve_bits(&novelty, pcm);
            let fresh = complete_curve_bits(
                &Novelty::new(pools()).expect("a fresh novelty transform builds"),
                pcm,
            );
            assert_eq!(first.len(), expected_len, "{name}: complete curve length");
            assert_eq!(
                first, reused,
                "{name}: another input changed the full curve"
            );
            assert_eq!(first, fresh, "{name}: fresh and reused curves differ");
        }
    }
}
