<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-render.svg)](https://crates.io/crates/kithara-render)
[![docs.rs](https://docs.rs/kithara-render/badge.svg)](https://docs.rs/kithara-render)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-render

`kithara-render` owns the existing playback worker, realtime player node,
Host transport processor, metronome, output taps, and the bridge state shared
with their control owners. The extracted implementations keep their current
Firewheel topology, commands, staging, and sample behavior.

`kithara-play` prepares sources and decides playback policy. It hands a
prepared `RenderResource` to this crate without making rendering depend on
source selection. `kithara-host` owns the session, graph construction, and
audio device lifecycle. Generic dispatch remains in `kithara-worker`; source
and DSP algorithms remain in their existing crates.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-render).
