<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-analysis.svg)](https://crates.io/crates/kithara-analysis)
[![docs.rs](https://docs.rs/kithara-analysis/badge.svg)](https://docs.rs/kithara-analysis)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-analysis

Progressive per-track waveform and beat analysis over decoded source audio.
It accepts both a dedicated `AudioReader` and already-decoded chunks through
`AudioObserver`, so playback warms the same analysis pass without decoding its
source twice. It owns analysis state, scheduling, DSP, and pure versioned bytes;
the source reader remains in `kithara-audio` and cache I/O/policy remains in the
consumer (currently `kithara-app`).

## Key Types

- `AnalyzerBuilder` / `AnalysisWorkerConfig` / `AnalysisWorker` — configure
  analysis and run progressive per-track passes on a domain dispatcher.
- `AnalysisProducer` — non-blocking decoded-chunk ingress for an open pass.
- `TrackAnalysis` — self-contained published snapshot: token, revision, source
  axis, coverage, fingerprint, waveform, and beat artifact.
- `Waveform` / `BeatArtifact` — analysis artifacts with versioned byte codecs.
- `BlobError` - format, corruption, and pooled restore errors from the artifact
  and composite byte codecs.

## Features

<table>

<tr><th>Feature</th><th>Default</th><th>Effect</th></tr>

<tr><td><code>default</code></td><td>yes</td><td><code>symphonia</code> + <code>client-reqwest</code> + <code>tls-rustls</code> for the supplied decoded-reader dependency closure</td></tr>

<tr><td><code>analysis-waveform</code></td><td>no</td><td>RealFFT waveform analyzer</td></tr>

<tr><td><code>analysis-beat</code></td><td>no</td><td>Beat-analysis pipeline with a caller-selected mono resampler backend</td></tr>

<tr><td><code>beat-nn</code></td><td>no</td><td>NN beat/downbeat detector backend through <code>kithara-beat</code></td></tr>

<tr><td><code>apple</code> / <code>android</code> / <code>webcodecs</code></td><td>no</td><td>Forward an alternate decoder backend to the supplied <code>kithara-audio</code> reader contract</td></tr>

<tr><td><code>client-wreq</code> / <code>tls-native</code></td><td>no</td><td>Forward alternate network and TLS selections through the reader and watchdog dependencies</td></tr>

</table>

## Integration

`kithara-analysis` consumes `kithara-audio`'s `AudioReader`, `AudioObserver`,
and decoded-signal values. It does not own decoder lifecycle, source readiness,
or playback scheduling. `AnalyzerBuilder<B, S>` receives the caller's typed
`PoolRegion<S>`; the schema must implement `HasPool<f32>`, so a missing sample
pool is rejected at compile time. Analysis scratch and
`kithara-resampler::MonoStream` buffers therefore compete under the same hard
region budget as the rest of the application instead of receiving a separate
pool. Persisting a `TrackAnalysis` through `AssetStore`, choosing cache keys,
and eviction policy remain application responsibilities. `write_to` appends to
caller-owned `Vec<u8>` output.

See [CONTEXT.md](CONTEXT.md) for the scheduling, ingest, waveform, and codec
contracts.
