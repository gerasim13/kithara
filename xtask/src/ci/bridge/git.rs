use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

use super::{
    api::{Github, Gitlab},
    command::BridgeConfig,
};

pub(super) struct GitRepo {
    root: PathBuf,
    config: BridgeConfig,
}

impl GitRepo {
    pub(super) fn new(state_dir: &Path, config: &BridgeConfig) -> Result<Self> {
        let root = state_dir.join("repository.git");
        if !root.exists() {
            fs::create_dir_all(state_dir)
                .with_context(|| format!("creating bridge state {}", state_dir.display()))?;
            let output = Command::new("git")
                .current_dir(state_dir)
                .args(["init", "--bare"])
                .arg(&root)
                .output()
                .context("initializing bridge bare repository")?;
            checked(output, "git init --bare")?;
        }
        Ok(Self {
            root,
            config: config.clone(),
        })
    }

    pub(super) fn fetch(&self, github: &Github, gitlab: &Gitlab) -> Result<()> {
        let github_url = format!("https://github.com/{}.git", self.config.github_repo);
        self.run(
            &[
                "fetch",
                "--no-tags",
                "--force",
                &github_url,
                &format!(
                    "+refs/heads/{}:refs/heads/bridge/github",
                    self.config.branch
                ),
            ],
            Some(github.git_header()?),
        )?;

        let gitlab_url = format!(
            "{}/{}.git",
            self.config.gitlab_origin(),
            self.config.gitlab_project_path
        );
        self.run(
            &[
                "fetch",
                "--no-tags",
                "--force",
                &gitlab_url,
                &format!(
                    "+refs/heads/{}:refs/heads/bridge/gitlab",
                    self.config.branch
                ),
            ],
            Some(gitlab.git_header()),
        )?;
        Ok(())
    }

    pub(super) fn is_ancestor(&self, older: &str, newer: &str) -> Result<bool> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["merge-base", "--is-ancestor", older, newer])
            .output()
            .context("running git merge-base --is-ancestor")?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            code => bail!(
                "git merge-base failed with exit code {}: {}",
                code.unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    pub(super) fn changed_paths(&self, older: &str, newer: &str) -> Result<Vec<String>> {
        let output = Command::new("git")
            .current_dir(&self.root)
            .args(["diff", "--name-only", "-z", &format!("{older}..{newer}")])
            .output()
            .context("running git diff for bridge control paths")?;
        let bytes = checked(output, "git diff --name-only")?;
        bytes
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| {
                String::from_utf8(path.to_vec())
                    .context("Git path was not UTF-8 during bridge validation")
            })
            .collect()
    }

    pub(super) fn push_gitlab(&self, gitlab: &Gitlab, sha: &str, destination: &str) -> Result<()> {
        let url = format!(
            "{}/{}.git",
            self.config.gitlab_origin(),
            self.config.gitlab_project_path
        );
        self.run(
            &["push", &url, &format!("{sha}:refs/heads/{destination}")],
            Some(gitlab.git_header()),
        )?;
        Ok(())
    }

    pub(super) fn push_github(&self, github: &Github, sha: &str) -> Result<()> {
        let url = format!("https://github.com/{}.git", self.config.github_repo);
        self.run(
            &[
                "push",
                &url,
                &format!("{sha}:refs/heads/{}", self.config.branch),
            ],
            Some(github.git_header()?),
        )?;
        Ok(())
    }

    fn run(&self, args: &[&str], header: Option<String>) -> Result<Vec<u8>> {
        let mut command = Command::new("git");
        command.current_dir(&self.root).args(args);
        if let Some(header) = header {
            command
                .env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraHeader")
                .env("GIT_CONFIG_VALUE_0", header);
        }
        let output = command
            .output()
            .with_context(|| format!("running git {}", args.first().unwrap_or(&"<unknown>")))?;
        checked(
            output,
            &format!("git {}", args.first().unwrap_or(&"<unknown>")),
        )
    }
}

fn checked(output: Output, label: &str) -> Result<Vec<u8>> {
    if !output.status.success() {
        bail!(
            "{label} failed with exit code {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}
