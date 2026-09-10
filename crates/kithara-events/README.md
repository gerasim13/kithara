<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-events.svg)](https://crates.io/crates/kithara-events)
[![docs.rs](https://docs.rs/kithara-events/badge.svg)](https://docs.rs/kithara-events)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-events

Typed event bus for the kithara audio pipeline. Each `EventBus` scope owns a broadcast channel per subscribed event type. Consumer-owned `EventSet` enums combine the types a subscriber needs.

## Usage

```rust
use kithara_events::{Event, EventBus};

#[derive(Clone, Debug, Event)]
struct Progress(u64);

let bus = EventBus::new(64);
let mut rx = bus.subscribe::<Progress>();

bus.publish(Progress(42));
assert_eq!(rx.try_recv()?.event.0, 42);
```

## Key Types

| Type | Role |
| --- | --- |
| `EventBus` | Scoped typed publisher |
| `BusScope` | Hierarchical scope and inherited labels |
| `Envelope` | Event value with publication metadata |
| `EventReceiver` | Receiver for an `EventSet` |
| `DeferredBus` | Fixed-capacity deferred publisher |
| `Event` | Marker trait and derive for event values |
| `EventSet` | Consumer-owned set and derive |
| `SlotId` | Bus identity for a player slot |
| `TrackId` | Bus identity for a queue item |

## Features

None.

## Integration

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-events) for detailed contracts and ownership rules.
