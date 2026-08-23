# kithara-ffi — Context

Contracts and invariants for the kithara-ffi crate; the README is the overview.

## Target split

`src/lib.rs` is the single structural target boundary: `core` and `player` are shared, `native` is
gated on non-wasm and `web` on wasm. The `arch.no-target-os-outside-platform` ast-grep rule exempts
`src/lib.rs` for exactly that split; narrower platform gates (`mod android`, `mod android_test`) live
one level down in `src/native/mod.rs`. `android_test` additionally requires the crate-local `test`
feature and must never ship in a release AAR. `FfiAssetLayoutRegistry`, `FfiAssetStore`, and
`FfiPlayerConfig` are native-only, so the shared `AudioPlayer` facade has a native-only
`new(FfiPlayerConfig)` constructor; the wasm surface constructs the same facade with no config.

## Device feature sets

Both device flows drop default features, so `symphonia` is absent and the hardware backend is the
sole decoder on-device.

- `xtask apple` builds frameworks with `uniffi,apple,dev,stretch-signalsmith`. The crate-local
  `apple` feature forwards `kithara/apple-fused-src` (plus `apple-net` and the matching
  `kithara-play` / `kithara-queue` fused-SRC features), so Apple AudioToolbox decodes directly to the
  host rate through decoder-embedded resampler placement. That set intentionally enables neither
  `resample-rubato`, `analysis-beat`, nor `analysis-waveform`.
- `xtask android` builds release JNI libraries with `uniffi,android,stretch-signalsmith` (debug adds
  `dev,test`). The facade `android` feature keeps the fixed-ratio rubato stage (`resample-rubato`)
  and `analysis-beat` enabled; Android does not use the Apple fused-SRC path.

## Cache ownership and layouts

The native cache object graph is owned in Rust:

- `FfiAssetLayoutRegistry` is a shareable UniFFI object holding the Rust `AssetLayoutRegistry` behind
  a mutex. `register(target, layout)` replaces the layout for the file or HLS target; targets are
  independent and the latest registration for one target is the registry's current value. A replaced
  foreign layout is dropped after the lock is released, so a foreign `Drop` may re-enter `register`
  without deadlocking.
- `query_identity_layout(rules)` takes ordered `FfiCacheIdentityRule` records (domain patterns plus
  application-defined query parameter names) and returns a Rust-owned layout. Callers install it with
  the ordinary `register` method for file, HLS, or both; no foreign callback participates in its
  cache-key derivation.
- `FfiAssetStore::new(root, registry)` snapshots the registry and builds one `AssetStore`.
  `root = None` preserves the platform `StorageBackend` default; a supplied root selects that outer
  disk directory without changing paths inside an asset root.
- `FfiAssetStore` also owns the `Region` whose pools are shared by cache, network, decode, and
  playback, plus a store-specific `CancelScope`. Dropping the last foreign `Arc<FfiAssetStore>`
  cancels that store subtree. Player cancellation is a separate `CancelToken` root and does not
  redefine the shared store lifetime.
- `FfiPlayerConfig.store: Arc<FfiAssetStore>` is the only asset/cache field on the player
  configuration. `NativeInner` retains that object, takes its `Region`, and clones its inner
  `AssetStore` handle into the queue and every resource, so one FFI store can back multiple players.

Registry mutation and store configuration are separate lifetimes. Registering or replacing a layout
after a store exists does not alter that store; only a later `FfiAssetStore::new` observes the new
snapshot. A store retains the foreign callbacks captured by its snapshot even if the registry or the
caller's original callback reference is released. There is no `FfiCacheConfig`,
`FfiCacheLayoutRegistration`, registration-list translation, SDK-owned registry, or per-player store
builder in the native contract; generated bindings expose the Rust registry and store objects, and
platform adapters only retain and forward them.

## Foreign layout callbacks

Foreign `root` and `path` callbacks receive complete owned FFI values. `root` is invoked once per
asset-scope construction and `path` once per resource-key construction; repeating scope or key
construction invokes the corresponding callback again. After a key is minted, cloning the scope and
all acquire, open, read, write, seek, state, availability, demand, and eviction operations stay in
Rust and never cross the FFI callback boundary.
Callbacks must be deterministic, fast, non-blocking, non-throwing, and safe on background threads.
Invalid output fails scope or key creation; it is neither sanitized nor replaced with the default
layout. The exact component rules live on the `FfiAssetLayout` trait doc in `src/core/layout.rs`. A
URL resource carries the full URL, so a custom delegate must preserve any required query identity
without writing query text, credentials, or other secrets into a path. The default layout uses a
bounded query fingerprint and ignores fragments.

An omitted target registration uses `DefaultLayout` — the normal default, not a compatibility
fallback. Its disk mapping: direct-file bytes at `track/track.<ext>`, HLS URL resources mirrored
below `track/` by authority and path with a query fingerprint when needed, and the named track
analysis artifact at `analysis/track.analysis`. Changing the outer store root does not change those
relative paths.

## Web target

The browser surface is the cross-platform `AudioPlayer` facade (`src/player/facade.rs`) with a
`#[wasm_bindgen] impl` in `src/web/surface.rs`: JS constructs one `new AudioPlayer()`, drives the
queue through `append` / `insert` / `selectItem`, transport through `play` / `pause` / `seek`, and
receives structured events through `setObserver` / `setItemObserver`. Generated TypeScript
definitions ship with the wasm-bindgen output.

The Web Worker (`src/web/worker.rs`) owns the `Queue` and builds its own in-memory `AssetStore`
(`StorageBackend::Memory`) and `Region` in Rust-owned build state, using the default layout registry
— native foreign layout callbacks are not bridged into JavaScript. The main-thread `WasmInner` owns
the `WorkerCmd` channel plus a local cache so the infallible facade getters can answer without a
round trip. Wasm builds use the web-audio backend and link no stretch backend; playback rate is
retained as main-thread control state only — no `WorkerCmd` carries it — so PCM speed stays 1.0
until a wasm-capable stretch backend exists.

Wasm does link a resampler: the wasm32 dependency arm re-adds `resample-rubato` on top of
`default-features = false`. A web-audio context runs at the one rate the browser gives it, and any
track whose own rate differs needs a fixed-ratio stage to reach it. Without the feature
`PlaybackResamplerBackend` resolves to `NoResamplerBackend` and such a track fails to open at all.
The Apple device set above can drop rubato because AudioToolbox decodes straight to the host rate;
the browser offers no equivalent.
