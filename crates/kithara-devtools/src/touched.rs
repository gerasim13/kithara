use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::common::project::{TestCargoOptions, TestCommandConfig, TestLaneConfig};

/// One run a touched selection asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Touched {
    /// A lane run whole.
    Whole(String),
    /// The default lane with its own build, running only the tests of
    /// `packages`: the packages of the touched lanes it builds.
    Narrowed {
        lane: String,
        packages: BTreeSet<String>,
    },
}

impl Touched {
    /// The configured lane the run renders its command from.
    pub(crate) fn lane(&self) -> &str {
        match self {
            Self::Whole(lane) | Self::Narrowed { lane, .. } => lane,
        }
    }
}

impl fmt::Display for Touched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Whole(lane) => f.write_str(lane),
            Self::Narrowed { lane, packages } => {
                let packages = packages
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{lane}, narrowed to {packages}")
            }
        }
    }
}

/// The runs of `scope` a branch touches; an empty scope is the default lane.
///
/// The default branch runs the whole scope. A path with no reviewed owner
/// runs the default lane whole, so a narrow run is an opt-in coverage
/// reduction, never the result of failing to classify a changed path.
pub(crate) fn lanes(
    root: &Path,
    test: &TestCommandConfig,
    scope: &[String],
) -> Result<Vec<Touched>> {
    if let Some(unknown) = scope.iter().find(|name| !test.lanes.contains_key(*name)) {
        bail!("test lane `{unknown}` is not configured");
    }
    let scope = &scoped(scope, &test.default_lane);
    let _ = Command::new("git")
        .current_dir(root)
        .args(["fetch", "--no-tags", "--quiet", "origin", "main"])
        .status();
    let base = git(root, &["merge-base", "origin/main", "HEAD"])?;
    if base == git(root, &["rev-parse", "HEAD"])? {
        return Ok(everything(scope));
    }
    let range = format!("{base}...HEAD");
    let changed = git(root, &["diff", "--name-only", &range])?;
    let changed: Vec<&str> = changed.lines().collect();
    Ok(select(
        &test.lanes,
        &test.shared_paths,
        &test.default_lane,
        scope,
        &changed,
    ))
}

/// What the touched paths select from `scope`.
///
/// A lane of the scope runs whole when the branch touched a path it owns. The
/// default lane is the exception: it owns every path no other lane owns, so
/// such a path runs it whole, and otherwise it runs once, with its own build,
/// narrowed to the packages of every touched lane it builds, in the scope or
/// not. That run is what the default branch runs for those packages, and a
/// target directory then holds one build however many lanes a branch touched,
/// so no lane's build replaces another's units that share a name, such as the
/// test dylib's.
///
/// A shared path runs the whole scope: the routing itself moved. A branch that
/// touched none of the scope runs nothing here; the lanes that carry the rest
/// run it.
fn select(
    lanes: &BTreeMap<String, TestLaneConfig>,
    shared: &[String],
    default: &str,
    scope: &[String],
    changed: &[&str],
) -> Vec<Touched> {
    if changed
        .iter()
        .any(|path| shared.iter().any(|shared| shared == path))
    {
        return everything(scope);
    }
    let touched = |lane: &TestLaneConfig| changed.iter().any(|path| owns(lane, path));
    scope
        .iter()
        .filter_map(|name| {
            let lane = lanes.get(name)?;
            if name != default {
                return touched(lane).then(|| Touched::Whole(name.clone()));
            }
            if changed
                .iter()
                .any(|path| !lanes.values().any(|lane| owns(lane, path)))
            {
                return Some(Touched::Whole(name.clone()));
            }
            let packages: BTreeSet<String> = lanes
                .values()
                .filter(|other| touched(other) && builds(&lane.cargo, &other.cargo))
                .flat_map(|other| other.cargo.packages.iter().cloned())
                .collect();
            (!packages.is_empty()).then(|| Touched::Narrowed {
                lane: name.clone(),
                packages,
            })
        })
        .collect()
}

fn owns(lane: &TestLaneConfig, path: &str) -> bool {
    lane.owns.iter().any(|prefix| path.starts_with(prefix))
}

/// Whether `outer` builds every package `inner` selects.
fn builds(outer: &TestCargoOptions, inner: &TestCargoOptions) -> bool {
    !inner.workspace
        && inner.packages.iter().all(|package| {
            if outer.workspace {
                !outer.exclude.contains(package)
            } else {
                outer.packages.contains(package)
            }
        })
}

/// The lanes a run names, or the default lane when it names none.
fn scoped(scope: &[String], default: &str) -> Vec<String> {
    if scope.is_empty() {
        vec![default.to_owned()]
    } else {
        scope.to_vec()
    }
}

