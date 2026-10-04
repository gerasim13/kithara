use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{
    common::project::ProjectConfig,
    test::{LaneChoice, LaneToggles, NextestAction, resolve},
};

#[derive(Debug)]
pub(crate) struct SuiteBin {
    pub(crate) binary_path: PathBuf,
    pub(crate) cwd: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ListOutput {
    #[serde(rename = "rust-suites")]
    rust_suites: BTreeMap<String, ListSuite>,
}

#[derive(Debug, Deserialize)]
struct ListSuite {
    #[serde(rename = "binary-path")]
    binary_path: PathBuf,
    cwd: PathBuf,
}

pub(crate) fn parse_list(json: &str) -> Result<BTreeMap<String, SuiteBin>> {
    let parsed: ListOutput = serde_json::from_str(json).context("parse nextest list json")?;
    Ok(parsed
        .rust_suites
        .into_iter()
        .map(|(id, suite)| {
            (
                id,
                SuiteBin {
                    binary_path: suite.binary_path,
                    cwd: suite.cwd,
                },
            )
        })
        .collect())
}

pub(crate) fn nextest_list(
    project: &ProjectConfig,
    flash: bool,
    backend: &str,
    target_dir: &Path,
) -> Result<BTreeMap<String, SuiteBin>> {
    let test = &project.test;
    let mut cmd = resolve(
        test,
        &LaneChoice {
            backend,
            features: &[],
            lane: &test.default_lane,
            toggles: LaneToggles {
                flash,
                no_block: false,
            },
        },
    )?
    .command(
        NextestAction::List,
        &["--message-format".to_owned(), "json".to_owned()],
    )?;
    cmd.env("CARGO_TARGET_DIR", target_dir);
    let out = cmd.output().context("run cargo nextest list")?;
    if !out.status.success() {
        bail!(
            "nextest list failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    parse_list(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parses_suite_binaries() {
        const JSON: &str = r#"{
      "rust-build-meta": {},
      "test-count": 2,
      "rust-suites": {
        "demo-tests::suite_light": {
          "package-name": "demo-tests",
          "binary-path": "/t/deps/suite_light-abc",
          "cwd": "/repo/tests",
          "test-cases": { "offline::gapless": { "ignored": false } }
        }
      }
    }"#;

        let suites = parse_list(JSON).expect("parse nextest list");
        let s = &suites["demo-tests::suite_light"];

        assert_eq!(s.binary_path, PathBuf::from("/t/deps/suite_light-abc"));
        assert_eq!(s.cwd, PathBuf::from("/repo/tests"));
    }
}
