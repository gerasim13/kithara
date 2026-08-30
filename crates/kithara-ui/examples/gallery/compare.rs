//! Compares two capture sets page by page and says, in numbers, where they
//! disagree.
//!
//! Asked for by `--compare <a> <b> <out>`. The comparison itself belongs to the
//! toolkit; what stands here is the way this example is asked for one.

use std::path::{Path, PathBuf};

use kithara_ui::capture::diff;

/// Compares the two sets and writes a difference image per page.
///
/// A budget turns the numbers into a gate: it prices each page, and a page
/// over its price — or missing from either set — fails the run. Without one
/// this only reports.
pub(super) fn run(sets: &[PathBuf], budget: Option<&Path>) -> Result<bool, String> {
    let [left, right, out] = sets else {
        return Err(format!(
            "a comparison takes two sets and a folder to write to, got {} paths",
            sets.len(),
        ));
    };
    let report = diff::compare(left, right, out, budget)?;
    print!("{report}");
    Ok(report.passed())
}
