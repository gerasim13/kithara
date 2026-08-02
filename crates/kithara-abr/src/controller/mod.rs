//! Shared per-player ABR controller — registry, tick orchestration,
//! event throttling.
//!
//! Layout:
//! - `core` — [`AbrController`] struct + lifecycle/registration/peer-state
//!   callbacks + the [`AbrSettings`] config and [`AbrPeerId`] identifier.
//! - `tick` — `record_bandwidth` + `tick`. The `tick` body uses a
//!   `TickContext::resolve` Option-resolver to collapse the previous
//!   homogeneous let-else cascade — see `xtask/src/idioms/checks/guard_cascade.rs`
//!   module docs for the rationale.
//! - `throttle` — `EventThrottleCache` + `emit_throttled` + the
//!   `bytes_per_second` / `relative_delta` / `duration_delta` helpers.
//! - `peer` — internal `PeerEntry` struct.

mod core;
mod peer;
mod throttle;
mod tick;

pub use core::{AbrController, AbrPeerId, AbrSettings};
