<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-beat.svg)](https://crates.io/crates/kithara-beat)
[![docs.rs](https://docs.rs/kithara-beat/badge.svg)](https://docs.rs/kithara-beat)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-beat

Beat / downbeat detection: CPU NN inference with optional native CUDA prediction. A port of the `beat_this`
pipeline (CPJKU, ISMIR 2024) via `danigb/beat-this-rs` @ `089b509`. Code and
model weights of both upstreams are MIT-licensed; this crate keeps that
attribution.

## Usage

```rust
use kithara_beat::{BeatThis, RawBeats};

let bt = BeatThis::builder()
    .mel_model(mel_bytes)
    .beat_model(beat_bytes)
    .pools(pools.clone())
    .build()?;
let raw: RawBeats = bt.analyze(&mono_22050)?;
```

## Key Types

- `BeatThis::builder()` — load models from bytes (caller chooses embed vs file
  vs download), inject a sample-capable pool facade, and pick the decoding policy.
- `BeatThis::analyze(&mono_22050)` — run the mel, inference and peak-pick pipeline.
- `BeatBackend` — select the predictor runtime in `BeatConfig::builder().backend(...)`.
  `Cpu` is the default. With `cuda`, use `BeatBackend::Cuda { device_id: 0 }`.
  Load one caller-owned `ort::environment::Environment` from the configured
  library path and supply `.environment(&environment)` to the detector builder.
  Mel extraction remains RTen CPU because its FFT model is unchanged; peak
  picking and output buffers also remain on CPU. CUDA provider registration
  errors are returned without switching the predictor to CPU. ONNX Runtime
  may place shape/control operators on CPU; model kernel placement and complete
  analysis latency must be measured before claiming acceleration.
- `BeatConfig` — predictor backend, CUDA runtime library path, peak threshold,
  max-pool half-width, and dedup width. The detector retains this configuration;
  `ConfigOwner::config` exposes the same immutable settings used by prediction
  and peak picking. The numerical defaults
  are the values the golden fixtures are held to; see [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-beat) before moving
  them.
- `RawBeats { beats, downbeats }` — pooled output positions in seconds, sorted
  and deduplicated.
- `BeatDetector` — the detector contract: one window of mono audio in, marks
  out. Both backends here implement it, and a caller can supply its own; the
  analysis pass drives a `dyn BeatDetector` and never names a backend. Errors
  arrive as `BeatDetectError`; the `mock` feature exposes `BeatDetectorMock`
  for a consumer's tests.
- `RawBeatGrid` / `BeatGridModel` — the served beat-grid contract: a document as
  it arrives, and the same document once its times, ordinals, bar anchors and
  meter have been checked. `BeatGridModel` is reachable only through
  `TryFrom<RawBeatGrid>`, which `serde` also routes deserialization through, so
  no unchecked grid exists. Model-only: no detector, no weights, no `nn`/`dsp`,
  so a server reading a stored grid carries nothing of the analyzer. A local
  pass reaches the same type through `kithara-analysis`, which states one grid
  per publication from its own beat artifact.

## Features

- `cuda` — optional ONNX Runtime GPU predictor using the same ONNX model bytes.
  Supply compatible native ONNX Runtime, CUDA and cuDNN libraries. Native library
  resolution preserves the standard `ORT_DYLIB_PATH` behavior unless
  `BeatConfig::builder().ort_library_path(...)` names an explicit override.
  The configuration resolves the path at construction, using the environment
  source from `kithara-config`, and the
  detector retains that resolved configuration. ONNX Runtime loads one library
  per process; configure it before any other runtime use and use the same library
  for every detector. The crate re-exports `ort`, so the caller creates its native
  resource with `ort::init_from(config.ort_library_path())?.build()?`, retains it
  across detector replacement, and passes `.environment(&environment)` to each
  detector builder. Each session retains an SDK handle to that same environment.
  Native resource destruction happens when all caller and session handles drop.
  No runtime binary is downloaded by this feature. Each
  detector owns one session and serializes its CUDA model calls. CPU-only
  builds retain the existing RTen path and require no ONNX Runtime library.
  CUDA construction without the native resource returns
  `BeatError::CudaEnvironmentMissing`. Native loading returns `ort::LoadDynamicError`,
  convertible to `BeatError::CudaLibrary`; session and provider failures return
  `BeatError::Cuda`.
- `embed-small-model`, `embed-full-model`, `embed-full-int8-model` — exactly one
  of these exposes `MEL_MODEL_BYTES` / `BEAT_MODEL_BYTES` / `BEAT_MODEL_TAG`, so
  FFI/mobile builds need no asset plumbing. Off by default. The build script
  fetches the selected model and the mel model into `KITHARA_BEAT_MODEL_CACHE`
  (default `$CARGO_HOME/kithara-beat-models`, with Cargo home defaulting to
  `~/.cargo`) and checks fetched and cached bytes against a pinned SHA-256.
  The compiler embeds a stable snapshot in Cargo's `OUT_DIR`; deleting pinned
  source-cache files does not invalidate an unchanged build. The first build
  with a cold cache needs network. The int8 model is quantized locally and has
  to be placed in the cache by hand; it has no pinned digest, and changes to
  this user-supplied input rerun the build script.

| feature | size | mean octave-folded error over 40 tracks |
|---|---|---|
| `embed-small-model` | 10.1 MB | 1.72 BPM |
| `embed-full-model` | 79 MB | 0.38 BPM |
| `embed-full-int8-model` | 22.6 MB | 0.35 BPM |

## Integration

A leaf analysis crate: it takes whole-track mono f32 PCM at 22 050 Hz and
returns raw beat / downbeat positions in seconds. It owns no decoder, resampler,
or I/O — the consumer (`kithara-analysis`) handles decode, downmix, resample, and
grid cleanup. The `dsp` backend takes its spectrum and autocorrelation from
`kithara-dsp`: Accelerate on Apple, `fearless_simd` elsewhere.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-beat) for detailed contracts, invariants, and internals.
