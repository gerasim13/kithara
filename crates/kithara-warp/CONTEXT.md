# kithara-warp - Context

## Ownership

This crate owns the pure protocol used to align one beat map with another and
to compose maps through nested synchronization groups. It owns immutable
snapshots, coordinates, `WarpMap`, `SyncGroup`, topology operations, alignment
plans, cursors, and typed results. It also owns the resident identity `Warp<S>`
decorator, `WarpConfig`, and synchronous `WarpRenderer<S>`, which applies temporal
plans through the backend-neutral `kithara-stretch::ElasticEngine` contract on
native targets. Without an elastic backend, including on wasm, the same
renderer contract stays resident as an exact identity stage.

Host-axis values describe an ephemeral musical clock; they do not make this
crate the owner of the live Host, playback session, audio graph, or worker.
`SyncMember::Grid` accepts only `Send + Sync` leaf grids because an owning
topology operation may cross the wasm Worker-to-Host route. Nested group owners
remain `MaybeSend + MaybeSync`; the platform owner must split worker-bound
runtime state before transferring such a group.

## Boundaries

- `kithara-beat` owns neural beat detection and its raw model output.
- `kithara-analysis` owns progressive analysis and the cleaned, identity-free
  `BeatArtifact` consumed by a future calibrated grid adapter.
- `kithara-audio` owns decoded-audio source lifecycle, decoder-side sample-rate
  conversion, readiness, and the prepared producer seam.
- `kithara-stretch` owns backend DSP engines and their exact-span contract.
- `kithara-play` owns `PlayWorker`, `DecoderNode`, final output admission,
  post-Warp playback effects, engine-load measurement, Players, session state,
  and the audio graph.
- `kithara-assets` is the only production persistence path.

The crate must not depend on audio, play, host, assets, or analyzer runtime
types. `Warp<S>` is generic over its source, and `WarpRenderer<S>` is a synchronous
stage; neither makes this crate the owner of source lifecycle, playback
scheduling, or worker threads.

R7 keeps `Warp<S>` in identity mode. The production path shares live stretch
controls through it, but does not yet evaluate `WarpMap`, advance a runtime map
cursor, apply non-identity alignment, or turn render/presentation progress into
a `SyncGroup::acknowledge` call. The map and acknowledgement APIs remain pure
contracts for the later actuator integration.

## Configuration

`WarpConfig` is built with `bon`, uses `fieldwork` for read access, and carries
the shared `StretchControls` owned by the resident identity `Warp<S>`. On
native targets it also carries backend preparation, source-block and optional
render-quantum settings expressed in frames. Standalone Warp leaves the quantum
unspecified; Player supplies 32 frames when omitted, so playback uses bounded
prefill/ring admission before observing the next control publication.
Rate smoothing uses Firewheel
`SmootherConfig` and runs once per output block in the playback owner. The identity
renderer deliberately ignores temporal intent while preserving the same stage
contract. Every renderer receives the caller's configured `PoolRegion<S>`; it
never creates a pool region. Source ownership, cancellation, worker resources,
and response budgets remain in their canonical configs and are not duplicated
here.

`WarpConfigPatch` is what a configuration document may say about it. The live
`StretchControls` handle is not a document key: it is shared with the deck and
the UI, so a document naming a ratio would lose to the first gesture. Backend
preparation geometry is one, carried as `backends` and re-read on every engine
rebuild, so the geometry a document names survives a backend switch. Which
engine runs stays a live control, not a document key.

Fixed-ratio sample-rate conversion remains owned by `kithara-decode`; it is not
a substitute Warp backend because resampling changes pitch. Targets without an
elastic backend report playback-rate capability as unavailable and preserve
decoded samples through the identity renderer.


## Published render rate

Asset region plans contain beats per source second, independent of deck tempo.
In Off mode, Warp uses the published smoothed manual multiplier. In HostSync
and LocalSync it derives source seconds per output second from the published
deck beat span divided by asset tempo, ignoring the manual multiplier. Missing
asset geometry or a stationary beat span preserves original tempo; it does not
establish phase lock. The playback owner projects local deck anchors onto the
Host output frame span before publication. HostSync uses the current Host span.

The output owner publishes the applied multiplier and request revision together
in `RenderContext`. Warp does not smooth that value or resample a live speed
control during rendering. Preparing a source quantum pins its context until
that quantum renders, even if a newer publication arrives in between.

Before the first presentation frontier exists, a worker must prefill the track.
The renderer captures the initial control target at construction for this
bootstrap phase. Subsequent rate changes arrive through the publisher; clearing
a context does not reactivate control polling. A standalone renderer caller
that changes rate must likewise publish its output context. Backend and keylock
selection remain separate live controls. WASM identity behavior is unchanged.
