use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use clap::{Args, ValueEnum};
use kithara_devtools::Ctx;
use serde::Serialize;

use crate::{
    ci::run::PipelineKind,
    config::{CiLaneArtifact, CiLaneConfig, KitharaExt},
};

/// Which CI system is asking. A lane may answer them differently, because the
/// fleets are different machines with different budgets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub(crate) enum Fleet {
    #[default]
    Github,
    Gitlab,
}

#[derive(Debug, Args)]
pub(crate) struct LanesArgs {
    /// The role whose lanes to render.
    #[arg(long)]
    pub(crate) role: String,
    #[arg(long, value_enum)]
    pub(crate) kind: PipelineKind,
    /// Render only these lanes, whatever their membership says. This is how a
    /// single subtask is run on its own.
    #[arg(long, value_delimiter = ' ')]
    pub(crate) only: Vec<String>,
    #[arg(long, value_enum, default_value_t = Fleet::Github)]
    pub(crate) fleet: Fleet,
    /// Print one member of the selection rather than the whole object. The CI
    /// image has no `jq`, and a workflow that parses JSON in shell is logic in
    /// YAML.
    #[arg(long, value_enum)]
    pub(crate) field: Option<Field>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Field {
    Matrix,
    Dependent,
}

#[derive(Debug, Serialize)]
pub(crate) struct Entry {
    pub(crate) lane: String,
    pub(crate) timeout: u32,
    pub(crate) depth: u32,
    pub(crate) artifact: Option<CiLaneArtifact>,
    pub(crate) queue: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Dependent {
    pub(crate) lane: String,
    pub(crate) timeout: u32,
    pub(crate) depth: u32,
    pub(crate) needs: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Selection {
    pub(crate) matrix: Vec<Entry>,
    pub(crate) dependent: Vec<Dependent>,
}

/// The kinds this fleet reads: a lane's own answer where it gives one, the
/// shared answer otherwise.
fn membership(lane: &CiLaneConfig, fleet: Fleet) -> &[String] {
    if fleet == Fleet::Github && !lane.kinds_github.is_empty() {
        return &lane.kinds_github;
    }
    &lane.kinds
}

/// A lane with the asked-for role that this pipeline kind schedules, or that
/// `--only` named directly. A lane with `needs` still has to pass this before
/// it can land in `matrix`; `dependent` below never calls it, because a
/// consumer's own membership is not what admits it.
fn is_asked_for(lane: &CiLaneConfig, name: &str, kind: &str, args: &LanesArgs) -> bool {
    if lane.role != args.role {
        return false;
    }
    if args.only.is_empty() {
        membership(lane, args.fleet)
            .iter()
            .any(|entry| entry == kind)
    } else {
        args.only.iter().any(|only| only == name)
    }
}

pub(crate) fn render(
    lanes: &BTreeMap<String, CiLaneConfig>,
    args: &LanesArgs,
) -> Result<Selection> {
    for name in &args.only {
        if !lanes.contains_key(name) {
            bail!(
                "`{name}` is not a CI lane; this repository has {}",
                lanes.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
    let kind = args.kind.name();

    let matrix: Vec<Entry> = lanes
        .iter()
        .filter(|(name, lane)| lane.needs.is_empty() && is_asked_for(lane, name, kind, args))
        .map(|(name, lane)| Entry {
            lane: name.clone(),
            timeout: lane.timeout_minutes,
            depth: lane.fetch_depth,
            artifact: lane.artifact.clone(),
            queue: lane.queue.clone(),
        })
        .collect();

    // A lane with `needs` never earns its own place through `--role`/`--kind`
    // or `--only`: it rides in only once a lane it reads from already made the
    // matrix above. Asking for one producer therefore brings its consumer with
    // it, and asking for an unrelated lane does not.
    let present: BTreeSet<&str> = matrix.iter().map(|entry| entry.lane.as_str()).collect();
    let dependent: Vec<Dependent> = lanes
        .iter()
        .filter(|(_, lane)| lane.role == args.role && !lane.needs.is_empty())
        .filter(|(_, lane)| {
            lane.needs
                .iter()
                .any(|need| present.contains(need.as_str()))
        })
        .map(|(name, lane)| Dependent {
            lane: name.clone(),
            timeout: lane.timeout_minutes,
            depth: lane.fetch_depth,
            needs: lane.needs.clone(),
        })
        .collect();

    Ok(Selection { matrix, dependent })
}

pub(crate) fn run(args: &LanesArgs, ctx: &Ctx) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let selection = render(&ext.ci.lanes, args)?;
    let rendered = match args.field {
        Some(Field::Matrix) => serde_json::to_string(&selection.matrix)?,
        Some(Field::Dependent) => serde_json::to_string(&selection.dependent)?,
        None => serde_json::to_string(&selection)?,
    };
    println!("{rendered}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(role: &str, kinds: &[&str], needs: &[&str]) -> CiLaneConfig {
        CiLaneConfig {
            role: role.to_owned(),
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            needs: needs.iter().map(|need| (*need).to_owned()).collect(),
            timeout_minutes: 30,
            cache_group: "linux".to_owned(),
            program: "just".to_owned(),
            ..CiLaneConfig::default()
        }
    }

    fn catalog() -> BTreeMap<String, CiLaneConfig> {
        BTreeMap::from([
            ("linux-lint".to_owned(), lane("gate", &["main"], &[])),
            ("deep-stress".to_owned(), lane("deep", &["nightly"], &[])),
            (
                "deep-stress-report".to_owned(),
                lane("deep", &["nightly"], &["deep-stress"]),
            ),
            ("deep-miri".to_owned(), lane("deep", &["weekly"], &[])),
        ])
    }

    fn args(role: &str, kind: PipelineKind, only: &[&str]) -> LanesArgs {
        LanesArgs {
            role: role.to_owned(),
            kind,
            only: only.iter().map(|name| (*name).to_owned()).collect(),
            fleet: Fleet::Github,
            field: None,
        }
    }

    #[test]
    fn a_role_and_kind_select_the_lanes_that_declare_both() {
        let selection = render(&catalog(), &args("gate", PipelineKind::Main, &[]))
            .expect("the gate role renders");
        let names: Vec<&str> = selection
            .matrix
            .iter()
            .map(|entry| entry.lane.as_str())
            .collect();
        assert_eq!(names, ["linux-lint"]);
        assert!(selection.dependent.is_empty());
    }

    // Asking for one lane must bring what reads its artifact, and nothing else.
    #[test]
    fn selecting_one_lane_brings_only_its_own_dependents() {
        let selection = render(
            &catalog(),
            &args("deep", PipelineKind::Nightly, &["deep-stress"]),
        )
        .expect("the stress lane renders");
        let names: Vec<&str> = selection
            .matrix
            .iter()
            .map(|entry| entry.lane.as_str())
            .collect();
        let dependent: Vec<&str> = selection
            .dependent
            .iter()
            .map(|entry| entry.lane.as_str())
            .collect();
        assert_eq!(names, ["deep-stress"]);
        assert_eq!(dependent, ["deep-stress-report"]);
    }

    #[test]
    fn a_lane_whose_needs_were_not_selected_is_left_out() {
        let selection = render(&catalog(), &args("deep", PipelineKind::Weekly, &[]))
            .expect("the weekly deep role renders");
        let names: Vec<&str> = selection
            .matrix
            .iter()
            .map(|entry| entry.lane.as_str())
            .collect();
        assert_eq!(names, ["deep-miri"]);
        assert!(selection.dependent.is_empty());
    }

    #[test]
    fn an_only_that_names_no_lane_fails_with_the_lanes_it_could_have_been() {
        let error = render(
            &catalog(),
            &args("deep", PipelineKind::Nightly, &["deep-strss"]),
        )
        .expect_err("a misspelled lane is refused");
        assert!(
            error.to_string().contains("deep-stress"),
            "the error must list the lanes: {error}"
        );
    }

    // The two fleets buy different amounts of CI. Where a lane says so, the
    // fleet asking is the one answered.
    #[test]
    fn a_field_prints_that_member_alone() {
        let selection = render(&catalog(), &args("gate", PipelineKind::Main, &[]))
            .expect("the gate role renders");
        let printed = serde_json::to_string(&selection.matrix).expect("the matrix serialises");
        assert!(
            printed.starts_with('['),
            "a field prints a bare array: {printed}"
        );
    }

    #[test]
    fn the_github_fleet_reads_its_own_membership_where_a_lane_declares_one() {
        let mut lanes = catalog();
        let deny = lanes
            .get_mut("linux-lint")
            .expect("the lint lane is in the catalog");
        deny.kinds = vec!["weekly".to_owned()];
        deny.kinds_github = vec!["main".to_owned()];

        let github = render(&lanes, &args("gate", PipelineKind::Main, &[]))
            .expect("the GitHub fleet renders");
        assert_eq!(github.matrix.len(), 1);

        let mut gitlab = args("gate", PipelineKind::Main, &[]);
        gitlab.fleet = Fleet::Gitlab;
        let gitlab = render(&lanes, &gitlab).expect("the GitLab fleet renders");
        assert!(gitlab.matrix.is_empty());
    }
}
