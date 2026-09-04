/// Maximum jobs admitted by one CI host.
///
/// Runner rendering and per-job cache partitioning share this value. Raising it
/// buys wall-clock and costs disk: every admitted job carries its own checkout
/// and `target`, and those are what the volume runs out of. The compiler cache
/// follows on its own — the host's budget is divided between the slots. Two
/// jobs keep the required parallelism while leaving four Cargo workers to each
/// on this ten-core host; three measured slower and evicted a third test tree.
pub(crate) const HOST_JOB_CONCURRENCY: usize = 2;

/// Cores the CI host has.
///
/// Job admission and per-job Cargo workers are both carved out of this, and
/// nothing else states the relation: raising the first without lowering the
/// second is how the machine ends up with more compilers than cores and no
/// room left for the runner, sccache, and the linkers.
pub(crate) const HOST_CORES: usize = 10;

/// Linker every Linux CI job links with, as target-scoped Cargo variables.
///
/// A test job spends more wall-clock linking than compiling: measured on the
/// GitHub fleet, a warm `Tests (simulated clock)` reached the last `Compiling`
/// line five and a half minutes before the profile finished, and what filled
/// that gap was `bfd` linking fifty-two optimised test binaries. `sccache`
/// cannot shorten it — it declines to cache anything that invokes the system
/// linker — so the linker itself is the only lever. `lld` is in the CI image
/// already and was never selected.
///
/// Scoped per target rather than through `RUSTFLAGS`, which would follow the
/// wasm and Apple builds to hosts that have no `ld.lld`. Both Linux triples are
/// named because the fleet is x86-64 and the image builds on Apple silicon;
/// the one that does not apply is inert.
pub(crate) const LINUX_LINKER_ENV: [(&str, &str); 2] = [
    (
        "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
        LINUX_LINKER_RUSTFLAGS,
    ),
    (
        "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
        LINUX_LINKER_RUSTFLAGS,
    ),
];

const LINUX_LINKER_RUSTFLAGS: &str = "-Clink-arg=-fuse-ld=lld";

/// Host-global lock namespace that coordinates the compiler-cache slots.
pub(crate) const SCCACHE_SLOT_CONTROL_NAMESPACE: &str = ".kithara-ci-sccache-slots";

/// CI-owned compiler-cache slots, kept disjoint from the local cache directory.
pub(crate) const SCCACHE_SLOT_CACHE_NAMESPACE: &str = "sccache-slots";
