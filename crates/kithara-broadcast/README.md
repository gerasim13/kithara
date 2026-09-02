<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-broadcast

Live HLS origin. It encodes master PCM to AAC-LC, frames the access units as ADTS behind the RFC 8216 section 3.4 timestamp tag, rotates segments on the media clock, keeps a sliding playlist window, and serves master playlist, media playlist, and segments over HTTP. Segments live in memory as `bytes::Bytes`.

## Usage

```rust
use kithara_broadcast::{Broadcast, BroadcastConfig};
use kithara_output::LiveOutput;
use kithara_worker::{Worker, WorkerConfig};

let config = BroadcastConfig::builder().sample_rate(48_000).channels(2).build();
let worker = Worker::new(WorkerConfig::new());
let (mut output, handle) = Broadcast::start(&worker, &pools, &config, Some(parent))?;

output.write_stereo(frames, left, right);

println!("on air at {}", handle.url());
handle.stop();
```

The packaging core is usable on its own - `Segmenter` and `LiveWindow` take the same config and own no threads:

```rust
use kithara_broadcast::{BroadcastConfig, LiveWindow, Segmenter};

let config = BroadcastConfig::builder().build();
let mut segmenter = Segmenter::new(&config)?;
let mut window = LiveWindow::new(&config)?;

for unit in encoder.push(&samples)? {
    if let Some(segment) = segmenter.push(&unit)? {
        window.push(segment);
    }
}

let snapshot = window.snapshot();
```

## Key Types

- `BroadcastConfig` - the audio, the segments, and the address the origin binds.
- `Broadcast` / `BroadcastHandle` - the live service: URL, status, and the graceful end of the broadcast.
- `BroadcastOutput` - the bounded non-blocking stereo `LiveOutput` installed in the master output group.
- `Segmenter` - ADTS framing plus segment rotation on the media clock.
- `Segment` - one closed segment: sequence number, bytes, duration, discontinuity flag.
- `LiveWindow` - sole owner of the playlist window, its retention, and the playlist text.
- `PlaylistSnapshot` - value view of the stream: playlist text, fetchable segments, end-of-stream flag.

## Integration

Takes access units from `kithara-encode`. The packaging core builds on wasm32;
the encoder, worker intake, and HTTP origin are native-only.

See [CONTEXT.md](CONTEXT.md) for detailed contracts, invariants, and internals.
