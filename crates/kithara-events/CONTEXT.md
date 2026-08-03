# kithara-events — Context

## Bus routing

- One root `EventBus` per player; `scoped()` / `scoped_labeled()` mint children sharing the same topic
  registry, each with its own `tokio::sync::broadcast` channel.
- Publishing sends to the publisher's channel **and every ancestor**. Subscribing to a scope sees only
  that scope's subtree; siblings are invisible. Isolation comes from channel topology — there is no
  receiver-side filtering.
- Bus ids come from a process-wide monotonic counter and are never reused. Dropping a bus removes its
  topic only when its channel has no receivers left.
- Capacity is per channel, fixed at root creation (`DEFAULT_EVENT_BUS_CAPACITY` = 1024, clamped to ≥ 1)
  and inherited by children. A slow subscriber gets `RecvError::Lagged(n)`; events are dropped for that
  receiver, never buffered unboundedly.
- Every event is wrapped in `Envelope { event, meta }`. `EventMeta` carries `origin` (publishing scope
  id), `seq` from one counter shared by the whole hierarchy, `ts_micros` relative to a process-wide
  `Instant` base, and the `deck` / `track` labels. `ScopeLabel` is inherited: a child's `Some`
  overrides, `None` keeps the parent's value.

## DeferredBus — decode-core hand-off

`broadcast::Sender::send` takes an internal lock, forbidden on the worker's forbid-blocking decode core,
so `DeferredBus<E>` splits publishing. `enqueue` runs on the decode core: lock-free, alloc-free push into
a fixed `ArrayQueue`, stamping `seq` and `ts_micros` at enqueue time so ordering reflects decode order,
not flush order; a full ring **drops** the event and bumps a counter (the only high-volume producer is
monotonic progress, where the next pass supersedes the drop). `flush` runs in the unchecked scheduler
shell, drains FIFO, publishes, and surfaces anything dropped since the last flush as
`BusEvent::Overflow { scope, dropped }` with the exact count. The ring element is the narrow per-domain
event (`FileEvent` / `HlsEvent`); conversion to `Event` happens at publish time so the ring stays small.

## Features

All `Event` variants and sub-enums are feature-gated; defaults turn everything on. Default-on domains:
`abr`, `app`, `asset`, `audio`, `decoder`, `downloader`, `drm`, `file`, `hls`, `player`, `queue`; `hls`
implies `abr`; `downloader` alone pulls dependencies (`kithara-net`, `url`). `Event::Bus`, `Envelope`,
`EventBus`, `BusScope`, `EventReceiver`, `SlotId`, `TrackId`, and `SeekEpoch` are unconditional. Off by
default: `client-reqwest`, `client-wreq`, `tls-rustls`, `tls-native` — they forward HTTP backend and TLS
selection to `kithara-net` through weak (`?`) refs, so enabling one never force-enables that optional dep.

## Conversions

- `From<…Event> for Event` for every subsystem event, so `bus.publish(HlsEvent::EndOfStream)` works
  without naming the top-level enum.
- `TrackId` ↔ `u64` both ways, plus `Display`. Ids come from `TrackId::allocate()`, one process-wide
  counter starting at 0 and never reset, so the `audioId` reported over FFI is exactly the queue's
  internal value.
- `AbrMode` ↔ `usize` is a packed FFI encoding, not a plain index: `Manual(i)` → `i`, `Auto(None)` →
  `usize::MAX`, `Auto(Some(v))` → `usize::MAX - 1 - v`. Values at or above `usize::MAX / 2` decode as
  `Auto`.
- `Duration` → `MediaTime` (`From`) and `&MediaTime` → `Duration` (`TryFrom`, rejecting invalid and
  indefinite times); `Display` on `FileError`, `HlsError`, `AudioFormat`, `VariantIndex`, `TrackId`.
- `FileError` covers only local/non-network failures and `FileEvent` variants are reader-side facts;
  network failures and request lifecycle arrive as `DownloaderEvent` (with a typed
  `kithara_net::NetError`) on the same bus scope.
