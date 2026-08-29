# kithara-resampler - Context

Detailed contracts and invariants for the kithara-resampler crate; the README
is the overview.

## Ownership

This crate owns the platform-neutral standalone sample-rate resampling
contract: `Resampler` (processing), `ResamplerBackend` (factory trait),
`ResamplerCapabilities`, `ResamplerMode`, the construction configs, and the
error types. Platform backend contracts such as Apple `AudioConverter` live
here; shared Apple ABI and safe AudioToolbox wrappers come from `kithara-apple`.

Backend choice is encoded by the caller's `B: ResamplerBackend` type parameter.
Playback, analysis, and decoder configs thread that type through their own
builders; this crate exposes no cfg-selected default backend handle.
`NoResamplerBackend` is the explicit "no backend" type — it advertises empty
capabilities and fails at build, it is not a fallback.

Decoder placement is not owned here: `kithara-decode` decides whether a decoder
uses a codec-embedded converter or wraps decoded audio in a decoder adapter. Codec
lifecycle, media input, and gapless translation stay there. Playback graph
routing is not owned here either: `kithara-audio` passes device-rate config into
decode, while `kithara-analysis` owns analysis-side use; neither crate contains
backend-specific resampler policy.

## Allocation Contract

Hot paths must not allocate through ordinary `Vec` growth. Backends use one of
these storage owners:

- caller-owned input and output slices passed to `Resampler::process_into_buffer`;
- scratch buffers taken from the `SamplePool` inside `ResamplerSettings`;
- backend-owned pooled scratch acquired during construction or reset.

Library code must not call `SamplePool::default()` as a hidden fallback. The pool is
injected by the surface that owns memory sizing, matching the `kithara-bufpool`
contract. Construction may pre-warm scratch for the configured channels and chunk
size; steady-state processing reuses already-owned buffers.

Any backend adapter bridging a third-party API shape (Rubato's planar adapter
surface, the Apple converter's pull callback) must hide that bridge behind pooled
scratch and keep the public trait on borrowed planar slices.

## Backend Contract

`Resampler` handles standalone decoded audio only: it accepts borrowed planar `f32`
slices and writes into caller-owned planar output slices. The returned
`ResamplerProcess` reports accepted input frames and produced output frames.
Callers size output using the backend's frame-capacity methods
(`output_frames_max`, `output_frames_next`, `output_frames_for_input`).
`flush_into_buffer` processes the final caller-supplied block (default: plain
`process_into_buffer`) and `drain_into_buffer` drains backend-owned tail
(default: none), so a backend with no tail needs neither.

Variable ratio and glide are advertised through `ResamplerCapabilities`
(`FIXED_RATIO`, `VARIABLE_RATIO`, `RATIO_GLIDE`, `REALTIME_SAFE`,
`REPORTS_LATENCY`, `STANDALONE`). Callers holding a concrete resampler reach the
optional `ResamplerControl` surface through `Resampler::control_mut()`;
fixed-ratio backends return `None`. Fixed-ratio analysis config never asks for
DJ pitch/glide modes.

`validate_settings` is the single gate: it rejects a non-positive
`chunk_size`, a non-finite/negative `passthrough_tolerance`, a non-finite or
non-positive `max_ratio_adjustment`, a mode the backend's capabilities do not
carry, and a non-finite or non-positive ratio. `create_resampler` validates and
then builds exactly the configured backend, or returns
`ResamplerBuildError`. There is no runtime fallback chain and no portable default
backend in this crate.

`ResamplerQuality` adjusts implementation quality inside a selected backend; it
never selects a backend. Rubato-specific algorithm choices, including FFT, are
`rubato::RubatoConfig` fields, not separate backend families or cargo features.
External backends may ignore quality or map it into their own config.

## Configuration Shapes

Everything is constructed through bon builders. `ResamplerSettings` (channels,
`sample_pool`, mode, options, quality) is what a backend builds from;
`ResamplerConfig<B>` pairs it with a backend value and is what `create_resampler`
consumes. `ResamplerOptions` holds the numeric tunables — `max_ratio_adjustment`
8.0, `passthrough_tolerance` 0.0001, `chunk_size` 4096 — and its `Default` goes
through the builder, so overriding one keeps the other defaults.

`Resample<S>` carries `target_sample_rate`, quality and options with a placement
scope `S` (`Unit<B>` or `Decode<B>`), keeping the backend in the type at the call
site. `MonoStream<B>` / `MonoStreamConfig<B>` are the pooled single-channel
streaming adapter used by beat analysis in `kithara-analysis`.

## Built-In Backend Families

- `Rubato` (`resample-rubato`): fixed-ratio. `rubato::RubatoConfig` selects async
  poly/sinc or FFT via `RubatoAlgorithm`.
- `Glide` (`resample-glide`): cursor-based glide renderer advertising
  `FIXED_RATIO | VARIABLE_RATIO | RATIO_GLIDE | REALTIME_SAFE | STANDALONE`;
  `GlideBackend::with_config` selects interpolation and anti-aliasing through
  `GlideConfig`. The scalar path is the portable baseline; on Apple targets the
  `apple-accelerate` feature swaps in `kithara-apple::accelerate` for copy,
  interpolation, and biquad filtering.

## Platform Backend Families

`apple::AppleAudioConverterBackend` is a standalone CoreAudio `AudioConverter`
backend compiled on macOS/iOS without a cargo feature. It advertises
`FIXED_RATIO | REPORTS_LATENCY | STANDALONE`, has no configuration of its own,
and builds `apple::AppleResampler` through `kithara-apple` wrappers rather than
local AudioToolbox FFI. The crate root denies unsafe code, so unavoidable Apple
unsafe lives in `kithara-apple`.
Codec-embedded Apple decode remains in `kithara-decode` but uses the same shared
Apple ABI crate.

No Android backend exists until there is a real Android resampling backend. Future
backend families get cargo features only when their implementation module lands;
empty placeholder features are not part of the public contract.

## Decoder Integration

`kithara-decode` imports this crate and supports one of two placements:

- codec-embedded: the decoder emits target-rate audio internally (Apple decoder
  plus Apple `AudioConverter`);
- decoder-adapter: any decoder emits source-rate audio and is wrapped by a
  standalone backend from this crate — built-in or any external type implementing
  `ResamplerBackend`.

Explicit invalid pairs return typed configuration errors. The planner must not
try another backend when the requested pair is unsupported.

Gapless frame metadata belongs to the decoder-output domain. Scaling from source
rate to output rate happens once in the decode plan, independent of whether the
resampler is codec-embedded or adapter-based.

## Feature Flags

No feature is on by default.

- `resample-rubato` - Rubato fixed-ratio backend; algorithm selection lives in
  `RubatoConfig`.
- `resample-glide` - Glide backend with fixed-ratio, variable-ratio, and glide
  capability.
- `apple-accelerate` - Apple-target Glide acceleration through
  `kithara-apple::accelerate`; ignored on non-Apple targets. The Apple
  `AudioConverter` backend is *not* behind a feature; it is target-gated.
