# kithara-warp - Context

## Ownership

This crate owns the pure protocol used to align one beat map with another and
to compose maps through nested synchronization groups. It owns immutable
snapshots, coordinates, topology operations, alignment plans, cursors, and
typed results.

Host-axis values describe an ephemeral musical clock; they do not make this
crate the owner of the live Host, playback session, audio graph, or worker.

## Boundaries

- `kithara-beat` owns beat-analysis algorithms and analyzed beat output data.
- `kithara-audio` owns decoded PCM sources, playback workers, and the resident render chain.
- `kithara-stretch` owns temporal DSP execution.
- `kithara-play` currently owns Players, session state, and the audio graph.
- `kithara-assets` is the only production persistence path.

The crate must not depend on audio, play, host, assets, or analyzer runtime
types. Runtime owners consume immutable plans produced through these contracts.

## Configuration

The current crate contains only pure contracts, so it must not expose an empty
or unused runtime config. The first concrete Warp runtime consumer introduces a
`WarpConfig` facade in the same change. That config is built with `bon`, uses
`fieldwork` for read access, and contains only values consumed by that runtime.
Decoded-source ownership, PCM format, shared pools, cancellation, and worker
resources remain in their canonical configs and are not duplicated here.
