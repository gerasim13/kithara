<div align="center">

<img src="../../logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-worker.svg)](https://crates.io/crates/kithara-worker)
[![docs.rs](https://docs.rs/kithara-worker/badge.svg)](https://docs.rs/kithara-worker)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-worker

Domain-free building blocks for prioritized, cancellable worker dispatch.

The crate owns scheduler threads, admission limits, task priority, bounded
compute submission, and cancellation ancestry. Playback, analysis, storage,
and other domain behavior remain in their owning crates.
