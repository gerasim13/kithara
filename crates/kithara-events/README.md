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

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>EventBus</code></td><td>Clone-able broadcast publisher; <code>publish()</code> works from both async and blocking contexts</td></tr>

<tr><td><code>BusScope</code></td><td>Hierarchical scope used to attribute events to player/track/peer subtrees</td></tr>

<tr><td><code>EventReceiver</code></td><td>Subscriber handle returned by <code>EventBus::subscribe()</code></td></tr>

<tr><td><code>Event</code></td><td>Marker trait for a concrete event type</td></tr>

<tr><td><code>SeekEpoch</code></td><td>Monotonic seek-generation tag carried across subsystems</td></tr>

</table>

## Features

Domain event modules are currently feature-gated; the bus and consumer sets are independent of those gates.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-events) for detailed contracts, invariants, and internals.
