<div align="center">

<img src="../../logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-encode

Audio encoding library with a portable continuous PCM/WAV path, optional native encoders, and storage-neutral container writes. `EncoderSession` and `ContainerSession` work on wasm32 without a filesystem; `StreamEncoder` provides native streaming AAC-LC; `EncoderFactory` creates finite packaged tracks and encoded bytes.

## Usage

```rust
use kithara_encode::{ContainerSession, EncodeConfig, EncoderSession};

let config = EncodeConfig::builder()
    .sample_rate(48_000)
    .channels(2)
    .build();
let encoder = EncoderSession::new(&config)?;
let container = ContainerSession::new(&config)?;
```

## Key types

- `EncodeConfig` — `bon` configuration; defaults to PCM/WAV float32.
- `EncoderSession` — portable continuous interleaved-f32 encoder.
- `ContainerSession` / `ContainerWrite` — portable container state and absolute, storage-neutral byte writes.
- `StreamEncoder` — native streaming AAC-LC encoder; interleaved f32 in, access units out.
- `EncoderFactory` — entry point; creates byte-oriented and packaged encoders.
- `InnerEncoder` — encoder trait returned by the factory.
- `BytesEncodeRequest` / `BytesEncodeTarget` — byte-encoding inputs.
- `PackagedEncodeRequest` — packaged access-unit encoding input.
- `EncodedBytes` / `EncodedTrack` — encoded outputs (complete bytes and packaged access units).

Consumes canonical media types from `kithara-stream`. Unsupported codec/container profiles fail explicitly; they are never substituted. The portable PCM/WAV path is available with no native encoder features, while FFmpeg and fdk-aac remain optional native backends.

See [CONTEXT.md](CONTEXT.md) for detailed contracts, invariants, and internals.
