<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-sync.svg)](https://crates.io/crates/kithara-sync)
[![docs.rs](https://docs.rs/kithara-sync/badge.svg)](https://docs.rs/kithara-sync)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-sync

`kithara-sync` owns the recursive synchronization group: its direct-child
membership policy, its ordered topology transactions, and the control-plane
protocol through which operations are admitted and acknowledged.

It owns `SyncGroup`, the live owner state behind it, the operation and
admission vocabulary, the immutable topology snapshot, and the monotonic
topology, operation, and load identities. Musical geometry — beat grids, beat
alignment, warp maps, and the presentation frontier — remains in
`kithara-warp`. Session timeline and clock ownership remain in `kithara-host`,
and execution, epochs, and real-time residency remain in `kithara-play`.

The crate has no renderer, executor, platform backend, or transport
responsibility, and never depends on a player, host, or queue.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-sync) for the ownership contract.
