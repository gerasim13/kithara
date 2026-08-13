/// Maximum jobs admitted by one CI host.
///
/// Runner rendering and per-job cache partitioning share this value.
pub(crate) const HOST_JOB_CONCURRENCY: usize = 2;

/// Host-global lock namespace that coordinates the compiler-cache slots.
pub(crate) const SCCACHE_SLOT_CONTROL_NAMESPACE: &str = ".kithara-ci-sccache-slots";

/// CI-owned compiler-cache slots, kept disjoint from the local cache directory.
pub(crate) const SCCACHE_SLOT_CACHE_NAMESPACE: &str = "sccache-slots";
