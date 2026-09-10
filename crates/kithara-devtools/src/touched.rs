use std::{collections::BTreeMap, process::Command};

use anyhow::{Context, Result, bail};

use crate::common::project::{TestCommandConfig, TestLaneConfig};

/// Lanes a branch touches, or the complete workspace fallback.
///
/// The default branch and paths with no reviewed owner run the complete
/// workspace lane. A narrow lane is therefore an opt-in coverage reduction,
/// never the result of failing to classify a changed path.
pub(crate) fn lanes(test: &TestCommandConfig) -> Result<Vec<String>> {
    // Best effort: a CI checkout carries only the pushed ref, and a workstation
    // may have no network. What has to hold is that `origin/main` resolves.
    let _ = Command::new("git")
        .args(["fetch", "--no-tags", "--quiet", "origin", "main"])
        .status();
    let base = git(&["merge-base", "origin/main", "HEAD"])?;
    if base == git(&["rev-parse", "HEAD"])? {
        return Ok(vec![test.default_lane.clone()]);
    }
    let range = format!("{base}...HEAD");
    let changed = git(&["diff", "--name-only", &range])?;
    let changed: Vec<&str> = changed.lines().collect();
    Ok(select(
        &test.lanes,
        &test.shared_paths,
        &test.default_lane,
        &changed,
    ))
}

fn select(
    lanes: &BTreeMap<String, TestLaneConfig>,
    shared: &[String],
    fallback: &str,
    changed: &[&str],
) -> Vec<String> {
    if changed
        .iter()
        .any(|path| shared.iter().any(|shared| shared == path))
    {
        return vec![fallback.to_owned()];
    }
    let selected: Vec<_> = lanes
        .iter()
        .filter(|(_, lane)| {
            lane.owns
                .iter()
                .any(|prefix| changed.iter().any(|path| path.starts_with(prefix)))
        })
        .map(|(name, _)| name.clone())
        .collect();
    if selected.is_empty() {
        vec![fallback.to_owned()]
    } else {
        selected
    }
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
    use std::path::Path;

    use super::*;
    use crate::common::project::ProjectConfig;

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

        let selected = select(&lanes, &[], "workspace", &["xtask/src/main.rs"]);

        assert_eq!(selected, vec!["tooling".to_owned()]);
    }

    #[test]
    fn an_unowned_path_falls_back_to_the_complete_workspace() {
        let lanes = config(&[("tooling", &["xtask/"]), ("harness", &["crates/test/"])]);

        let selected = select(&lanes, &[], "workspace", &["crates/other/src/lib.rs"]);

        assert_eq!(selected, vec!["workspace"]);
    }

    #[test]
    fn a_shared_path_falls_back_to_the_complete_workspace() {
        let lanes = config(&[
            ("tooling", &["xtask/"]),
            ("harness", &["crates/test/"]),
            ("workspace", &[]),
        ]);
        let shared = [".config/xtask.toml".to_owned()];

        let selected = select(&lanes, &shared, "workspace", &[".config/xtask.toml"]);

        assert_eq!(selected, vec!["workspace"]);
    }

    #[test]
    fn an_empty_ownership_catalog_falls_back_to_the_complete_workspace() {
        let lanes = config(&[("workspace", &[])]);

        let selected = select(&lanes, &[], "workspace", &["crates/other/src/lib.rs"]);

        assert_eq!(selected, vec!["workspace"]);
    }

    #[test]
    fn a_prefix_matches_a_file_as_well_as_a_directory() {
        let lanes = config(&[("harness", &["crates/kithara-platform/tests/flash_"])]);

        let selected = select(
            &lanes,
            &[],
            "workspace",
            &["crates/kithara-platform/tests/flash_lexical.rs"],
        );

        assert_eq!(selected, vec!["harness".to_owned()]);
    }

    #[test]
    fn repository_lanes_keep_domain_ownership_narrow() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let project = ProjectConfig::load(root).expect("repository config");
        let test = &project.test;

        assert_eq!(
            select(
                &test.lanes,
                &test.shared_paths,
                &test.default_lane,
                &["crates/kithara-stream/src/lib.rs"],
            ),
            vec!["core"],
        );
        assert!(
            test.lanes["core"]
                .prefix_args
                .windows(2)
                .any(|args| args == ["-p", "kithara-core-test-fixtures"]),
            "the core lane must compile its shared test inputs"
        );
        assert_eq!(
            select(
                &test.lanes,
                &test.shared_paths,
                &test.default_lane,
                &["tests/crates/core/src/lib.rs"],
            ),
            vec!["core"],
        );
        assert_eq!(
            select(
                &test.lanes,
                &test.shared_paths,
                &test.default_lane,
                &["xtask/tests/lane_config.rs"],
            ),
            vec!["tooling"],
        );
        assert_eq!(
            select(
                &test.lanes,
                &test.shared_paths,
                &test.default_lane,
                &["crates/kithara-ui/src/atoms/button.rs"],
            ),
            vec!["ui"],
        );
        assert_eq!(
            select(
                &test.lanes,
                &test.shared_paths,
                &test.default_lane,
                &["crates/kithara-devtools/tests/config_contract.rs"],
            ),
            vec!["tooling"],
        );
    }
}
