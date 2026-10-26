use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    NextestAction,
    command::plan,
    inventory::{compare_expected, inventory_args, list, read_expected},
    prepared_args,
    request::TestRequest,
    resolve::{NextestOperation, ResolvedLane},
};
use crate::common::project::{ProjectConfig, TestRunner};

/// One canonical lane selection, recorded by a producer and compared by its consumer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NextestPlan {
    lane: ResolvedLane,
    caller: Vec<String>,
    expected_tests: Option<PathBuf>,
}

/// Authenticated delivery paths supplied by the artifact owner. Content identities,
/// toolchain compatibility and producer provenance must be checked by that owner first.
#[derive(Debug)]
pub struct PreparedNextest<'a> {
    /// Verified immutable nextest archive.
    pub archive_file: &'a Path,
    /// Consumer workspace containing the verified runtime data.
    pub workspace_remap: &'a Path,
    /// Consumer-owned archive extraction destination.
    pub extract_to: &'a Path,
    /// Verified producer inventory in exact identity TSV format.
    pub expected_tests: &'a Path,
    /// Required fixture, model and runtime library paths, verified by the caller.
    pub runtime_inputs: &'a [&'a Path],
    /// Consumer runtime and pinned-tool environment; configured lane values cannot change.
    pub environment: &'a BTreeMap<String, String>,
}

/// Resolve full or touched requests through the ordinary test selection owner.
/// An empty vector is an explicit canonical empty branch selection.
///
/// # Errors
/// Refuses invalid requests, non-nextest lanes and unsupported prepared options.
pub fn nextest_plans(
    root: &Path,
    project: &ProjectConfig,
    args: &[String],
    base: &str,
) -> Result<Vec<NextestPlan>> {
    let request = TestRequest::parse(args)?;
    inventory_args(&request.passthrough)?;
    prepared_args::split(&request.passthrough)?;
    ensure!(
        !request.passthrough.iter().any(|arg| arg == "--no-run"),
        "prepared tests require execution, not --no-run"
    );
    let selection = plan(&project.test, root, &request, base)?;
    if let Some(output) = &request.selection_output {
        selection.write_selection(&root.join(output))?;
    }
    let lanes = selection.resolved(&project.test, &request)?;
    lanes
        .into_iter()
        .map(|lane| {
            ensure!(
                matches!(&lane.runner, TestRunner::Nextest(_)),
                "test lane `{}` has no nextest archive",
                lane.lane
            );
            Ok(NextestPlan {
                lane,
                caller: request.passthrough.clone(),
                expected_tests: request.expected_tests.clone(),
            })
        })
        .collect()
}

impl NextestPlan {
    /// The configured lane represented by this selection.
    #[must_use]
    pub fn lane(&self) -> &str {
        &self.lane.lane
    }

    /// Build and archive the canonical Cargo selection. Test filters and run policy
    /// are applied only when the archive is inventoried or executed.
    /// The selected nextest profile supplies its archive includes during preparation.
    ///
    /// # Errors
    /// Refuses invalid resolved inputs and unsupported archive arguments.
    pub fn archive_command(&self, archive_file: &Path) -> Result<Command> {
        self.lane.validate()?;
        self.lane
            .operation(NextestOperation::Archive(archive_file), &self.caller)
    }

