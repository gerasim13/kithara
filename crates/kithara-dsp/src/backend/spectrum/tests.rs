use std::f32::consts::TAU;

use fearless_simd::{Level, dispatch};
use kithara_test_utils::{bufpool::pools, kithara};

use super::{Dft, Work, kernels, magnitude};
#[cfg(any(target_os = "macos", target_os = "ios"))]
use crate::backend::accelerate;
use crate::{
    backend::phase::{phase, phase_kernel},
    spectrum::{FftLen, SpectrumError, oracle},
};

/// Largest `|X|` error against `hypot`, relative: two `f32` epsilons.
const MAGNITUDE_PARITY: f32 = 2.0 * f32::EPSILON;
/// Largest `arg` error against `atan2`, in radians on the circle.
const PHASE_PARITY: f32 = 1e-6;
const LENGTHS: [usize; 7] = [16, 48, 80, 240, 1024, 3072, 4096];
const RADII: [f32; 3] = [1.0e-3, 1.0, 4096.0];
const STEPS: u16 = 64;
const UNWRITTEN: f32 = -1.0;

type BinKernel = fn(&[f32], &[f32], &mut [f32]) -> usize;
type Forward = fn(FftLen, &[f32]) -> Result<[Vec<f32>; 2], SpectrumError>;

/// One kernel reading every bin, so every check runs the same points on each.
struct Kernel {
    name: &'static str,
    run: BinKernel,
}

fn magnitudes() -> Vec<Kernel> {
    Vec::from([
        Kernel {
            name: "portable-native",
            run: magnitude,
        },
        Kernel {
            name: "portable-fallback",
            run: |re, im, output| dispatch!(Level::fallback(), simd => kernels::magnitude_kernel(simd, re, im, output)),
        },
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        Kernel {
            name: "accelerate",
            run: accelerate::magnitude,
        },
    ])
}

/// Every platform reads the phase through one kernel, so its rows are the
/// SIMD levels.
fn phases() -> [Kernel; 2] {
    [
        Kernel {
            name: "native",
            run: phase,
        },
        Kernel {
            name: "fallback",
            run: |re, im, output| dispatch!(Level::fallback(), simd => phase_kernel(simd, re, im, output)),
        },
    ]
}

/// Points around the circle at three radii, then the four signed zeros and
/// the axes; the count is no multiple of a vector, so the tail runs too.
fn points() -> (Vec<f32>, Vec<f32>) {
    let circle = (0..STEPS).flat_map(|step| {
        let angle = f32::from(step) * (TAU / f32::from(STEPS));
        RADII.map(|radius| (radius * angle.cos(), radius * angle.sin()))
    });
    let edges = [
        (0.0, 0.0),
        (-0.0, 0.0),
        (0.0, -0.0),
        (-0.0, -0.0),
        (1.0, 0.0),
        (-1.0, 0.0),
        (-1.0, -0.0),
        (0.0, 1.0),
        (0.0, -1.0),
    ];
    circle.chain(edges).unzip()
}

/// Distance between two angles on the circle, in radians.
fn angular_distance(a: f32, b: f32) -> f32 {
    let difference = a - b;
    (difference - TAU * (difference / TAU).round()).abs()
}

#[kithara::test]
fn magnitude_tracks_hypot_on_every_backend() {
    let (re, im) = points();
    for Kernel { name, run } in magnitudes() {
        let mut output = vec![UNWRITTEN; re.len()];
        assert_eq!(run(&re, &im, &mut output), re.len(), "{name}");
        for ((x, y), got) in re.iter().zip(&im).zip(&output) {
            let want = x.hypot(*y);
            assert!(
                (got - want).abs() <= MAGNITUDE_PARITY * want,
                "{name}: |{x} + i{y}| = {got}, hypot {want}"
            );
        }
    }
}

#[kithara::test]
fn phase_tracks_atan2_at_every_simd_level() {
    let (re, im) = points();
    for Kernel { name, run } in phases() {
        let mut output = vec![UNWRITTEN; re.len()];
        assert_eq!(run(&re, &im, &mut output), re.len(), "{name}");
        for ((x, y), got) in re.iter().zip(&im).zip(&output) {
            let want = y.atan2(*x);
            assert!(
                angular_distance(*got, want) <= PHASE_PARITY,
                "{name}: arg({x} + i{y}) = {got}, atan2 {want}"
            );
        }
        let mut origin = [UNWRITTEN];
        assert_eq!(run(&[0.0], &[0.0], &mut origin), 1);
        assert_eq!(
            origin.map(f32::to_bits),
            [0.0_f32.to_bits()],
            "{name}: arg(0) is +0"
        );
    }
}

fn portable_forward(len: FftLen, signal: &[f32]) -> Result<[Vec<f32>; 2], SpectrumError> {
    let dft = Dft::try_from(len)?;
    let mut work: Work = dft.work(&pools()).expect("the work fits the region");
    work.input_mut().copy_from_slice(signal);
    let (mut re, mut im) = (vec![UNWRITTEN; len.bins()], vec![UNWRITTEN; len.bins()]);
    dft.forward(&mut work, [&mut re, &mut im])?;
    Ok([re, im])
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn accelerate_forward(len: FftLen, signal: &[f32]) -> Result<[Vec<f32>; 2], SpectrumError> {
    let dft = accelerate::Dft::try_from(len)?;
    let mut work: accelerate::Work = dft.work(&pools()).expect("the work fits the region");
    work.input_mut().copy_from_slice(signal);
    let (mut re, mut im) = (vec![UNWRITTEN; len.bins()], vec![UNWRITTEN; len.bins()]);
    dft.forward(&mut work, [&mut re, &mut im])?;
    Ok([re, im])
}

/// One backend's DFT and the window gain that undoes its scaling.
struct Transform {
    name: &'static str,
    forward: Forward,
    scale: f32,
}

fn transforms() -> Vec<Transform> {
    Vec::from([
        Transform {
            name: "portable",
            forward: portable_forward,
            scale: Dft::WINDOW_SCALE,
        },
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        Transform {
            name: "accelerate",
            forward: accelerate_forward,
            scale: accelerate::Dft::WINDOW_SCALE,
        },
    ])
}

#[kithara::test]
fn every_backend_dft_times_its_window_scale_tracks_a_direct_dft() {
    let rows = transforms();
    for len in LENGTHS {
        let signal = oracle::mix(len);
        let want = oracle::dft(&signal);
        let len = FftLen::new(len).expect("the test lengths are FFT lengths");
        for row in &rows {
            let name = row.name;
            let scaled = |plane: Vec<f32>| -> Vec<f32> {
                plane.into_iter().map(|value| value * row.scale).collect()
            };
            let [re, im] = (row.forward)(len, &signal).expect("every FFT length builds");
            let snr = oracle::snr([&scaled(re), &scaled(im)], &want);
            assert!(
                snr >= oracle::DFT_SNR_DB,
                "{name} at {}: {snr} dB",
                len.get()
            );
        }
    }
}
