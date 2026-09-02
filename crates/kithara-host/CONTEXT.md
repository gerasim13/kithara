# kithara-host context

## Ownership

The Host owns the session root sync group and its member values, the shared
Firewheel graph, session transport, mix tap, output limiter, and native or web
audio backend. The graph registry is only a projection of Host-owned members;
it must not become a second mutable synchronization topology. On wasm the
main-thread member owns the sendable synchronization state and desired level;
the remote Worker Host retains the Queue runtime and its worker-bound JS
resources until explicit removal.

`kithara-play` owns one player/deck, its render node, effects, fades, worker,
and playback flow. `kithara-warp` owns synchronization and warp contracts. The
dependency direction is `kithara-host -> kithara-play` and
`kithara-host -> kithara-warp`; neither lower crate may depend on the Host.

## Offline rendering

`OfflineHost` owns the same root group, Firewheel graph, limiter, transport,
and member insertion path as `Host`; only its audio backend is different. The
backend starts lazily on the first render when no player has started it yet, so
an empty master mix is valid silence rather than an unavailable graph.

Its renderer consumes one absolute finite frame range at a time. It may skip
forward by rendering undisclosed frames, never rewinds an already consumed
timeline, and writes only the requested range to `RenderSink` in configured
bounded blocks. `OfflineHost::new` receives the composition root's
`PoolRegion<S>`; each block is acquired from its `f32` pool and returned after
the sink call, preserving the shared hard budget. Signal-format mismatch,
cancellation, backend failure, and sink failure are terminal for that request.
The composition owner finalizes the sink on success and drops it on error; Host
does not own storage or encoding.

## Runtime boundary

The existing `kithara-engine` session thread is the canonical Host owner and the
lower PlayerSession actuator. It owns the root group, member values, Firewheel
context, and graph commands. Closing a player remains a caller-side two-phase
operation: close the runtime first, then detach the member. Calling close from
the owner loop could dispatch back into that same loop and deadlock.

Terminal shutdown first drops the command receiver, then drops session state,
then acknowledges the Host. This order disconnects queued callers before
Player destruction takes the per-player admission gate, so an admitted command
cannot wait on the owner while the owner waits on that command's gate.

A `HostOwned` endpoint contains the canonical member identity and the player's
cloneable control capability. It does not expose or own the inner Player,
retain players through `Arc` or `Weak`, or introduce a second command route.
The wasm remote Host's resident registry only retains Worker-bound runtime
values; membership remains canonical in the main-thread topology. The
immutable `RootView` publishes grid, topology, and status observations; it is a
read-only projection, never another mutable root.

Worker teardown attempts explicit removal once. Success detaches and releases
the resident; `SessionGone` ends teardown without a retry because the canonical
owner no longer exists. Any remote Host that still has resident entries when it
drops logs the invariant failure and retains only those players. It neither
retries a potentially permanent failure nor drops runtime state while a
main-thread topology may still reference its member.

`Host::insert` accepts one fully configured Player or decorator instance. The
instance already owns its stable grid identity. Insertion attaches an opaque
`SessionBinding` exactly once. Native transfers the instance into the Host
root. Wasm transfers only its synchronization state and current desired level
to the main-thread root, while the remote Worker Host retains the resident
instance. The session graph copies that level when the player registers and
owns later graph actuation. Decorators only delegate the binding and ownership
split to their resident Player. Neither a config builder nor a raw
`SessionDispatcher` crosses the insertion API.

Native and web dispatch wrap lower Player commands and Host topology commands
in one private envelope. On web, the main-thread Host and its Worker facade use
that same envelope and shared `RootView`; the Worker is never given a raw
`SessionHandle` with which to construct an unattached player.

The envelope is typed by the composition root's pool schema. `Host<S>` accepts
only `PlayerControlSource<Schema = S>`, and each graph deck retains the same
`PoolRegion<S>` handle carried by the player's registration command.

On web, one local Host is exclusive per JavaScript thread. It owns the session
state and every remote command receiver; TLS retains only a nongeneric active
Host flag. Sender and receiver wrappers are capabilities, not owners. Host
shutdown drops those receivers before session state so queued reply channels
disconnect and a Worker call cannot wait forever. A replacement Host starts
with cleared bridge playback observations.

If a command cannot be sent, ownership has not transferred and the original
operation is rejected as owner-unavailable. If the sole owner thread stops
after accepting an ownership-bearing command, commit state is unknowable. That
single post-send boundary deliberately fails fast instead of fabricating a
rejection or fallback state.

## Route and sample-rate invariant

`HostConfig` is the only product default for the initial output sample rate.
Each inserted Player is built for an explicit initial rate and the one-shot
`SessionBinding` carries only the canonical Host dispatcher. Insertion queries
that session and rejects a mismatch before the Player can register or start.
Session register/start commands derive their rate from the same query rather
than accepting another caller-supplied value.

Route changes keep the existing flow: the Host observes the device change and
the Player receives the resulting sample-rate update through its current
control path. Rebuilding the output graph is reserved for a physical route
change. Decode resampling, the Stream-to-decode hook, worker scheduling, fades,
and playback semantics are not redesigned by this extraction.

## Migration rule

Move existing session behavior mechanically and preserve command ordering,
errors, stream restart behavior, and native/web threading. Do not add fallback
routes or parallel synchronization state to bridge the move.