/// Every lane of the scope, whole.
fn everything(scope: &[String]) -> Vec<Touched> {
    scope.iter().cloned().map(Touched::Whole).collect()
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(root)
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

    /// The default lane: the workspace without the tool packages.
    const WORKSPACE: &str = "workspace";

    /// A lane as `(name, owns, packages)`.
    type Lane<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

    /// Lanes beside a workspace lane that leaves `xtask` out.
    fn config(owned: &[Lane<'_>]) -> BTreeMap<String, TestLaneConfig> {
        let strings = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect();
        let mut lanes: BTreeMap<String, TestLaneConfig> = owned
            .iter()
            .map(|(name, owns, packages)| {
                let lane = TestLaneConfig {
                    owns: strings(owns),
                    cargo: TestCargoOptions {
                        packages: strings(packages),
                        ..TestCargoOptions::default()
                    },
                    ..TestLaneConfig::default()
                };
                ((*name).to_owned(), lane)
            })
            .collect();
        lanes.insert(
            WORKSPACE.to_owned(),
            TestLaneConfig {
                cargo: TestCargoOptions {
                    workspace: true,
                    exclude: vec!["xtask".to_owned()],
                    ..TestCargoOptions::default()
                },
                ..TestLaneConfig::default()
            },
        );
        lanes
    }

    /// Lanes the workspace builds, a lane it does not, and a lane that tests
    /// a package the workspace builds under a configuration of its own.
    fn product() -> BTreeMap<String, TestLaneConfig> {
        config(&[
            ("host", &["crates/host/"], &["host", "host-tests"]),
            ("play", &["crates/play/"], &["play", "play-tests"]),
            ("net", &["crates/net/"], &["net"]),
            ("net-host", &["crates/net/src/host/"], &["net"]),
            ("tooling", &["xtask/"], &["xtask"]),
        ])
    }

    fn scope(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn whole(name: &str) -> Touched {
        Touched::Whole(name.to_owned())
    }

    fn narrowed(packages: &[&str]) -> Touched {
        Touched::Narrowed {
            lane: WORKSPACE.to_owned(),
            packages: packages
                .iter()
                .map(|package| (*package).to_owned())
                .collect(),
        }
    }

    #[test]
    fn touched_lanes_the_default_lane_builds_narrow_one_default_run() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&[WORKSPACE]),
            &["crates/host/src/lib.rs", "crates/play/src/lib.rs"],
        );

        assert_eq!(
            selected,
            vec![narrowed(&["host", "host-tests", "play", "play-tests"])]
        );
    }

    #[test]
    fn a_lane_the_default_lane_does_not_build_adds_nothing_to_its_run() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&[WORKSPACE]),
            &["crates/host/src/lib.rs", "xtask/src/main.rs"],
        );

        assert_eq!(selected, vec![narrowed(&["host", "host-tests"])]);
    }

    #[test]
    fn a_branch_that_touched_only_lanes_outside_the_default_runs_nothing_of_it() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&[WORKSPACE]),
            &["xtask/src/main.rs"],
        );

        assert!(selected.is_empty(), "{selected:?}");
    }

    #[test]
    fn an_unowned_path_beside_owned_ones_runs_the_default_lane_whole() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&[WORKSPACE]),
            &["crates/host/src/lib.rs", "crates/other/src/lib.rs"],
        );

        assert_eq!(selected, vec![whole(WORKSPACE)]);
    }

    #[test]
    fn an_empty_ownership_catalog_runs_the_default_lane_whole() {
        let selected = select(
            &config(&[]),
            &[],
            WORKSPACE,
            &scope(&[WORKSPACE]),
            &["crates/other/src/lib.rs"],
        );

        assert_eq!(selected, vec![whole(WORKSPACE)]);
    }

    #[test]
    fn a_named_lane_runs_whole_beside_the_narrowed_default_run() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&[WORKSPACE, "net-host", "tooling"]),
            &["crates/net/src/host/client.rs", "xtask/src/main.rs"],
        );

        assert_eq!(
            selected,
            vec![narrowed(&["net"]), whole("net-host"), whole("tooling")]
        );
    }

    #[test]
    fn a_scope_without_the_default_lane_runs_its_touched_lanes_whole() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&["tooling", "net-host", "play"]),
            &[
                "xtask/src/main.rs",
                "crates/net/src/host/client.rs",
                "crates/other/src/lib.rs",
            ],
        );

        assert_eq!(selected, vec![whole("tooling"), whole("net-host")]);
    }

    #[test]
    fn a_scoped_run_that_touched_none_of_its_lanes_runs_nothing() {
        let selected = select(
            &product(),
            &[],
            WORKSPACE,
            &scope(&["tooling"]),
            &["crates/host/src/lib.rs", "crates/other/src/lib.rs"],
        );

        assert!(selected.is_empty(), "{selected:?}");
    }

    #[test]
    fn a_shared_path_runs_the_whole_scope() {
        let shared = [".config/xtask.toml".to_owned()];

        let selected = select(
            &product(),
            &shared,
            WORKSPACE,
            &scope(&["tooling", WORKSPACE]),
            &[".config/xtask.toml"],
        );

        assert_eq!(selected, vec![whole("tooling"), whole(WORKSPACE)]);
    }

    #[test]
    fn a_prefix_matches_a_file_as_well_as_a_directory() {
        let lanes = config(&[(
            "harness",
            &["crates/kithara-platform/tests/flash_"],
            &["xtask"],
        )]);

        let selected = select(
            &lanes,
            &[],
            WORKSPACE,
            &scope(&["harness"]),
            &["crates/kithara-platform/tests/flash_lexical.rs"],
        );

        assert_eq!(selected, vec![whole("harness")]);
    }

    #[test]
    fn a_run_that_names_no_lane_is_scoped_to_the_default_lane() {
        let named = scope(&["tooling"]);

        assert_eq!(scoped(&[], WORKSPACE), scope(&[WORKSPACE]));
        assert_eq!(scoped(&named, WORKSPACE), named);
    }
}
