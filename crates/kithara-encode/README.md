<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

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

## Key Types

- `EncodeConfig` — `bon` configuration; defaults to PCM/WAV float32.
- `EncoderSession` — portable continuous interleaved-f32 encoder.
- `ContainerSession` / `ContainerWrite` — portable container state and absolute, storage-neutral byte writes.
- `StreamEncoder` — native streaming AAC-LC encoder; interleaved f32 in, access units out.
- `EncoderFactory` — entry point; encodes to complete bytes or to packaged access units.
- `InnerEncoder` — encoder trait returned by the factory.
- `BytesEncodeRequest` / `BytesEncodeTarget` — byte-encoding inputs.
- `PackagedEncodeRequest` — packaged access-unit encoding input.
- `EncodedBytes` / `EncodedTrack` — encoded outputs (complete bytes and packaged access units).

## Integration

Consumes canonical media types from `kithara-stream`. Unsupported codec/container profiles fail explicitly; they are never substituted. The portable PCM/WAV path is available with no native encoder features, while FFmpeg and fdk-aac remain optional native backends. The streaming path feeds live broadcast; the packaged and byte paths generate encoded fixtures and packaged tracks for the integration harness.

See [CONTEXT.md](CONTEXT.md) for detailed contracts, invariants, and internals.
