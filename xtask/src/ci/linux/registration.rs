use std::{fs, os::unix::fs::PermissionsExt, path::Path, process};

use anyhow::{Context, Result, bail};
use reqwest::{
    blocking::Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use tracing::info;

use super::profile::{LinuxHost, LinuxRunner};

const API_VERSION: &str = "X-GitHub-Api-Version";

#[derive(Deserialize)]
struct Registration {
    id: u64,
    name: String,
    status: String,
}

#[derive(Deserialize)]
struct Listing {
    runners: Vec<Registration>,
}

#[derive(Deserialize)]
struct JitConfig {
    encoded_jit_config: String,
}

#[derive(Serialize)]
struct JitRequest<'a> {
    name: String,
    runner_group_id: u32,
    labels: &'a [String],
    work_folder: &'a str,
}

/// Mint one just-in-time configuration and leave it where the runner's service
/// will read it.
///
/// A just-in-time configuration carries its own credentials, is accepted once,
/// and expires with the job it serves. The token that mints it stays on the
/// host: only the generated configuration reaches the container.
pub(super) fn configure(host: &LinuxHost, runner: &LinuxRunner, env_file: &Path) -> Result<()> {
    let token = read_token(&host.token_file)?;
    let client = client(&token)?;
    let endpoint = format!(
        "https://api.github.com/repos/{}/actions/runners",
        host.repository
    );

    prune_offline(&client, &endpoint, &runner.name)?;

    let request = JitRequest {
        // A name is claimed until its runner is removed, and an ephemeral
        // runner is removed only after it has served a job. The process id
        // keeps a restart from colliding with the registration it replaces.
        name: format!("{}-{}", runner.name, process::id()),
        runner_group_id: 1,
        labels: &runner.labels,
        work_folder: "_work",
    };
    let response = client
        .post(format!("{endpoint}/generate-jitconfig"))
        .json(&request)
        .send()
        .context("requesting a just-in-time runner configuration")?;
    if !response.status().is_success() {
        bail!(
            "GitHub refused a runner configuration for {}: {}",
            runner.name,
            response.status()
        );
    }
    let config: JitConfig = response
        .json()
        .context("reading the just-in-time runner configuration")?;

    write_secret(
        env_file,
        &format!("ACTIONS_RUNNER_JITCONFIG={}\n", config.encoded_jit_config),
    )?;
    info!(runner = runner.name, "runner configuration written");
    Ok(())
}

/// Drop registrations left behind by runners that never got a job. An ephemeral
/// runner removes itself once it has served one, but a runner that is restarted
/// or killed first leaves its name registered forever.
fn prune_offline(client: &Client, endpoint: &str, prefix: &str) -> Result<()> {
    let response = client
        .get(endpoint)
        .send()
        .context("listing the repository's runners")?;
    if !response.status().is_success() {
        bail!("GitHub refused to list runners: {}", response.status());
    }
    let listing: Listing = response.json().context("reading the runner listing")?;
    for stale in listing
        .runners
        .iter()
        .filter(|runner| runner.status == "offline" && runner.name.starts_with(prefix))
    {
        let removed = client
            .delete(format!("{endpoint}/{}", stale.id))
            .send()
            .with_context(|| format!("removing the stale runner {}", stale.name))?;
        if !removed.status().is_success() {
            bail!(
                "GitHub refused to remove the stale runner {}: {}",
                stale.name,
                removed.status()
            );
        }
        info!(runner = stale.name, "stale registration removed");
    }
    Ok(())
}

fn client(token: &str) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("kithara-ci"));
    headers.insert(API_VERSION, HeaderValue::from_static("2022-11-28"));
    let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
        .context("the runner token is not a usable header value")?;
    authorization.set_sensitive(true);
    headers.insert(AUTHORIZATION, authorization);
    Client::builder()
        .default_headers(headers)
        .build()
        .context("building the GitHub client")
}

fn read_token(path: &Path) -> Result<String> {
    let token = fs::read_to_string(path)
        .with_context(|| format!("reading the runner token {}", path.display()))?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        bail!("the runner token {} is empty", path.display());
    }
    Ok(token)
}

/// Credentials reach disk unreadable to anyone but their owner, and never pass
/// through a command line where every process on the machine could read them.
fn write_secret(path: &Path, contents: &str) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))
}
