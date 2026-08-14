/// Maximum jobs admitted by one CI host.
///
/// Runner rendering and per-job cache partitioning share this value. Raising it
/// buys wall-clock and costs disk: every admitted job carries its own checkout
/// and `target`, and those are what the volume runs out of. The compiler cache
/// follows on its own — the host's budget is divided between the slots.
pub(crate) const HOST_JOB_CONCURRENCY: usize = 3;

/// Host-global lock namespace that coordinates the compiler-cache slots.
pub(crate) const SCCACHE_SLOT_CONTROL_NAMESPACE: &str = ".kithara-ci-sccache-slots";

/// CI-owned compiler-cache slots, kept disjoint from the local cache directory.
pub(crate) const SCCACHE_SLOT_CACHE_NAMESPACE: &str = "sccache-slots";
