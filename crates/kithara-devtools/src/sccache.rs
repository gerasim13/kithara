use std::{env, ffi::OsStr};

use crate::consts;

/// Whether this run is a CI job, whose build shares the fleet's compiler cache.
fn in_ci() -> bool {
    set(env::var_os("CI").as_deref())
}

fn set(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

/// What a Clippy run must drop from its environment to get the caching that
/// suits where it runs, or nothing when what it inherited already suits it.
pub(crate) fn clippy_cleared() -> &'static [&'static str] {
    clippy_cleared_for(in_ci())
}

/// `sccache` aborts outright rather than fall back when it meets an incremental
/// build, so a Clippy run gets one of the two and never both. Which one is worth
/// more depends entirely on where it runs.
///
/// On a workstation, incremental: the dependencies are already built in the
/// local target directory, so the cache would serve almost nothing, while
/// incremental turns a fifteen-second re-check into two. Dropping the two
/// variables is what selects it - Cargo already compiles workspace crates
/// incrementally and registry ones never, so only the blanket
/// `CARGO_INCREMENTAL=0` the `justfile` exports was suppressing it. Answering
/// with `1` instead says the same thing to Cargo and one thing more to
/// everything else: `sccache` reads that variable too, so a C or C++ dependency
/// reaching it through a `CMake` compiler launcher dies on a Rust flag it never
/// used.
///
/// In CI, the cache: `sccache` is one content-addressed volume shared by every
/// runner, so one runner's entry is another's hit, while incremental state is
/// per-runner, invalidated by any toolchain or feature change, and was how a
/// build directory grew past two hundred gigabytes. Clearing the wrapper there
/// is what made every job re-check five hundred dependency crates from source.
///
/// Only `clippy-driver`'s own compilations - the workspace crates - go
/// uncached either way, which is the part this cannot help.
const fn clippy_cleared_for(in_ci: bool) -> &'static [&'static str] {
    if in_ci {
        &[]
    } else {
        &[consts::WRAPPER, consts::INCREMENTAL]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ci_clippy_run_keeps_the_shared_cache() {
        assert!(clippy_cleared_for(true).is_empty());
    }

    #[test]
    fn a_workstation_clippy_run_trades_the_cache_for_incremental() {
        assert_eq!(
            clippy_cleared_for(false),
            [consts::WRAPPER, consts::INCREMENTAL]
        );
    }
}
