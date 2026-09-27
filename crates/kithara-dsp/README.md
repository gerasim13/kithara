<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-dsp.svg)](https://crates.io/crates/kithara-dsp)
[![docs.rs](https://docs.rs/kithara-dsp/badge.svg)](https://docs.rs/kithara-dsp)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-dsp

Vector DSP kernels over planar `f32` slices. The layout functions match
`fast_interleave` for `f32`: Apple builds run the stereo pair on Accelerate,
every other target runs it on `fearless_simd` at the best SIMD level the CPU
reports, and other channel counts take a bit-exact strided copy. `filter`
runs multichannel biquad cascades and `interp` reads a window at fractional
positions on the same backends. No kernel panics; only a filter's
constructor allocates.

## Usage

```rust
use std::num::NonZeroUsize;

// Two channels of three frames each, one after the other.
let planar = [1.0_f32, 2.0, 3.0, -1.0, -2.0, -3.0];
let stride = NonZeroUsize::new(3).expect("three frames per plane");
let stereo = NonZeroUsize::new(2).expect("two channels");
let mut interleaved = [0.0_f32; 6];
kithara_dsp::interleave_channel_major(&planar, stride, 0..3, &mut interleaved, stereo);
assert_eq!(
    interleaved.map(f32::to_bits),
    [1.0_f32, -1.0, 2.0, -2.0, 3.0, -3.0].map(f32::to_bits)
);

let mut samples = [f32::NAN, 0.5];
kithara_dsp::sanitize(&mut samples);
assert_eq!(samples, [0.0, 0.5]);
```

## Key Types

<table>

<tr><th>Item</th><th>Role</th></tr>

<tr><td><code>deinterleave_variable</code></td><td>One interleaved slice into planar channels; <code>fast_interleave</code>'s signature for <code>f32</code></td></tr>

<tr><td><code>interleave_channel_major</code></td><td>Every plane of one strided channel-major slice into one interleaved slice, at any channel count without a slice of plane references</td></tr>

<tr><td><code>deinterleave_channel_major</code></td><td>One interleaved slice into every plane of one strided channel-major slice</td></tr>

<tr><td><code>sanitize</code></td><td>Zeroes NaN, ±infinity, subnormals and −0.0 in place</td></tr>

<tr><td><code>filter::Biquad</code></td><td>Biquad sections in cascade over planar channels; <code>new</code> allocates, <code>retune</code>, <code>process</code>, <code>settle</code> and <code>copy_state</code> do not, and silence resets the state</td></tr>

<tr><td><code>filter::Coefficients</code></td><td>The <code>biquad</code> crate's RBJ cookbook designs (with <code>Type</code>, <code>Hertz</code> and <code>Errors</code>), re-exported as the one import path</td></tr>

<tr><td><code>interp::interpolate</code></td><td>Reads a window at fractional positions with an <code>interp::Interpolation</code> method (linear, quadratic, Hermite, Watte); a position outside the window is an error and leaves the output untouched</td></tr>

<tr><td><code>interp::RateRamp</code></td><td>Read rate moving linearly to a target; places a block of positions in closed form and lands on the target exactly</td></tr>

<tr><td><code>fade::FadeCurve</code></td><td>firewheel's fade curve, re-exported as the one import path</td></tr>

<tr><td><code>param::*</code></td><td>firewheel's parameter smoother, smoothing filter and A/B mix, re-exported as the one import path</td></tr>

</table>

## Integration

`kithara-signal` interleaves and deinterleaves its buffers through the layout
functions; its pooled planar buffer goes through the channel-major ones, so
no channel count allocates. `kithara-resampler`'s Glide backend filters
through `filter::Biquad`, interpolates through `interp::interpolate` and places
its positions with `interp::RateRamp`. `kithara-decode` sanitizes resampled
planes. The build target picks the backend at compile time; on x86 the SIMD
level is detected once per process, so a call costs one load before the kernel
runs.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-dsp) for detailed contracts, invariants, and internals.
