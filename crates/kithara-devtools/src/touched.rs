use std::{collections::BTreeMap, process::Command};

use anyhow::{Context, Result, bail};

use crate::common::project::{TestCommandConfig, TestLaneConfig};

/// Lanes the working branch touches, in configuration order.
///
/// The default branch is where branches land, so it runs every owning lane
/// rather than the diff of whatever merged last.
pub(crate) fn lanes(test: &TestCommandConfig) -> Result<Vec<String>> {
    // Best effort: a CI checkout carries only the pushed ref, and a workstation
    // may have no network. What has to hold is that `origin/main` resolves.
    let _ = Command::new("git")
        .args(["fetch", "--no-tags", "--quiet", "origin", "main"])
        .status();
    let base = git(&["merge-base", "origin/main", "HEAD"])?;
    if base == git(&["rev-parse", "HEAD"])? {
        return Ok(owning(&test.lanes));
    }
    let range = format!("{base}...HEAD");
    let changed = git(&["diff", "--name-only", &range])?;
    let changed: Vec<&str> = changed.lines().collect();
    Ok(select(&test.lanes, &test.shared_paths, &changed))
}

fn select(
    lanes: &BTreeMap<String, TestLaneConfig>,
    shared: &[String],
    changed: &[&str],
) -> Vec<String> {
    if changed
        .iter()
        .any(|path| shared.iter().any(|shared| shared == path))
    {
        return owning(lanes);
    }
    lanes
        .iter()
        .filter(|(_, lane)| {
            lane.owns
                .iter()
                .any(|prefix| changed.iter().any(|path| path.starts_with(prefix)))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn owning(lanes: &BTreeMap<String, TestLaneConfig>) -> Vec<String> {
    lanes
        .iter()
        .filter(|(_, lane)| !lane.owns.is_empty())
        .map(|(name, _)| name.clone())
        .collect()
}

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8(output.stdout).context("git printed non-UTF-8")?;
    Ok(text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(owned: &[(&str, &[&str])]) -> BTreeMap<String, TestLaneConfig> {
        owned
            .iter()
            .map(|(name, owns)| {
                let lane = TestLaneConfig {
                    owns: owns.iter().map(|owns| (*owns).to_owned()).collect(),
                    ..TestLaneConfig::default()
                };
                ((*name).to_owned(), lane)
            })
            .collect()
    }

    #[test]
    fn a_lane_is_selected_by_a_change_under_a_path_it_owns() {
        let lanes = config(&[("tooling", &["xtask/"]), ("harness", &["crates/test/"])]);

        let selected = select(&lanes, &[], &["xtask/src/main.rs"]);

        assert_eq!(selected, vec!["tooling".to_owned()]);
    }

    #[test]
    fn a_lane_owning_nothing_the_branch_changed_stays_out() {
        let lanes = config(&[("tooling", &["xtask/"]), ("harness", &["crates/test/"])]);

        let selected = select(&lanes, &[], &["crates/other/src/lib.rs"]);

        assert!(selected.is_empty(), "selected {selected:?}");
    }

    #[test]
    fn a_shared_path_runs_every_owning_lane() {
        let lanes = config(&[
            ("tooling", &["xtask/"]),
            ("harness", &["crates/test/"]),
            ("workspace", &[]),
        ]);
        let shared = [".config/xtask.toml".to_owned()];

        let selected = select(&lanes, &shared, &[".config/xtask.toml"]);

        assert_eq!(selected, vec!["harness".to_owned(), "tooling".to_owned()]);
    }

    #[test]
    fn a_lane_declaring_no_ownership_is_never_selected_by_a_diff() {
        let lanes = config(&[("workspace", &[])]);

        let selected = select(&lanes, &[], &["crates/other/src/lib.rs"]);

        assert!(selected.is_empty(), "selected {selected:?}");
    }

    #[test]
    fn a_prefix_matches_a_file_as_well_as_a_directory() {
        let lanes = config(&[("harness", &["crates/kithara-platform/tests/flash_"])]);

        let selected = select(
            &lanes,
            &[],
            &["crates/kithara-platform/tests/flash_lexical.rs"],
        );

        assert_eq!(selected, vec!["harness".to_owned()]);
    }
}
