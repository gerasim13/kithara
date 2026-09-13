/// Current default linker for Linux CI jobs, as target-scoped Cargo variables.
///
/// Cargo timing establishes that the build dominates the test lane but does
/// not split code generation from linking, so `lld` is a controlled candidate,
/// not a root-cause conclusion. The image also carries `mold` for a separately
/// measured target-scoped override. `sccache` cannot reuse final link outputs.
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
