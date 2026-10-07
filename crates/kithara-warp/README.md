<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-warp.svg)](https://crates.io/crates/kithara-warp)
[![docs.rs](https://docs.rs/kithara-warp/badge.svg)](https://docs.rs/kithara-warp)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-warp

Beat-map geometry and the source-generic resident Warp decorator for Kithara.

The crate owns musical coordinates, immutable beat-map snapshots, beat
alignment edges, the presentation frontier, `Warp<S>`, `WarpConfig`, live
temporal controls, and the synchronous `WarpRenderer<S>` that drives a
`kithara-stretch::ElasticEngine`. Select at least one backend feature:
`stretch-identity`, `stretch-glide`, `stretch-signalsmith`, or `stretch-bungee`.
Identity passes decoded audio through; DSP backends expose rate and keylock
through their capabilities. It does not decode audio, own source lifecycle,
own `Player` / `PlayWorker` / Host/session state, access storage, or analyze
samples.

`WarpMap::projected` uses stamped source/session grid alignment; grid
materialization stays outside rendering. The renderer follows the speed its
render lane sets on a frame through the existing smoother. Worker-side backend
preparation selects Signalsmith/Bungee for keylock and Glide varispeed when
keylock is off. Engine selection checks backend capabilities at runtime.
Identity preserves the original sample buffer at every requested speed and does
not support region plans or keylock. DSP backends preserve decoded samples at unity. The `render` feature
exposes the renderer; geometry and configuration remain available without it.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-warp) for ownership and dependency boundaries.
