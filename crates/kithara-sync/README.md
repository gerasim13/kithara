<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-sync.svg)](https://crates.io/crates/kithara-sync)
[![docs.rs](https://docs.rs/kithara-sync/badge.svg)](https://docs.rs/kithara-sync)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-sync

`kithara-sync` owns synchronization of decks to a session: the recursive
synchronization group and its owner state, the session root that is the one
owner of every member's gate, the executor that stages and installs a
member's preparation, and the audio-thread half that claims, activates and
reports it. The protocol is generic: a player and a host take part only
through the narrow ports this crate defines. Musical geometry — beat grids,
alignment, warp maps and the presentation frontier — stays in
`kithara-warp`; the session clock stays in `kithara-host`; staging I/O and
audio rendering stay in `kithara-play`. The crate never depends on a
player, host, or queue.

## Key Types

<table>

<tr><th>Type</th><th>Kind</th><th>Role</th></tr>

<tr><td><code>SyncGroup</code> / <code>GroupState</code></td><td>trait / struct</td><td>One synchronization group and the live owner state behind it: membership, topology, verified transactions</td></tr>

<tr><td><code>SyncOperation</code> / <code>SyncAdmission</code></td><td>enums</td><td>An operation routed through the group owner and the owner's answer to it</td></tr>

<tr><td><code>SyncRoot</code> / <code>RootCut</code></td><td>structs</td><td>The session's root group, its gate and member cells; every change runs inside one owner cut</td></tr>

<tr><td><code>RootPort</code> / <code>EntryPort</code></td><td>traits</td><td>What the host supplies to the root: slot inboxes, graph liveness, snapshot publication, the audio clock</td></tr>

<tr><td><code>SyncExecutor</code> / <code>StagePort</code></td><td>struct / trait</td><td>Stages a member's preparation through the player's staging port and hands the installed lane over</td></tr>

<tr><td><code>ReceiptSink</code></td><td>trait</td><td>The owner an executor reports each staged lane to</td></tr>

<tr><td><code>SyncTicket</code> / <code>SyncCallback</code></td><td>structs</td><td>An installed activation and the audio-thread owner that claims, activates and returns it</td></tr>

<tr><td><code>SyncReceiptTx</code> / <code>SyncReceiptInbox</code></td><td>structs</td><td>The per-slot mailbox from the audio thread back to the root</td></tr>

<tr><td><code>SyncGateBinding</code> / <code>SourceReservation</code></td><td>structs</td><td>A member's gate as a player holds it, and the player's one writer of its source changes</td></tr>

</table>

## Features

- `mock` — `mock::MemberOwner`, the owner of one member behind its own gate, and `ReceiptSinkMock`, for tests of the member side of the gate.
- `usdt`, `perf` — forwarded to `kithara-warp`.

## Integration

`kithara-host` holds the `SyncRoot` and implements its ports; `kithara-play`
implements `StagePort` and runs `SyncCallback` in its audio callback.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-sync) for the ownership contract.
