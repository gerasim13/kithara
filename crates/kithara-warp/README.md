# kithara-warp

Beat-map synchronization contracts and the source-generic resident Warp decorator for Kithara.

The crate owns musical coordinates, immutable beat-map snapshots, group
topology, alignment plans, synchronization operations, `Warp<S>`, `WarpConfig`,
live temporal controls, and the synchronous `WarpRenderer` that drives a
`kithara-stretch::ElasticEngine`. It does not decode audio, own source
lifecycle, own `Player` / `PlayWorker` / Host/session state, access storage, or
analyze PCM.

See [CONTEXT.md](CONTEXT.md) for ownership and dependency boundaries.
