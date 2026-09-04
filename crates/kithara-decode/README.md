<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-decode.svg)](https://crates.io/crates/kithara-decode)
[![docs.rs](https://docs.rs/kithara-decode/badge.svg)](https://docs.rs/kithara-decode)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-decode

Audio decoding library with explicit, typed backend selection. `DecoderFactory` creates synchronous `Decoder` instances that convert compressed audio (MP3, AAC, FLAC, WAV, ALAC, …) into pool-backed decoded-audio chunks. No threading, no channels — just decoding.

The public surface centres on one trait - `Decoder`. Concrete backends (Symphonia / Apple / Android) implement it directly. Internally, container parsing and frame decoding are split: the `Demuxer` trait owns container framing, the `FrameCodec` trait owns codec decoding, and `ComposedDecoder<D, C, S>` (internal) pairs them with the registered pool schema so backends can be mixed and matched. The factory hides this detail - callers only ever see `Box<dyn Decoder>`.

## Usage

```rust
use std::io::Cursor;
use kithara_bufpool::{OverallBudget, PoolConfig, pool_schema};
use kithara_decode::{DecoderBackend, DecoderConfig, DecoderFactory};

pool_schema! {
    pub AppPools {
        bytes: u8,
        samples: f32,
    }
}

let pool_config = || PoolConfig::builder().max_buffers(32).build();
let pools = AppPools::builder(OverallBudget(64 * 1024 * 1024))
    .bytes(pool_config())
    .samples(pool_config())
    .build()?;
let reader = Cursor::new(wav_bytes);
let config = DecoderConfig::builder()
    .backend(DecoderBackend::Symphonia)
    .pools(pools)
    .build();
let mut decoder = DecoderFactory::create_with_probe(reader, Some("wav"), config)?;

let spec = decoder.spec(); // sample_rate, channels
loop {
    match decoder.next_chunk()? {
        kithara_decode::DecoderChunkOutcome::Chunk(chunk) => play(&chunk.samples),
        kithara_decode::DecoderChunkOutcome::Pending(_) => continue,
        kithara_decode::DecoderChunkOutcome::Eof => break,
    }
}
```

For HLS / cross-codec recreate paths, prefer `DecoderFactory::create_from_media_info(reader, &media_info, config)` — it skips probing and uses the carried `MediaInfo` to pick the backend.

## Features

<table>

<tr><th>Feature</th><th>Backend</th><th>Implementation</th><th>Platform</th></tr>

<tr><td><code>symphonia</code></td><td>Symphonia</td><td>Software decoding; all formats</td><td>Cross-platform</td></tr>

<tr><td><code>apple</code></td><td>Apple AudioToolbox</td><td>Hardware-accelerated; fMP4, ADTS, MP3, FLAC, CAF</td><td>macOS / iOS</td></tr>

<tr><td><code>android</code></td><td>Android MediaCodec</td><td>Hardware path for fMP4 AAC-LC/FLAC plus standalone WAV, MP3, and ALAC through <code>AMediaExtractor</code>; no runtime Symphonia fallback</td><td>Android</td></tr>

</table>

## Integration

`kithara-play` owns the playback worker and effects. Decoder sample-rate conversion is configured
through `kithara-audio` and implemented with `kithara-resampler`; this crate stays a synchronous
decoder over `R: Read + Seek + Send + Sync + 'static` inputs such as `Stream<File<S>>`, `Stream<Hls<S>>`,
cursors, or plain files. Shared `AudioSpec`, `AudioChunkInfo`, `AudioChunk`, frame/sample units, and
pure sample/time math are owned by `kithara-signal`; decoder-specific profiles and errors remain here.

See [CONTEXT.md](CONTEXT.md) for detailed contracts, invariants, and internals.
