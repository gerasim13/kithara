use std::env;

use anyhow::{Context, Result, bail};
use clap::Args;
use reqwest::blocking::Client;
use serde::{Deserialize, de::DeserializeOwned};

use super::github::client;
use crate::consts::GITHUB_PAGE_SIZE as PAGE_SIZE;

#[derive(Debug, Args)]
pub(crate) struct BaselineArgs {
    /// Repository the workflow runs in, as `owner/name`.
    #[arg(long)]
    repository: String,
    /// The calling workflow's `github.workflow_ref`; only runs of that
    /// workflow are searched.
    #[arg(long)]
    workflow_ref: String,
    /// Lane whose last successful run is wanted.
    #[arg(long)]
    lane: String,
}

#[derive(Deserialize)]
struct Repository {
    default_branch: String,
}

#[derive(Deserialize)]
struct Runs {
    workflow_runs: Vec<Run>,
}

#[derive(Deserialize)]
struct Run {
    id: u64,
}

#[derive(Deserialize)]
struct Jobs {
    jobs: Vec<Job>,
}

#[derive(Deserialize)]
struct Job {
    name: String,
    conclusion: Option<String>,
    run_attempt: u64,
}

/// Prints, as workflow outputs, the run and attempt in which the lane last
/// succeeded on the default branch. That attempt's artifact is the one the
/// lane compares against. A lane that never succeeded there has nothing to
/// compare against, and the lookup fails rather than letting it pass unjudged.
pub(crate) fn run(args: &BaselineArgs) -> Result<()> {
    let token = env::var("GH_TOKEN").context("GH_TOKEN must hold a token that reads Actions")?;
    let client = client(&token)?;
    let api = format!("https://api.github.com/repos/{}", args.repository);
    let workflow = workflow_file(&args.workflow_ref)?;
    let branch = get::<Repository>(&client, &api)?.default_branch;
    for page in 1.. {
        let runs = get::<Runs>(
            &client,
            &format!(
                "{api}/actions/workflows/{workflow}/runs?branch={branch}&status=completed\
                 &per_page={PAGE_SIZE}&page={page}"
            ),
        )?
        .workflow_runs;
        for run in &runs {
            if let Some(attempt) = succeeded(&jobs(&client, &api, run.id)?, &args.lane) {
                println!("run-id={}", run.id);
                println!("attempt={attempt}");
                return Ok(());
            }
        }
        if runs.len() < PAGE_SIZE {
            break;
        }
    }
    bail!(
        "no run of {workflow} on {branch} has a successful `{}`",
        args.lane
    )
}

/// The workflow file a `github.workflow_ref` names:
/// `owner/name/.github/workflows/<file>@<ref>`.
fn workflow_file(workflow_ref: &str) -> Result<&str> {
    workflow_ref
        .split_once('@')
        .and_then(|(path, _)| path.rsplit_once('/'))
        .map(|(_, file)| file)
        .filter(|file| !file.is_empty())
        .with_context(|| format!("`{workflow_ref}` is not a workflow reference"))
}

fn jobs(client: &Client, api: &str, run: u64) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    for page in 1.. {
        let listed = get::<Jobs>(
            client,
            &format!(
                "{api}/actions/runs/{run}/jobs?filter=latest&per_page={PAGE_SIZE}&page={page}"
            ),
        )?
        .jobs;
        let last = listed.len() < PAGE_SIZE;
        jobs.extend(listed);
        if last {
            break;
        }
    }
    Ok(jobs)
}

/// The attempt in which the lane's job succeeded. A lane runs through a called
/// workflow, so GitHub names its job after every caller on the way down, and
/// the lane's own name comes last.
fn succeeded(jobs: &[Job], lane: &str) -> Option<u64> {
    jobs.iter()
        .find(|job| {
            job.name.rsplit(" / ").next() == Some(lane)
                && job.conclusion.as_deref() == Some("success")
        })
        .map(|job| job.run_attempt)
}

fn get<T: DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("GitHub answered GET {url} with {status}");
    }
    response
        .json()
        .with_context(|| format!("reading GET {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(name: &str, conclusion: Option<&str>, run_attempt: u64) -> Job {
        Job {
            name: name.to_owned(),
            conclusion: conclusion.map(str::to_owned),
            run_attempt,
        }
    }

    #[test]
    fn the_lane_is_found_by_the_last_segment_of_a_nested_job_name() {
        let jobs = [
            job("quality / Select quality lanes", Some("success"), 1),
            job(
                "quality / lane (quality-coverage-risk-extra, 240) / quality-coverage-risk-extra",
                Some("success"),
                1,
            ),
            job(
                "quality / lane (quality-coverage-risk, 240) / quality-coverage-risk",
                Some("success"),
                2,
            ),
        ];

        assert_eq!(succeeded(&jobs, "quality-coverage-risk"), Some(2));
    }

    #[test]
    fn a_lane_that_did_not_succeed_in_a_run_is_not_its_baseline() {
        let jobs = [
            job("quality / lane (a) / a", Some("failure"), 1),
            job("quality / lane (b) / b", None, 1),
        ];

        assert_eq!(succeeded(&jobs, "a"), None);
        assert_eq!(succeeded(&jobs, "b"), None);
    }

    #[test]
    fn the_workflow_file_is_read_from_the_workflow_ref() {
        assert_eq!(
            workflow_file("gerasim13/kithara/.github/workflows/dispatch.yml@refs/heads/main")
                .expect("a workflow reference"),
            "dispatch.yml"
        );
        assert!(workflow_file("dispatch.yml").is_err());
    }
}
