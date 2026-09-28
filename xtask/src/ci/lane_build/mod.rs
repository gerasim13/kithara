//! A lane's build directory is shared by every checkout on the fleet, and
//! cargo judges freshness by mtime. A persistent checkout keeps the mtime of
//! every file a branch switch left alone, so artifacts another checkout built
//! from other sources can read as newer than those files and be reused
//! unbuilt. The directory records which content its artifacts may come from,
//! and a checkout that claims it stamps every file whose content is not the
//! only one recorded, so cargo rebuilds exactly those and reuses the rest.
//! A build the record did not see — a job that never claimed the directory,
//! or one that died before releasing it — leaves artifacts of unknown content,
//! so every file is stamped until a lane succeeds again.
//! A claim first prunes the units the directory's builds stopped using.

mod claim;
mod layout;
mod prune;

pub(crate) use claim::LaneBuild;
