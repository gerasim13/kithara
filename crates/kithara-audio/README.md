<div align="center">

<img src="../../logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-audio.svg)](https://crates.io/crates/kithara-audio)
[![docs.rs](https://docs.rs/kithara-audio/badge.svg)](https://docs.rs/kithara-audio)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](../../LICENSE-MIT)

</div>

# kithara-audio

Audio pipeline with decoding, effects, resampling, native time-stretch, and
source-signal analysis. `Audio<S>` is the PCM reader surface. Playback task
registration and the shared OS thread are owned by `kithara-play::PlayWorker`;
this crate supplies the PCM task and lock-free reader/runtime capabilities.

## Features

<table>

<tr><th>Feature</th><th>Default</th><th>Effect</th></tr>

<tr><td><code>default</code></td><td>yes</td><td><code>symphonia</code> + <code>stretch-signalsmith</code> + <code>client-reqwest</code> + <code>tls-rustls</code></td></tr>

<tr><td><code>symphonia</code></td><td>yes</td><td>Symphonia software decoder path via <code>kithara-decode/symphonia</code></td></tr>

<tr><td><code>stretch-signalsmith</code></td><td>yes</td><td>Native <code>signalsmith-stretch</code> key-lock backend through <code>kithara-stretch</code></td></tr>

<tr><td><code>client-reqwest</code></td><td>yes</td><td>Forward the default HTTP backend selection to network-reaching deps</td></tr>

<tr><td><code>tls-rustls</code></td><td>yes</td><td>Forward rustls TLS selection to network-reaching deps</td></tr>

<tr><td><code>apple</code></td><td>no</td><td>Apple AudioToolbox hardware decoder via <code>kithara-decode/apple</code></td></tr>

<tr><td><code>android</code></td><td>no</td><td>Android <code>MediaExtractor</code>/<code>MediaCodec</code> via <code>kithara-decode/android</code></td></tr>

<tr><td><code>fdk-aac</code></td><td>no</td><td>Enable libfdk-aac HE-AAC v1/v2 decode in the software path</td></tr>

<tr><td><code>beat-nn</code></td><td>no</td><td>Enable NN beat/downbeat analysis through <code>kithara-beat</code></td></tr>

<tr><td><code>stretch-bungee</code></td><td>no</td><td>Native Bungee key-lock backend through <code>kithara-stretch</code>'s private <code>bungee-sys</code> adapter</td></tr>

<tr><td><code>client-wreq</code></td><td>no</td><td>Forward the native <code>wreq</code> HTTP backend selection to network-reaching deps</td></tr>

<tr><td><code>tls-native</code></td><td>no</td><td>Forward native TLS selection to network-reaching deps</td></tr>

<tr><td><code>probe</code></td><td>no</td><td>USDT probes for tracing</td></tr>

<tr><td><code>mock</code></td><td>no</td><td>Generated mocks for tests</td></tr>

<tr><td><code>perf</code></td><td>no</td><td>Hotpath timing instrumentation</td></tr>

<tr><td><code>memprof</code></td><td>no</td><td>Allocation tracking for profiling examples</td></tr>

</table>

## Key Types

- `Audio<S>` — main PCM reader; the consumer reads frames from it and requests
  seeks.
- `AudioConfig<T>` — `bon` builder for stream config, decode backend,
  resampling, gapless mode, stretch controls, and engine load.
- `PcmSource` — worker-independent per-track decoded PCM source contract.
- `ResamplerQuality` / `ResamplerOptions` — sample-rate-conversion config
  threaded into the decoder-owned resampler plan.
- `StretchControls` / `TimeStretchProcessor` — preserve-pitch tempo mode on
  native targets; native builds require at least one `kithara-stretch` backend.
- `AnalyzerBuilder` / `AnalysisWorker` / `TrackAnalysis` — source-signal
  waveform and optional beat analysis.
- `Waveform` / `BeatGrid` — analysis artifacts; public blob I/O uses
  `Vec::<u8>::from(&artifact)` and `Artifact::try_from(&[u8])`.
- `EngineLoad` / `EngineLoadSnapshot` — live decode/effects cost meter.

## Usage

```rust
use kithara_audio::{
    AudioConfig, AudioDecoderConfig, DecoderResamplerSettings, ResamplerQuality,
};
use kithara_bufpool::Region;
use kithara_decode::GaplessMode;
use kithara_hls::{Hls, HlsConfig};
use kithara_play::{PlayWorker, PlayWorkerConfig};

let decoder_config = AudioDecoderConfig::builder()
    .gapless_mode(GaplessMode::CodecPriming)
    .resampler(
        DecoderResamplerSettings::builder()
            .quality(ResamplerQuality::High)
            .build(),
    )
    .build();
let audio_config = AudioConfig::<Hls>::for_stream(hls_config)
    .host_sample_rate(sample_rate)
    .decoder(decoder_config)
    .build();

let region = Region::default();
let worker = PlayWorker::new(
    PlayWorkerConfig::for_pools(region.byte_pool(), region.pcm_pool()).build(),
);
let mut audio = worker.open(audio_config).await?;
```

## Orientation

`kithara-audio` sits between `kithara-decode` and playback consumers. The
downloader lives in `kithara-stream`; audio consumes stream/storage contracts
without reconstructing protocol policy. Native time-stretch DSP backends live
in `kithara-stretch`; wasm retains the control surface without native DSP.
Analysis runs on decoded source PCM, not post-EQ, post-stretch, or post-resample
output.

See [CONTEXT.md](CONTEXT.md) for detailed threading, seek/recreate, analysis,
blob, and time-stretch contracts.
