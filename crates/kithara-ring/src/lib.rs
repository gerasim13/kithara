//! Lock-free single-producer single-consumer ring over caller-owned storage.
//!
//! [`split`] takes any owner of a contiguous slice of `Copy` values, such as a
//! pooled buffer, and returns the producer and consumer halves of a
//! [`ringbuf`] ring that reads and writes that slice in place. The owner is
//! dropped only after both halves are gone, so the memory is released through
//! the owner's own drop logic and never through the ring.

mod ring;
mod storage;

pub use ring::{Ring, RingCons, RingHalves, RingProd, split};
pub use storage::OwnedSlice;
