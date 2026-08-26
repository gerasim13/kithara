# kithara-stretch - Context

Contracts and invariants for the kithara-stretch crate; the README is the overview.

## Ownership

This crate owns pure time-stretch DSP only. Audio-graph glue (`StretchControls`,
`TimeStretchProcessor`, `PcmChunk`, `PcmMeta`, resampler-rate routing) stays in `kithara-audio`,
which passes its existing `PcmPool` through `ElasticConfig`; `kithara-stretch` must not create a
default or global pool.

- `ElasticEngine` is the sole backend contract. Exact source/output frame counts control time;
  `set_pitch` remains independent, and `flush` / `reset` define stream lifecycle.
- `StretchKind` is the compiled backend selector. Persisted discriminants are stable regardless of
  which variants are compiled in: `1 = Signalsmith`, `2 = Bungee`; `3` is reserved for a future
  pure-Rust native backend. An unknown discriminant decodes to `StretchKind::all()[0]`, the first
  compiled-in backend, which is also `Default`.
- `ElasticConfig` is the single fallible `#[non_exhaustive]` `bon` root config. It owns the
  `StretchKind` selection, sample rate, channel count, maximum source/output frame spans and the
  injected `PcmPool`; the selector is not a second factory argument.
- `build_engine(config)` dispatches the config-owned selector to `Box<dyn ElasticEngine>`.
- `ElasticPriming` is an optional capability for engines that can absorb history without emitting
  it. Nothing above an adapter names a concrete DSP library.

## Exact-span contract

`ElasticEngine` renders exactly `request.output_frames()` from exactly `request.source_frames()`;
the frame counts are the only rate control, so the caller owns the transport and two engines fed the
same plan advance through the source identically. `prepare` allocates outside the render core;
`capabilities()` is fixed for the engine's lifetime; `reset()` clears history and may fail for an
engine that clears state by rebuilding itself.

Each engine reports an `ElasticRateEnvelope` spanning every non-empty request representable by its
prepared source/output frame limits, plus its own `ElasticLatency`. Request shape, buffer lengths,
prepared limits and the rate window are checked once by `ElasticCapabilities`, so every engine
accepts and rejects the same requests.

`ElasticPriming` is separate because it is not universal. Priming resets the engine, absorbs the
declared source history **without emitting it**, and discards exactly the declared output latency,
so the next `process` starts at the source frame after the warmup span with no leading gap. An
engine whose pipeline can only emit what it has already consumed cannot do this and does not
implement the trait.

## Engine contract

Engines process interleaved `f32` PCM. `ElasticRequest` names exact non-empty source and output
frame spans; their ratio is the only tempo control. `set_pitch(scale)` is independent (`1.0` keeps
pitch locked), which preserves keylock without a second streaming API. Invalid preparation,
requests, pitch or processing return `ElasticError`; the outer `AudioEffect::process` maps failure
to "drop this chunk + warn", never a panic.

The produce path must stay allocation-free. Callers provide fixed output slices from scratch
reserved before the checked render call, and an engine that needs planar scratch checks it out from
the `PcmPool` supplied in `ElasticConfig`; no engine owns a default or global pool.

`flush(out)` writes the next buffered-tail portion into caller-owned storage sized from
`terminal_chunk_frames` and returns the number of interleaved frames written. EOF repeats the call
until zero; a completed drain stays empty until new input. This streaming contract lets a
rate-dependent tail span several fixed-size chunks without loss. A rate change between adjacent
exact requests preserves history and is not a flush or reset boundary. Every backend must expose the
same real terminal-drain behavior; a no-op or synthetic-silence flush is not conforming.
`reset()` clears buffered state after seek; source-spec and backend changes are handled by the
caller preparing a replacement outside its checked render core. The trait intentionally does not
depend on `kithara-decode::PcmSpec`.

## Backend limitations

- Bungee uses the low-level `bungee-sys` granular `Stretcher` API behind a private RAII owner. Native
  planar output is validated and copied immediately into pooled Rust storage; no native pointer or
  mutable slice escapes the call that produced it.
- Bungee retains the source lookahead and output remainder required by its overlapping grains. EOF
  first advances finite requests to the exact source end, clips output by native request timestamps,
  then clears the four-grain pipeline with invalid requests. `flush` therefore returns real terminal
  audio across one or more chunks instead of dropping roughly one latency of the track.
- Bungee preparation fails when the injected pool budget cannot cover its planar scratch or
  native stretcher construction fails; the audio adapter warns once and marks the engine unavailable.
- `BungeeElastic` does not implement `ElasticPriming`, for the same root cause as the missing tail
  drain in the former high-level adapter. The new low-level state machine now provides the necessary
  source coordinates and preroll primitive, but priming remains outside the common contract until
  the shared facade conformance matrix covers it for both engines.
- `BungeeElastic::reset` clears and drains the resident native pipeline without rebuilding it; its
  Rust-side input/output storage remains the buffers reserved from the injected pool at prepare.
- Bungee reports its unity latency only after its pipeline is warm, and runtime latency moves with
  the rate. Preparation measures the unity reference on the resident, shape-sized core, resets that
  same core in place, and separately declares a safe fixed terminal chunk from the native maximum
  input-grain span; no probe engine or extra pool allocation is retained.
- Both engines accept the same pitch range, `0.25..=4.0`; this is the range covered by Bungee's
  native sizing and prevents a backend selector from changing validation semantics.
- Both engines expose the complete non-empty rate domain implied by their prepared source/output
  frame limits; the conformance suite exercises its minimum, unity and maximum requests.
- Bungee on iOS is opt-in. Its CMake C++ build must see `IPHONEOS_DEPLOYMENT_TARGET`; `xtask apple`
  exports the value from `[workspace.metadata.apple] deployment-target` before invoking
  `cargo swift package`. Preserve the same env for manual `-F stretch-bungee` Apple builds.

## Adding a backend

1. Add `src/backends/<name>.rs` with a concrete adapter implementing `ElasticEngine`, re-exported
  from `backends/mod.rs` under the same gate.
1. Add a feature `stretch-<name>` in `Cargo.toml` and to the `any(...)` guard of the
  `compile_error!` in `lib.rs` (the crate requires ≥1 backend).
1. Gate the adapter module, the `StretchKind` variant, its `all()` entry, its `From`/`u8` arms, and
  the `build_engine` factory arm on `#[cfg(feature = "stretch-<name>")]`; keep the discriminant
  stable.
1. Declare latency, implement `ElasticPriming` only if the engine can absorb history without
  emitting it, and add the backend as a named case in the shared facade conformance matrix (plus the
  priming matrix when it primes) in `tests/elastic.rs`. The suite is the contract; backend-specific
  tests cover only preparation mechanics and measured latency.
1. Document any target, tail-drain or priming limitation above.

Do not declare `stretch-native` or add `backends/native.rs` until the pure-Rust engine exists.

## No-backend and wasm builds

There is no "no backend" build here: `lib.rs` `compile_error!`s unless at least one `stretch-*`
feature is set, and the machinery (kind, factory, config, backends) is unconditional. "Stretch is
absent" lives one level up — `kithara-audio` depends on `kithara-stretch` **optionally** (only its
`stretch-signalsmith` / `stretch-bungee` features pull it), so a build with no stretch, including
every wasm build today, simply does not link this crate. Domain types that non-stretch code needs
(`GridSegment`, `RegionPlan`) therefore live in `kithara-audio`. The C++ backends are native-only
(`wasm32-unknown-unknown` has no libc++), and `kithara-bufpool` is likewise an optional non-wasm
dependency pulled in by the backend features.
