<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-render.svg)](https://crates.io/crates/kithara-render)
[![docs.rs](https://docs.rs/kithara-render/badge.svg)](https://docs.rs/kithara-render)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-render

The producer-side render stage of Kithara playback. A decoded audio source
passes through its Warp renderer and effect chain here, one render quantum at
a time, before the result enters the play output ring.

## Usage

```rust,ignore
use kithara_render::WarpSource;

let stage = WarpSource::new(source, renderer, effects, drain, spec, pools);
// `stage` is itself an `AudioSource`: the play worker steps it like any
// decoded source and writes what it produces into the output ring.
```

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>WarpSource</code></td><td>Steps a decoded source through Warp and effects, keeps staged input across seeks and drains, and reports where a lane entering its plan starts decoding</td></tr>

</table>

## Features

| Feature | Effect |
| --- | --- |
| `stretch-signalsmith` | Signalsmith time-stretch backend in the Warp renderer |
| `stretch-bungee` | Bungee time-stretch backend in the Warp renderer |
| `stretch-glide` | Glide time-stretch backend in the Warp renderer |

## Integration

`kithara-play` builds one `WarpSource` per loaded track on its worker and
steps it from the decoder node. The crate depends on the engine layer
(`kithara-audio`, `kithara-effects`, `kithara-warp`) and on nothing in the
player.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-render) for detailed contracts, invariants, and internals.
