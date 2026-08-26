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

`flush(out)` writes the buffered tail into caller-owned storage and returns the number of interleaved
frames written. It is a one-shot tail drain: repeated flushes without new input or `reset` return
zero, so an EOF drain can loop until the engine reports no frames. A rate change between adjacent
exact requests preserves history and is not a flush or reset boundary. A backend that cannot expose
a true tail drain must document that in its adapter.
`reset()` clears buffered state after seek; source-spec and backend changes are handled by the
caller preparing a replacement outside its checked render core. The trait intentionally does not
depend on `kithara-decode::PcmSpec`.

## Backend limitations

- Bungee has no tail drain (its high-level `Stream` exposes none, and feeding muted input would emit
  stretched silence instead of the buffered tail, inflating duration): `flush` is a no-op and roughly
  one latency of audio is dropped at end of stream. A real drain needs the low-level granular
  `Stretcher` API.
- Bungee preparation fails when the injected pool budget cannot cover its planar scratch or
  `Stream::new` fails; the audio adapter warns once and marks the engine unavailable.
- `BungeeElastic` does not implement `ElasticPriming`, for the same root cause as the missing tail
  drain. Its `Stream` emits with a fixed lag: the input-frame coordinate of the next output frame is
  always `emitted_output_frames - latency`, so absorbing history costs exactly as much emitted
  output as it consumes input, and no history/warmup pair leaves the engine aligned. A primed
  Bungee engine needs the low-level granular `Stretcher` API, whose `Request::position` is a source
  coordinate.
- `BungeeElastic::reset` rebuilds the stream (the high-level `Stream` has no reset), so it
  allocates and can fail; `SignalsmithElastic::reset` is allocation-free. Callers perform reset
  from their off-core lifecycle seam.
- Bungee reports its latency only once a grain has been analysed, and the value keeps growing until
  the pipeline is full; it also moves with the rate. `BungeeElastic` therefore saturates the number
  on a throwaway stream at prepare and reports that unity steady-state value for its lifetime.
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
  emitting it, and add one `elastic_engine_conformance!` line (plus
  `elastic_priming_conformance!` when it primes) in `tests/elastic.rs`. The suite is the contract;
  backend-specific tests cover only real capability differences such as latency and terminal drain.
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
