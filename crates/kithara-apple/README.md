<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-apple.svg)](https://crates.io/crates/kithara-apple)
[![docs.rs](https://docs.rs/kithara-apple/badge.svg)](https://docs.rs/kithara-apple)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-apple

Apple platform ABI and safe wrappers shared by Kithara crates.

This crate owns raw AudioToolbox, Accelerate, and Foundation binding surfaces.
Higher-level crates use its typed wrappers or re-exported Apple framework types
instead of declaring local Apple FFI structs, externs, or binding dependencies.
Codec policy remains in `kithara-decode`; resampler algorithms remain in
`kithara-resampler`; HTTP semantics remain in `kithara-net`.
