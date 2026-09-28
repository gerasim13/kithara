//! A lane builds in a slot of its own pool: the first slot no other job holds,
//! with a lock file beside it, so jobs of one lane build side by side instead
//! of queueing. A slot outlives checkouts, and cargo judges freshness by mtime.
//! A persistent checkout keeps the mtime of every file a branch switch left
//! alone, so artifacts another checkout built from other sources can read as
//! newer and be reused unbuilt. The slot records which content its artifacts
//! may come from, and a claim stamps every file whose content is not the only
//! one recorded. A build the record did not see leaves artifacts of unknown
//! content, so every file is stamped until a lane succeeds again. A claim
//! first prunes the units the slot's builds stopped using.

mod claim;
mod layout;
mod pool;
mod prune;

pub(crate) use claim::LaneBuild;
pub(crate) use pool::{SlotPool, lock_of};
