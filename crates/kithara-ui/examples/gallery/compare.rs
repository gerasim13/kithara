//! Compares two capture sets page by page and says, in numbers, where they
//! disagree.
//!
//! Enabled by `KITHARA_GALLERY_COMPARE=<a>:<b>:<out>`. The comparison itself
//! belongs to the toolkit; what stands here is the way this example is asked
//! for one.

use std::{
    env,
    path::{Path, PathBuf},
};

use kithara_ui::capture::diff;

/// What a comparison run decided.
pub(super) enum Verdict {
    /// No comparison was asked for; the caller falls through.
    NotAsked,
    /// Every page compared within its budget, or no budget was given.
    Passed,
    /// A page differed more than its budget allows, or was missing from a set.
    Failed,
}

/// Runs the comparison when asked.
///
/// A budget turns the numbers into a gate: `KITHARA_GALLERY_COMPARE_BUDGET`
/// names a file of per-page allowances, and a page over its allowance — or
/// missing from either set — fails the run. Without one this only reports.
pub(super) fn run() -> Verdict {
    let Some(spec) = env::var_os("KITHARA_GALLERY_COMPARE") else {
        return Verdict::NotAsked;
    };
    let budget = env::var_os("KITHARA_GALLERY_COMPARE_BUDGET").map(PathBuf::from);
    match compare(&spec.to_string_lossy(), budget.as_deref()) {
        Ok(passed) => {
            if passed {
                Verdict::Passed
            } else {
                Verdict::Failed
            }
        }
        Err(error) => {
            eprintln!("compare failed: {error}");
            Verdict::Failed
        }
    }
}

fn compare(spec: &str, budget: Option<&Path>) -> Result<bool, String> {
    let parts = spec.split(':').collect::<Vec<_>>();
    let [left, right, out] = parts.as_slice() else {
        return Err("expected KITHARA_GALLERY_COMPARE=<a>:<b>:<out>".to_owned());
    };
    let report = diff::compare(Path::new(left), Path::new(right), Path::new(out), budget)?;
    print!("{report}");
    Ok(report.passed())
}
