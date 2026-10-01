//! A lane builds in a slot of its own pool: the first slot no other job holds,
//! with a lock file beside it, so jobs of one lane build side by side instead
//! of queueing. A slot outlives checkouts, and a claim first prunes the units
//! the slot's builds stopped using. What it does next depends on how the lane
//! judges freshness.
//!
//! A lane that judges by mtime reads a persistent checkout, which keeps the
//! mtime of every file a branch switch left alone, so artifacts another
//! checkout built from other sources can read as newer and be reused unbuilt.
//! The slot records which content its artifacts may come from, and a claim
//! stamps every file whose content is not the only one recorded. A build the
//! record did not see leaves artifacts of unknown content, so every file is
//! stamped until a lane succeeds again; a job that fails or dies recorded its
//! content before it built, so it is not such a build.
//!
//! A lane that judges by checksum reads a fresh checkout on any runner, so its
//! sources need no stamp; only build-script runs are still judged by mtime. A
//! claim keeps a run only when the content of every checkout path it watches
//! is the content recorded, and nothing else it watches changed since, then
//! dates every artifact at the claim so no fresh checkout reads as newer.
//! A rebuild check replays that claim after the job's build, so cargo can say
//! what the next job of the same commit would build.

mod claim;
#[cfg(test)]
mod fixture;
mod layout;
mod pool;
mod prune;
mod sources;
mod tracked;
mod units;

pub(crate) use claim::LaneBuild;
pub(crate) use pool::{SlotPool, lock_of};
