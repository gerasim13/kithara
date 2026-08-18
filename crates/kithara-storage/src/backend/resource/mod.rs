#![forbid(unsafe_code)]

//! Generic `ResourceCore<D>` state machine + the typed `Resource<S, D>` handles.
//!
//! - `state` — `ResourceCore<D>`, `Inner`/`CommonState`, ctors, `check_health`.
//! - `io` — `read_at_inner` + `write_at_inner` bodies.
//! - `wait` — `wait_range_inner` body.
//! - `lifecycle` — commit / fail / reactivate / inspect bodies.
//! - `retire` — the bin where produce-core reads park availability snapshots
//!   so the write side, never the audio thread, pays their frees.
//! - `handle` — phantom-typestate handles (`Resource<S, D>`,
//!   `ResourceWriter`/`ResourceReader` aliases) + the sealed `ResourceRead`
//!   trait.

pub(crate) mod handle;
pub(crate) mod io;
pub(crate) mod lifecycle;
pub(super) mod retire;
pub(crate) mod state;
pub(crate) mod wait;

pub use handle::{
    Active, Committed, Reader, Resource, ResourcePhase, ResourceRead, ResourceReader,
    ResourceWriter,
};
