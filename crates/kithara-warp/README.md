<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-warp.svg)](https://crates.io/crates/kithara-warp)
[![docs.rs](https://docs.rs/kithara-warp/badge.svg)](https://docs.rs/kithara-warp)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-warp

Beat-map synchronization contracts and the source-generic resident Warp decorator for Kithara.

The crate owns musical coordinates, immutable beat-map snapshots, group
topology, alignment plans, synchronization operations, `Warp<S>`, `WarpConfig`,
live temporal controls, and the synchronous `WarpRenderer<S>` that drives a
`kithara-stretch::ElasticEngine` when one is available and otherwise preserves
decoded audio through the same renderer contract. It does not decode audio,
own source lifecycle, own `Player` / `PlayWorker` / Host/session state, access
storage, or analyze samples.

See [CONTEXT.md](CONTEXT.md) for ownership and dependency boundaries.
