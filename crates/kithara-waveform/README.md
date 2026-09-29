<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-waveform.svg)](https://crates.io/crates/kithara-waveform)
[![docs.rs](https://docs.rs/kithara-waveform/badge.svg)](https://docs.rs/kithara-waveform)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-waveform

`kithara-waveform` owns the three-band waveform: its value type, its stored
byte form, its tunables, and the streaming analyzer that produces it.

The model, its codec, its parameters, and its resume record are available
without the `dsp` feature, so a consumer that only displays or transports a
waveform runs no analysis. The parameters name their FFT length as a
`kithara_dsp::spectrum::FftLen`, so `kithara-dsp` is always a dependency; only
`dsp` turns on its `spectrum` feature. Enabling `dsp` adds the analyzer, which
takes its FFT, band energies and downmix from `kithara-dsp` and re-enters a
stopped pass from its resume record exactly where it left off.

Frame-range coverage belongs to `kithara-signal` and the versioned blob
framing to `kithara-blob`; scheduling, persistence, and the artifact envelope
belong to `kithara-analysis`.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-waveform) for the ownership contract.
