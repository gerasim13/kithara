<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-output.svg)](https://crates.io/crates/kithara-output)
[![docs.rs](https://docs.rs/kithara-output/badge.svg)](https://docs.rs/kithara-output)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-output

Neutral master-output protocols shared by hosts and output consumers.

The current public slice defines exact finite offline rendering:

- `OfflineRenderRequest` names an absolute output-frame range and signal format.
- `OfflineRenderer` drives the owner graph in bounded configured blocks.
- `RenderSink` receives interleaved `f32` PCM without storage assumptions.

Encoding, filesystems, networking, and Firewheel remain in their owning crates.
See [CONTEXT.md](CONTEXT.md) for lifecycle invariants.
