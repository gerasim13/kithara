# kithara-stretch - Context

Contracts and invariants for the kithara-stretch crate; the README is the overview.

## Ownership

This crate owns pure time-stretch DSP only. Audio-graph glue (`StretchControls`,
`TimeStretchProcessor`, `PcmChunk`, `PcmMeta`, resampler-rate routing) stays in `kithara-audio`,
which passes its existing `PcmPool` through `StretchOptions`; `kithara-stretch` must not create a
default or global pool.

- `StretchBackend` / `StretchBackendError` define the backend contract.
- `StretchKind` is the compiled backend selector. Persisted discriminants are stable regardless of
  which variants are compiled in: `1 = Signalsmith`, `2 = Bungee`; `3` is reserved for a future
  pure-Rust native backend. An unknown discriminant decodes to `StretchKind::all()[0]`, the first
  compiled-in backend, which is also `Default`.
- `StretchOptions` (a `#[non_exhaustive]` `bon` builder) owns backend construction settings: source
  sample rate, channel count, `max_input_frames` (default 8192), and the injected `PcmPool`.
- `build_backend(kind, &options)` dispatches selector → concrete backend.

## Backend contract

Backends process interleaved `f32` PCM. `set_ratio` and `set_pitch` are independent controls — that
decoupling is what makes keylock real. `set_ratio(stretch)` is the time factor
`output_frames / input_frames` (above `1.0` lengthens the output); `set_pitch(scale)` is the pitch
factor (`1.0` keeps pitch locked). Both reject non-finite or non-positive values with
`StretchBackendError::Param`. `Process` errors exist so the outer `AudioEffect::process` (fixed at
`-> Option<PcmChunk>`) maps a backend failure to "drop this chunk + warn", never a panic.

The produce path must stay allocation-free in steady state: callers ask
`max_output_samples(input_frames)` before `process` or `flush`, reserve that much scratch capacity,
then reuse the same output buffer across chunks. Backends that need planar scratch use the `PcmPool`
supplied in `StretchOptions`; no backend owns a global pool.

`flush(out)` drains the buffered tail at end of stream or at a real region ratio boundary. It is a
one-shot tail drain: repeated flushes without new input or `reset` append nothing, so an EOF drain
can loop until it yields an empty append. A backend that cannot expose a true tail drain must
document that in its adapter. `reset()` clears buffered state after seek, source-spec change, or
backend swap; a spec change is handled by the caller rebuilding the backend with the new scalar
sample rate and channel count, so the trait intentionally does not depend on
`kithara-decode::PcmSpec`.

## Backend limitations

- Bungee has no tail drain (its high-level `Stream` exposes none, and feeding muted input would emit
  stretched silence instead of the buffered tail, inflating duration): `flush` is a no-op and roughly
  one latency of audio is dropped at end of stream. A real drain needs the low-level granular
  `Stretcher` API.
- Bungee constructs disabled (warning once) when the pool budget cannot cover its planar scratch or
  `Stream::new` fails; `process` then emits nothing rather than erroring per chunk.
- Bungee on iOS is opt-in. Its CMake C++ build must see `IPHONEOS_DEPLOYMENT_TARGET`; `xtask apple`
  exports the value from `[workspace.metadata.apple] deployment-target` before invoking
  `cargo swift package`. Preserve the same env for manual `-F stretch-bungee` Apple builds.

## Adding a backend

1. Add `src/backends/<name>.rs` with a concrete adapter implementing `StretchBackend`, re-exported
  from `backends/mod.rs` under the same gate.
1. Add a feature `stretch-<name>` in `Cargo.toml` and to the `any(...)` guard of the
  `compile_error!` in `lib.rs` (the crate requires ≥1 backend).
1. Gate the adapter module, the `StretchKind` variant, its `all()` entry, its `From`/`u8` arms, and
  the `build_backend` factory arm on `#[cfg(feature = "stretch-<name>")]`; keep the discriminant
  stable.
1. Document any target or tail-drain limitation above.

Do not declare `stretch-native` or add `backends/native.rs` until the pure-Rust engine exists.

## No-backend and wasm builds

There is no "no backend" build here: `lib.rs` `compile_error!`s unless at least one `stretch-*`
feature is set, and the machinery (kind, factory, options, backends) is unconditional. "Stretch is
absent" lives one level up — `kithara-audio` depends on `kithara-stretch` **optionally** (only its
`stretch-signalsmith` / `stretch-bungee` features pull it), so a build with no stretch, including
every wasm build today, simply does not link this crate. Domain types that non-stretch code needs
(`GridSegment`, `RegionPlan`) therefore live in `kithara-audio`. The C++ backends are native-only
(`wasm32-unknown-unknown` has no libc++), and `kithara-bufpool` is likewise an optional non-wasm
dependency pulled in by the backend features.
