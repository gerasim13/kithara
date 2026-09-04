//! Worker-neutral PCM ports and blocking-reader wake primitives.

mod ports;
pub(crate) mod wake;

pub(crate) use ports::{Inlet, Outlet, WakeSignal, connect};
