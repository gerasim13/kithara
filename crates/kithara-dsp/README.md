<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-dsp.svg)](https://crates.io/crates/kithara-dsp)
[![docs.rs](https://docs.rs/kithara-dsp/badge.svg)](https://docs.rs/kithara-dsp)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-dsp

Vector DSP kernels over planar `f32` slices. The layout functions mirror
`fast_interleave` for `f32`: Apple builds run the stereo pair on Accelerate,
every other target runs it on `fearless_simd` at the best SIMD level the CPU
reports, and other channel counts take a bit-exact strided copy. No kernel
allocates or panics.

## Usage

```rust
use std::num::NonZeroUsize;

let planes = [[1.0_f32, 2.0], [-1.0, -2.0]];
let mut interleaved = [0.0_f32; 4];
let stereo = NonZeroUsize::new(2).expect("two channels");
kithara_dsp::interleave_variable(&planes, 0..2, &mut interleaved, stereo);
assert_eq!(interleaved.map(f32::to_bits), [1.0_f32, -1.0, 2.0, -2.0].map(f32::to_bits));

let mut samples = [f32::NAN, 0.5];
kithara_dsp::sanitize(&mut samples);
assert_eq!(samples, [0.0, 0.5]);
```

## Key Types

<table>

<tr><th>Item</th><th>Role</th></tr>

<tr><td><code>interleave_variable</code></td><td>Planar channels into one interleaved slice; <code>fast_interleave</code>'s signature for <code>f32</code></td></tr>

<tr><td><code>deinterleave_variable</code></td><td>One interleaved slice into planar channels; <code>fast_interleave</code>'s signature for <code>f32</code></td></tr>

<tr><td><code>sanitize</code></td><td>Zeroes NaN, ±infinity, subnormals and −0.0 in place</td></tr>

<tr><td><code>fade::FadeCurve</code></td><td>firewheel's fade curve, re-exported as the one import path</td></tr>

<tr><td><code>param::*</code></td><td>firewheel's parameter smoother, smoothing filter and A/B mix, re-exported as the one import path</td></tr>

</table>

## Integration

`kithara-signal` interleaves and deinterleaves its buffers through the layout
functions, and `kithara-decode` sanitizes resampled planes. The build target
picks the backend at compile time; on x86 the SIMD level is detected once per
process, so a call costs one load before the kernel runs.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-dsp) for detailed contracts, invariants, and internals.
