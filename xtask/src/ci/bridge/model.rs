use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Paths that define the pipeline judging an imported commit. A merged pull
/// request touching one of these would rewrite its own judge, so the import
/// stops and the change is ported through a reviewed merge request instead.
///
/// Manifests are deliberately absent. `Cargo.toml` and `Cargo.lock` do not
/// touch the judge — the trusted pipeline runs the real suite against them —
/// and refusing them outright turned every dependency bump into a manual port.
/// Their risk is a build script running on the runner, which is what
/// `deps:deny` covers on the quarantine pipeline.
pub(super) const CONTROL_PATHS: &[&str] = &[
    ".gitlab-ci.yml",
    ".gitlab/",
    ".config/ci-pins.toml",
    ".config/just/",
    ".config/mutation-suites.toml",
    ".config/nextest.toml",
    ".config/xtask.toml",
    "ci/",
    "docker/",
    "justfile",
    "xtask/",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Direction {
    Equal,
    GithubAhead,
    GitlabAhead,
    Diverged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ImportState {
    Testing,
    Promoted,
    Rejected,
}

pub(super) fn direction_for(
    github_sha: &str,
    gitlab_sha: &str,
    mut is_ancestor: impl FnMut(&str, &str) -> Result<bool>,
) -> Result<Direction> {
    if github_sha == gitlab_sha {
        return Ok(Direction::Equal);
    }
    if is_ancestor(gitlab_sha, github_sha)? {
        return Ok(Direction::GithubAhead);
    }
    if is_ancestor(github_sha, gitlab_sha)? {
        return Ok(Direction::GitlabAhead);
    }
    Ok(Direction::Diverged)
}

pub(super) fn validate_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(super) fn simple_branch(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn simple_repository(value: &str) -> bool {
    let mut parts = value.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next()),
        (Some(owner), Some(name), None)
            if simple_repository_part(owner) && simple_repository_part(name)
    )
}

fn simple_repository_part(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn regular_file(path: &Path) -> bool {
    path.is_absolute()
        && path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_are_ancestry_based() {
        let github_ahead = direction_for("github", "gitlab", |older, newer| {
            Ok((older, newer) == ("gitlab", "github"))
        })
        .unwrap();
        assert_eq!(github_ahead, Direction::GithubAhead);

        let gitlab_ahead = direction_for("github", "gitlab", |older, newer| {
            Ok((older, newer) == ("github", "gitlab"))
        })
        .unwrap();
        assert_eq!(gitlab_ahead, Direction::GitlabAhead);

        assert_eq!(
            direction_for("same", "same", |_, _| Ok(false)).unwrap(),
            Direction::Equal
        );
        assert_eq!(
            direction_for("github", "gitlab", |_, _| Ok(false)).unwrap(),
            Direction::Diverged
        );
    }

    #[test]
    fn repository_and_branch_values_are_bounded() {
        assert!(simple_repository("zvuk/kithara"));
        assert!(!simple_repository("zvuk/kithara/extra"));
        assert!(!simple_repository("zvuk/kithara?token=secret"));
        assert!(simple_branch("main"));
        assert!(!simple_branch("heads/main"));
    }
}
