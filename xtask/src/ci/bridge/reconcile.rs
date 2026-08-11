use std::{
    fs, thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use tracing::info;

use super::{
    api::{Github, Gitlab},
    command::BridgeConfig,
    git::GitRepo,
    ledger::Ledger,
    model::{Direction, ImportState, changed_control_paths, direction_for, validate_sha},
};

pub(super) struct Bridge {
    config: BridgeConfig,
    ledger: Ledger,
    github: Github,
    gitlab: Gitlab,
    repo: GitRepo,
}

impl Bridge {
    pub(super) fn new(config: BridgeConfig) -> Result<Self> {
        fs::create_dir_all(&config.state_dir)
            .with_context(|| format!("creating bridge state {}", config.state_dir.display()))?;
        Ok(Self {
            ledger: Ledger::new(&config.state_dir)?,
            github: Github::new(&config)?,
            gitlab: Gitlab::new(&config)?,
            repo: GitRepo::new(&config.state_dir, &config)?,
            config,
        })
    }

    pub(super) fn reconcile_once(&self) -> Result<()> {
        self.repo.fetch(&self.github, &self.gitlab)?;
        let github_sha = self.github.head()?;
        let gitlab_sha = self.gitlab.head()?;
        require_sha("GitHub", &github_sha)?;
        require_sha("GitLab", &gitlab_sha)?;
        let direction = direction_for(&github_sha, &gitlab_sha, |older, newer| {
            self.repo.is_ancestor(older, newer)
        })?;
        info!(?direction, %github_sha, %gitlab_sha, "repository direction observed");

        match direction {
            Direction::Equal => Ok(()),
            Direction::GitlabAhead => self.export_gitlab(&gitlab_sha),
            Direction::GithubAhead => self.import_github(&github_sha, &gitlab_sha),
            Direction::Diverged => {
                let detail = format!(
                    "GitHub `{github_sha}` and GitLab `{gitlab_sha}` are not ancestors of each \
                     other. Synchronization stopped."
                );
                self.gitlab
                    .ensure_issue("GitHub and GitLab main branches diverged", &detail)?;
                bail!("GitHub and GitLab histories diverged")
            }
        }
    }

    fn export_gitlab(&self, gitlab_sha: &str) -> Result<()> {
        let status = self
            .gitlab
            .latest_push_pipeline_status(gitlab_sha)?
            .context("GitLab main commit has no push pipeline yet")?;
        if status != "success" {
            bail!("GitLab main pipeline for {gitlab_sha} is {status}; GitHub export is blocked");
        }
        self.repo.push_github(&self.github, gitlab_sha)
    }

    fn import_github(&self, github_sha: &str, gitlab_base_sha: &str) -> Result<()> {
        let entry = self.ledger.get(github_sha, gitlab_base_sha)?;
        if entry
            .as_ref()
            .is_some_and(|entry| entry.state == ImportState::Promoted)
        {
            return Ok(());
        }
        if let Some(entry) = entry
            .as_ref()
            .filter(|entry| entry.state == ImportState::Rejected)
        {
            bail!(
                "{}",
                entry
                    .detail
                    .as_deref()
                    .unwrap_or("GitHub import was rejected")
            );
        }

        let Some(pull_number) = self.github.merged_pull_request(github_sha)? else {
            let detail = format!(
                "GitHub head {github_sha} is not associated with a merged pull request targeting {}",
                self.config.branch
            );
            self.ledger.put(
                github_sha,
                gitlab_base_sha,
                ImportState::Rejected,
                None,
                Some(detail.clone()),
            )?;
            self.gitlab
                .ensure_issue("Untrusted direct GitHub main update", &detail)?;
            bail!("{detail}");
        };

        self.github
            .report_status(github_sha, "pending", "GitLab verification running")?;
        let control_paths =
            changed_control_paths(self.repo.changed_paths(gitlab_base_sha, github_sha)?);
        if !control_paths.is_empty() {
            let paths = control_paths
                .iter()
                .map(|path| format!("- `{path}`"))
                .collect::<Vec<_>>()
                .join("\n");
            let detail = format!(
                "The merged GitHub pull request changes CI control files. Import stopped; port \
                 these changes through a reviewed GitLab merge request instead:\n\n{paths}"
            );
            self.reject(github_sha, gitlab_base_sha, None, "failure", &detail)?;
            self.gitlab
                .ensure_issue("GitHub import requires CI control review", &detail)?;
            bail!("{detail}");
        }

        let pipeline_id = if let Some(id) = entry.as_ref().and_then(|entry| entry.pipeline_id) {
            id
        } else {
            let quarantine = format!("quarantine/github/{github_sha}");
            self.repo
                .push_gitlab(&self.gitlab, github_sha, &quarantine)?;
            let id = self.gitlab.create_pipeline(&quarantine, github_sha)?;
            self.ledger.put(
                github_sha,
                gitlab_base_sha,
                ImportState::Testing,
                Some(id),
                Some(format!("GitHub PR #{pull_number}")),
            )?;
            id
        };

        let status = self.wait_for_pipeline(pipeline_id)?;
        if status != "success" {
            let detail = format!("GitLab quarantine pipeline {pipeline_id} finished with {status}");
            self.reject(
                github_sha,
                gitlab_base_sha,
                Some(pipeline_id),
                "failure",
                &detail,
            )?;
            bail!("{detail}");
        }

        let current_github = self.github.head()?;
        let current_gitlab = self.gitlab.head()?;
        if current_github != github_sha || current_gitlab != gitlab_base_sha {
            let detail = "repository heads changed during quarantine verification; promotion was \
                          not attempted";
            self.reject(
                github_sha,
                gitlab_base_sha,
                Some(pipeline_id),
                "error",
                detail,
            )?;
            bail!("{detail}");
        }

        self.repo
            .push_gitlab(&self.gitlab, github_sha, &self.config.branch)?;
        self.ledger.put(
            github_sha,
            gitlab_base_sha,
            ImportState::Promoted,
            Some(pipeline_id),
            Some(format!("promoted from GitHub PR #{pull_number}")),
        )?;
        self.github.report_status(
            github_sha,
            "success",
            &format!("GitLab pipeline {pipeline_id} passed; commit promoted."),
        )
    }

    fn reject(
        &self,
        github_sha: &str,
        gitlab_base_sha: &str,
        pipeline_id: Option<u64>,
        state: &str,
        detail: &str,
    ) -> Result<()> {
        self.ledger.put(
            github_sha,
            gitlab_base_sha,
            ImportState::Rejected,
            pipeline_id,
            Some(detail.to_string()),
        )?;
        self.github.report_status(github_sha, state, detail)
    }

    fn wait_for_pipeline(&self, pipeline_id: u64) -> Result<String> {
        wait_for_status(
            self.config.pipeline_timeout(),
            self.config.pipeline_poll_interval(),
            || self.gitlab.pipeline_status(pipeline_id),
        )
    }
}

fn wait_for_status(
    timeout: Duration,
    poll_interval: Duration,
    mut status: impl FnMut() -> Result<String>,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let current = status()?;
        if matches!(
            current.as_str(),
            "success" | "failed" | "canceled" | "skipped" | "manual"
        ) {
            return Ok(current);
        }
        thread::sleep(poll_interval);
    }
    Ok("timeout".into())
}

fn require_sha(owner: &str, sha: &str) -> Result<()> {
    if !validate_sha(sha) {
        bail!("{owner} returned an invalid commit SHA: {sha:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_pipeline_status_returns_without_an_extra_poll() {
        let mut calls = 0;
        let status = wait_for_status(Duration::from_secs(1), Duration::from_millis(1), || {
            calls += 1;
            Ok("success".into())
        })
        .unwrap();
        assert_eq!(status, "success");
        assert_eq!(calls, 1);
    }
}
