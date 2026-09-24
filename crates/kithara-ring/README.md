<div align="center">

<img src="https://raw.githubusercontent.com/zvuk/kithara/main/logo.svg" alt="kithara" width="300">

</div>

<div align="center">

[![crates.io](https://img.shields.io/crates/v/kithara-ring.svg)](https://crates.io/crates/kithara-ring)
[![docs.rs](https://docs.rs/kithara-ring/badge.svg)](https://docs.rs/kithara-ring)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/zvuk/kithara/blob/main/LICENSE-MIT)

</div>

# kithara-ring

Lock-free single-producer single-consumer ring over caller-owned storage. Any
owner of a contiguous slice of `Copy` values, such as a pooled buffer, becomes
the slots of a `ringbuf` ring; the owner is dropped through its own logic once
both halves are gone, so the memory never leaves the allocator it came from.

## Usage

```rust
use kithara_ring::split;
use ringbuf::traits::{Consumer, Producer};

let Ok((mut prod, mut cons)) = split(vec![0.0_f32; 4]) else {
    unreachable!("a non-empty owner forms a ring");
};
assert_eq!(prod.push_slice(&[1.0, 2.0]), 2);
let mut out = [0.0; 2];
assert_eq!(cons.pop_slice(&mut out), 2);
assert_eq!(out, [1.0, 2.0]);
```

## Key Types

<table>

<tr><th>Type</th><th>Role</th></tr>

<tr><td><code>split</code></td><td>Turns a non-empty owner into the producer and consumer halves</td></tr>

<tr><td><code>RingProd</code> / <code>RingCons</code></td><td>Halves of the shared ring; either may drop first</td></tr>

<tr><td><code>OwnedSlice</code></td><td>Ring storage that keeps the owner alive and addresses its slice in place</td></tr>

</table>

## Integration

`kithara-bufpool` builds pooled rings on top of this crate, so a ring's slots
count against the pool budget. The crate carries no domain; it only fixes the
owner in place for the ring's lifetime.

See [crate contracts](https://github.com/zvuk/kithara/wiki/kithara-ring) for detailed contracts, invariants, and internals.
