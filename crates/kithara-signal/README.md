<div align="center">

<img src="../../logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-signal.svg)](https://crates.io/crates/kithara-signal)
[![docs.rs](https://docs.rs/kithara-signal/badge.svg)](https://docs.rs/kithara-signal)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-signal

`kithara-signal` is Kithara's dependency-light decoded-audio data plane.

It owns the value types shared by decoders, playback, Warp, and streaming
analysis: format, owning chunk, timeline/provenance facts, frame/sample units,
and pure sample/time conversion. Pool-region mechanics remain in `kithara-bufpool`.
Encoded/container media facts remain in `kithara-stream`.

The crate has no decoder, network, asset, worker, scheduler, Warp, stretch,
player, analyzer, backend feature, or configuration responsibility.

See [CONTEXT.md](CONTEXT.md) for the ownership contract.
