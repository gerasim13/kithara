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

/// Selects caching in CI and incremental Clippy checks on workstations;
/// `sccache` aborts on incremental builds, so the two cannot coexist.
/// Local dependencies already reside in the target, making incremental checks
/// more useful. Removing both variables lets Cargo choose its normal policy;
/// setting `CARGO_INCREMENTAL=1` would also make C/C++ sccache launchers abort.
/// CI shares content-addressed cache entries across runners; incremental state
/// is runner-local, feature/toolchain-specific, and grows build directories.
/// `clippy-driver` workspace compilations remain uncached in either case.
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
