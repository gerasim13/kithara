<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-stretch.svg)](https://crates.io/crates/kithara-stretch)
[![docs.rs](https://docs.rs/kithara-stretch/badge.svg)](https://docs.rs/kithara-stretch)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-stretch

Pure time-stretch DSP contracts and backend adapters for Kithara.

This crate owns the `ElasticEngine` exact-span and stream-lifecycle contract,
the backend selector and factory, and the native C++ adapters that implement it.
Backend features depend downward on `kithara-bufpool` for scratch storage, and
native builds include `kithara-workspace-hack`. `kithara-warp` owns the
synchronous renderer and temporal controls, `kithara-play` supplies the shared
pool region and composes post-Warp effects, decoded-audio values and audio-chunk
metadata belong to `kithara-signal`, and decoder sample-rate conversion remains
in the decode/audio seam.

Every engine reports its prepared frame limits and latency through
`ElasticCapabilities`, so callers plan against capabilities rather than against
a named library. Conformance cases exercise the operations those capabilities
allow.

Feature flags select the compiled backends:

- `stretch-signalsmith` enables `signalsmith-stretch` and is the default.
- `stretch-bungee` enables the private `bungee-sys` adapter as an opt-in backend.
- `stretch-glide` enables the pure-Rust Glide varispeed backend, including on Wasm.
- `stretch-identity` enables unity-rate sample copy without rate or keylock support.

Keylock-off rendering uses `build_varispeed_engine` with the existing Glide
resampler when the selected backend supports rate changes: exact source/output
spans change both duration and pitch. Identity remains a unity-rate copy in both
factories and allocates no pooled scratch. Keylock-on uses the selected native
engine. All engines share `ElasticEngine`; physical
input admission and the source advance represented by audible output are
explicit frame counts on `ElasticRequest`.

The two keylock backends are native-only. `BackendCapabilities` reports rate and
keylock support for each compiled backend; configuration intersects the requested
rate policy with those capabilities and the prepared frame limits. Identity-only
builds do not depend on the Glide resampler. See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-stretch)
for the backend contract and wasm notes.