    /// List this archive's selected non-ignored identities as an exact UTF-8 TSV.
    /// Nextest extracts to its own temporary directory and never compiles. Listing
    /// executes archived binaries: authenticate the producer and payload first.
    ///
    /// # Errors
    /// Refuses missing inputs, failed/corrupt inventory, empty selected lanes and
    /// mismatches with the request's optional expected-test ledger. Conflicting
    /// delivery environment values are refused before listing.
    pub fn archive_inventory(
        &self,
        root: &Path,
        producer: &Self,
        archive_file: &Path,
        workspace_remap: &Path,
        environment: &BTreeMap<String, String>,
    ) -> Result<String> {
        ensure!(
            self == producer,
            "prepared nextest plan differs from the current selection"
        );
        self.validate_environment(environment)?;
        let mut command = self.inventory_command(archive_file, workspace_remap)?;
        command.envs(environment);
        let expected = self
            .expected_tests
            .as_ref()
            .map(|path| read_expected(&root.join(path)))
            .transpose()?;
        let selected = list(&mut command, root, self.lane())?;
        ensure!(
            !selected.is_empty(),
            "prepared lane `{}` selects no tests",
            self.lane()
        );
        if let (Some(expected), Some(path)) = (expected, &self.expected_tests) {
            compare_expected(&expected, &selected, path)?;
        }
        Ok(selected
            .iter()
            .map(|(binary, name)| format!("{binary}\t{name}\n"))
            .collect())
    }

    /// Admit an authenticated archive against the current plan and exact inventory,
    /// then return its compile-free run command. Paths must be absolute; extraction
    /// is into the consumer's own area, never the producer's mutable target.
    ///
    /// # Errors
    /// Refuses different plans, conflicting delivery environment values, missing
    /// runtime inputs and inventory mismatches before returning an execution command.
    pub fn prepared_command(
        &self,
        root: &Path,
        producer: &Self,
        input: &PreparedNextest<'_>,
    ) -> Result<Command> {
        ensure!(
            self == producer,
            "prepared nextest plan differs from the current selection"
        );
        self.validate_environment(input.environment)?;
        ensure!(
            input.expected_tests.is_absolute(),
            "prepared expected-tests path must be absolute"
        );
        ensure!(
            input.extract_to.is_absolute(),
            "prepared extraction path must be absolute"
        );
        for path in input.runtime_inputs {
            ensure!(
                path.is_absolute() && path.exists(),
                "missing prepared runtime input {}",
                path.display()
            );
        }
        let expected = read_expected(input.expected_tests)?;
        if let Some(path) = &self.expected_tests {
            compare_expected(&read_expected(&root.join(path))?, &expected, path)?;
        }
        let mut command = self.inventory_command(input.archive_file, input.workspace_remap)?;
        command.envs(input.environment);
        compare_expected(
            &expected,
            &list(&mut command, root, self.lane())?,
            input.expected_tests,
        )?;
        let mut command = self.lane.operation(
            NextestOperation::Prepared {
                action: NextestAction::Run,
                archive: input.archive_file,
                workspace: input.workspace_remap,
                extract_to: Some(input.extract_to),
            },
            &self.caller,
        )?;
        command.current_dir(root).envs(input.environment);
        Ok(command)
    }

    /// Command's map supplies platform key equivalence, including Windows Unicode aliases.
    fn validate_environment(&self, environment: &BTreeMap<String, String>) -> Result<()> {
        for (key, configured) in &self.lane.env {
            for (name, delivered) in environment {
                let mut aliases = Command::new("cargo-nextest");
                aliases.env(key, configured).env(name, delivered);
                ensure!(
                    aliases.get_envs().count() == 2 || delivered == configured,
                    "prepared environment conflicts with configured lane value `{key}`"
                );
            }
        }
        Ok(())
    }

    fn inventory_command(&self, archive: &Path, workspace: &Path) -> Result<Command> {
        self.lane.validate()?;
        ensure!(
            archive.is_absolute()
                && fs::metadata(archive)
                    .with_context(|| format!("read prepared archive {}", archive.display()))?
                    .is_file(),
            "prepared archive must be an absolute file path"
        );
        ensure!(
            workspace.is_absolute() && workspace.is_dir(),
            "prepared workspace remap must name an existing absolute directory"
        );
        self.lane.operation(
            NextestOperation::Prepared {
                action: NextestAction::List,
                archive,
                workspace,
                extract_to: None,
            },
            &inventory_args(&self.caller)?,
        )
    }
}
