use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use reqwest::Url;
use serde::Deserialize;

use super::{
    model::{regular_file, simple_branch, simple_repository},
    reconcile::Bridge,
};

#[derive(Debug, Args)]
pub(crate) struct BridgeArgs {
    #[command(subcommand)]
    command: BridgeCommand,
}

#[derive(Debug, Subcommand)]
enum BridgeCommand {
    /// Validate configuration and secret file references without network access.
    Validate {
        /// Bridge configuration file.
        #[arg(long, env = "KITHARA_BRIDGE_CONFIG")]
        config: PathBuf,
    },
    /// Reconcile both repository heads once and exit.
    Reconcile {
        /// Bridge configuration file.
        #[arg(long, env = "KITHARA_BRIDGE_CONFIG")]
        config: PathBuf,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BridgeConfig {
    pub(super) github_repo: String,
    pub(super) github_app_id: u64,
    pub(super) github_installation_id: u64,
    pub(super) github_private_key: PathBuf,
    pub(super) gitlab_url: Url,
    pub(super) gitlab_project_id: u64,
    pub(super) gitlab_project_path: String,
    pub(super) gitlab_username: String,
    pub(super) gitlab_token_file: PathBuf,
    pub(super) gitlab_ca_file: PathBuf,
    pub(super) branch: String,
    pub(super) state_dir: PathBuf,
    #[serde(default = "default_pipeline_timeout_seconds")]
    pub(super) pipeline_timeout_seconds: u64,
    #[serde(default = "default_pipeline_poll_seconds")]
    pub(super) pipeline_poll_seconds: u64,
}

impl BridgeConfig {
    fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("reading bridge config {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("parsing bridge config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if !simple_repository(&self.github_repo) {
            bail!("github_repo must use a safe owner/name form");
        }
        if !simple_repository(&self.gitlab_project_path) {
            bail!("gitlab_project_path must use a safe group/name form");
        }
        if self.github_app_id == 0
            || self.github_installation_id == 0
            || self.gitlab_project_id == 0
        {
            bail!("GitHub App, installation, and GitLab project ids must be positive");
        }
        for (label, path) in [
            ("GitHub App private key", &self.github_private_key),
            ("GitLab token", &self.gitlab_token_file),
            ("GitLab CA", &self.gitlab_ca_file),
        ] {
            if !regular_file(path) {
                bail!("{label} not found: {}", path.display());
            }
        }
        if self.gitlab_url.scheme() != "https"
            || self.gitlab_url.host_str().is_none()
            || !self.gitlab_url.username().is_empty()
            || self.gitlab_url.password().is_some()
            || self.gitlab_url.query().is_some()
            || self.gitlab_url.fragment().is_some()
        {
            bail!("gitlab_url must be an HTTPS origin without credentials or query");
        }
        if !simple_branch(&self.branch) {
            bail!("branch must be one simple branch name");
        }
        if self.gitlab_username.is_empty()
            || !self
                .gitlab_username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            bail!("gitlab_username contains unsupported characters");
        }
        if !self.state_dir.is_absolute()
            || self.state_dir.parent().is_none()
            || self.state_dir.parent() == Some(Path::new("/"))
        {
            bail!("state_dir must be an absolute path below a dedicated directory");
        }
        if self.pipeline_timeout_seconds == 0 || self.pipeline_poll_seconds == 0 {
            bail!("pipeline timeout and poll interval must be positive");
        }
        if self.pipeline_poll_seconds >= self.pipeline_timeout_seconds {
            bail!("pipeline poll interval must be shorter than its timeout");
        }
        Ok(())
    }

    pub(super) fn pipeline_timeout(&self) -> Duration {
        Duration::from_secs(self.pipeline_timeout_seconds)
    }

    pub(super) fn pipeline_poll_interval(&self) -> Duration {
        Duration::from_secs(self.pipeline_poll_seconds)
    }

    pub(super) fn gitlab_origin(&self) -> String {
        self.gitlab_url.as_str().trim_end_matches('/').to_string()
    }
}

pub(crate) fn run(args: &BridgeArgs) -> Result<()> {
    match &args.command {
        BridgeCommand::Validate { config } => {
            BridgeConfig::load(config)?;
            Ok(())
        }
        BridgeCommand::Reconcile { config } => {
            Bridge::new(BridgeConfig::load(config)?)?.reconcile_once()
        }
    }
}

fn default_pipeline_timeout_seconds() -> u64 {
    7_200
}

fn default_pipeline_poll_seconds() -> u64 {
    15
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::Cli;

    #[test]
    fn bridge_requires_an_explicit_operation() {
        assert!(Cli::try_parse_from(["xtask", "ci", "bridge"]).is_err());
        assert!(
            Cli::try_parse_from([
                "xtask",
                "ci",
                "bridge",
                "validate",
                "--config",
                "bridge.json"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "xtask",
                "ci",
                "bridge",
                "reconcile",
                "--config",
                "bridge.json"
            ])
            .is_ok()
        );
    }
}
