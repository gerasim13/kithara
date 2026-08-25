# kithara-warp

Pure beat-map and synchronization contracts for Kithara.

The crate owns musical coordinates, immutable beat-map snapshots, group
topology, alignment plans, and synchronization operations. It does not decode
audio, own playback or host state, access storage, or analyze PCM.

See [CONTEXT.md](CONTEXT.md) for ownership and dependency boundaries.
