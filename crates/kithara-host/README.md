# kithara-host

`kithara-host` owns Kithara's multi-player session, shared Firewheel output
graph, session transport, and platform audio backend. A player/deck remains in
`kithara-play`; beat-grid and synchronization contracts remain in
`kithara-warp`.

Callers fully construct a player or decorator and transfer that instance with
`Host::insert`. The Host attaches its opaque session capability exactly once
and retains the instance; callers receive a typed `HostOwned` control endpoint,
not the player value or its session dispatcher.

The current crate is a mechanical ownership extraction. Runtime invariants and
dependency boundaries are documented in [`CONTEXT.md`](CONTEXT.md).
