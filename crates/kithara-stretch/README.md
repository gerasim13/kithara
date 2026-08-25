<div align="center">

<img src="../../logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-stretch.svg)](https://crates.io/crates/kithara-stretch)
[![docs.rs](https://docs.rs/kithara-stretch/badge.svg)](https://docs.rs/kithara-stretch)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-stretch

Pure time-stretch DSP contracts and backend adapters for Kithara.

This crate owns the `StretchBackend` trait for the streaming stretch slot, the
`ElasticEngine` / `ElasticPriming` traits for exact-span rendering, the backend
selector and factory, and the native C++ adapters that implement them. The
crate depends downward on `kithara-bufpool` for injected scratch storage, and
native builds include `kithara-workspace-hack`; audio graph plumbing, region
planning, chunk metadata, and resampler routing stay in `kithara-audio`.

Every compiled-in engine runs the same exact-span conformance suite, and each
declares its own rate window and latency through `ElasticCapabilities`, so
callers plan against capabilities rather than against a named library.

Feature flags select the compiled backends:

- `stretch-signalsmith` enables `signalsmith-stretch` and is the default; it is
  unavailable on wasm targets.
- `stretch-bungee` enables `bungee-rs` as an opt-in backend; it is unavailable
  on wasm and Windows MSVC targets.

The crate intentionally fails to compile when no backend feature is enabled or
when an enabled backend is unsupported by the target. See
[CONTEXT.md](CONTEXT.md) for the backend contract, complete target rules, and
the future pure-Rust backend recipe.
