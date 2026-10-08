#![forbid(unsafe_code)]

//! Storage resources with driver-selected backends and lifecycle typestate.
//! [`ResourceWriter`] owns writes; Committed seals them; [`ResourceReader`] is a cloneable read view.
//! [`MmapResource`] and [`MemResource`] back [`StorageResource`]; [`ResourceRead`] seals consumer reads.

mod backend;
mod decorator;
mod error;
mod resource;
#[cfg(test)]
pub(crate) use kithara_test_utils::bufpool as test_pools;
mod unified;

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub use backend::{
    Active, AvailabilityObserver, Committed, Driver, DriverIo, MemDriver, MemOptions, MemResource,
    Reader, Resource, ResourcePhase, ResourceRead, ResourceReader, ResourceWriter,
};
#[cfg(not(target_arch = "wasm32"))]
pub use backend::{MmapDriver, MmapOptions, MmapResource};
pub use decorator::{Atomic, AtomicChunked, Barrier, OpenIntent};
pub use error::{StorageError, StorageResult};
pub use resource::{OpenMode, ResourceStatus, WaitOutcome};
pub use unified::StorageResource;
mod consts;
