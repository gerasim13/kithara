use std::{fmt::Write as _, fs, path::Path, process};

use anyhow::{Context, Result, bail};
use reqwest::{
    blocking::Client,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT},
};
use serde::{Deserialize, Serialize};
use tracing::info;

use super::profile::{LinuxHost, LinuxRunner, WindowsGuest};
use crate::{ci::cache::client_environment, consts};

#[derive(Deserialize)]
struct Registration {
    id: u64,
    name: String,
    status: String,
    busy: bool,
}

#[derive(Deserialize)]
struct Listing {
    runners: Vec<Registration>,
}

#[derive(Deserialize)]
struct JitConfig {
    encoded_jit_config: String,
}

#[derive(Deserialize)]
struct RegistrationToken {
    token: String,
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
    let config = mint_jit(host, &runner.repository, &runner.name, &runner.labels)?;
    write_secret(env_file, &runtime_environment(runner, &config)?)?;
    info!(runner = runner.name, "runner configuration written");
    Ok(())
}

/// Give the native listener a one-job configuration through the runner's
/// secret input environment. The runner masks and removes this input before
/// starting a job, and no credential reaches the process arguments.
pub(super) fn configure_image_runner(host: &LinuxHost, env_file: &Path) -> Result<()> {
    let runner = host
        .image_runner
        .as_ref()
        .context("this machine's profile defines no native image runner")?;
    let config = mint_jit(host, &runner.repository, &runner.name, &runner.labels)?;
    write_image_configuration(env_file, &config)?;
    info!(runner = runner.name, "image runner configuration written");
    Ok(())
}

fn write_image_configuration(env_file: &Path, config: &str) -> Result<()> {
    write_secret(
        env_file,
        &format!("ACTIONS_RUNNER_INPUT_JITCONFIG={config}\n"),
    )
}

fn mint_jit(host: &LinuxHost, repository: &str, name: &str, labels: &[String]) -> Result<String> {
    let credential = host.credential(repository)?;
    let token = read_token(&credential.token_file)?;
    let client = client(&token)?;
    let endpoint = format!(
        "https://api.github.com/repos/{}/actions/runners",
        credential.name
    );

    prune_offline(&client, &endpoint, name)?;

    let request = JitRequest {
        // A name is claimed until its runner is removed, and an ephemeral
        // runner is removed only after it has served a job. The process id
        // keeps a restart from colliding with the registration it replaces.
        name: format!("{name}-{}", process::id()),
        runner_group_id: 1,
        labels,
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
            name,
            response.status()
        );
    }
    let config: JitConfig = response
        .json()
        .context("reading the just-in-time runner configuration")?;

    Ok(config.encoded_jit_config)
}

/// Everything a runner's container starts with that is not the same for every
/// runner: its registration, its trust scope, and its scope's store.
///
/// The store's host file is read and completed here, on every start, rather
/// than handed to docker as it is. A host file predates any key added after it
/// was written, and the defaults for those keys live in this binary, so a
/// container started from the raw file would miss them. Compose starts the
/// same container from this file, so it gets the same store and scope.
fn runtime_environment(runner: &LinuxRunner, jit_config: &str) -> Result<String> {
    let mut contents = format!(
        "ACTIONS_RUNNER_JITCONFIG={jit_config}\nKITHARA_CACHE_TRUST={}\n",
        runner.cache_trust.as_str()
    );
    let store = client_environment(&runner.sccache_s3_env_file).with_context(|| {
        format!(
            "reading the S3 cache environment for Linux runner {}",
            runner.name
        )
    })?;
    for (name, value) in store {
        writeln!(contents, "{name}={value}")?;
    }
    Ok(contents)
}

/// Mint the token a machine registers itself with, once.
///
/// A container is handed a configuration that serves one job, because a
/// container is built again for the next one. A guest is installed once and
/// kept, so it enrols the way a physical machine does: it registers itself and
/// holds that registration across the jobs and the restarts that follow. The
/// token minted here is what it registers with, and it expires within the hour
/// whether or not it was used.
pub(super) fn enrolment_token(host: &LinuxHost, guest: &WindowsGuest) -> Result<String> {
    let credential = host.credential(&guest.repository)?;
    let token = read_token(&credential.token_file)?;
    let client = client(&token)?;
    let response = client
        .post(format!(
            "https://api.github.com/repos/{}/actions/runners/registration-token",
            credential.name
        ))
        .send()
        .context("requesting a runner registration token")?;
    if !response.status().is_success() {
        bail!("GitHub refused a registration token: {}", response.status());
    }
    let minted: RegistrationToken = response
        .json()
        .context("reading the runner registration token")?;
    Ok(minted.token)
}

/// Whether a runner of this name is registered and waiting for work. This is
/// the only answer that distinguishes a guest that enrolled from one that
/// booted and did nothing.
pub(super) fn is_online(host: &LinuxHost, guest: &WindowsGuest) -> Result<bool> {
    Ok(registered(host, guest)?.is_some_and(|runner| runner.status == "online"))
}

/// Whether the guest is running a job right now. One that is not registered,
/// or not connected, runs nothing.
pub(super) fn is_busy(host: &LinuxHost, guest: &WindowsGuest) -> Result<bool> {
    Ok(registered(host, guest)?.is_some_and(|runner| runner.busy))
}

/// What GitHub holds under the guest's name, if anything. Asked by name: the
/// listing comes a page at a time, and the fleet's ephemeral runners alone can
/// fill the first one.
fn registered(host: &LinuxHost, guest: &WindowsGuest) -> Result<Option<Registration>> {
    let credential = host.credential(&guest.repository)?;
    let token = read_token(&credential.token_file)?;
    let client = client(&token)?;
    let response = client
        .get(format!(
            "https://api.github.com/repos/{}/actions/runners?name={}",
            credential.name, guest.name
        ))
        .send()
        .context("listing the repository's runners")?;
    if !response.status().is_success() {
        bail!("GitHub refused to list runners: {}", response.status());
    }
    let listing: Listing = response.json().context("reading the runner listing")?;
    Ok(listing
        .runners
        .into_iter()
        .find(|runner| runner.name == guest.name))
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
        .filter(|runner| is_prunable(runner, prefix))
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

fn is_prunable(runner: &Registration, prefix: &str) -> bool {
    runner.status == "offline"
        && !runner.busy
        && runner
            .name
            .strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_prefix('-'))
            .is_some_and(|pid| pid.parse::<u32>().is_ok())
}

fn client(token: &str) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static("kithara-ci"));
    headers.insert(consts::API_VERSION, HeaderValue::from_static("2022-11-28"));
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
    super::permissions::set_mode(path, consts::OWNER_ONLY)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::{Registration, is_prunable, runtime_environment, write_image_configuration};
    use crate::{
        ci::{environment::CacheTrust, host::linux::profile::tests::host_fixture},
        consts,
    };

    /// The shape every Linux host file had: written before the prefix existed.
    fn host_store(directory: &std::path::Path) -> std::path::PathBuf {
        let path = directory.join("cache.env");
        std::fs::write(
            &path,
            "SCCACHE_BUCKET=kithara-review\nSCCACHE_ENDPOINT=http://kithara-ci-cache:9000\n\
             SCCACHE_REGION=us-east-1\nSCCACHE_S3_USE_SSL=false\nAWS_ACCESS_KEY_ID=key\n\
             AWS_SECRET_ACCESS_KEY=secret\nAWS_EC2_METADATA_DISABLED=true\n",
        )
        .expect("write the host store");
        super::super::permissions::set_mode(&path, consts::OWNER_ONLY)
            .expect("restrict the host store");
        path
    }

    /// A container starts from what its host file says and what this binary
    /// fills in, so a host file that predates a key still yields it.
    #[test]
    fn a_runner_starts_with_its_completed_store() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut host = host_fixture();
        host.runners[0].sccache_s3_env_file = host_store(directory.path());

        let contents =
            runtime_environment(&host.runners[0], "jit").expect("the environment must render");

        for line in [
            "ACTIONS_RUNNER_JITCONFIG=jit",
            "SCCACHE_BUCKET=kithara-review",
            "SCCACHE_S3_KEY_PREFIX=sccache",
            "AWS_SECRET_ACCESS_KEY=secret",
        ] {
            assert!(
                contents.lines().any(|entry| entry == line),
                "{line}:\n{contents}"
            );
        }
    }

    #[test]
    fn a_trusted_runner_marks_only_its_own_job_as_trusted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut host = host_fixture();
        let store = host_store(directory.path());
        for runner in &mut host.runners {
            runner.sccache_s3_env_file.clone_from(&store);
        }
        host.runners[0].cache_trust = CacheTrust::Trusted;
        host.runners[1].cache_trust = CacheTrust::Review;

        let trusted = runtime_environment(&host.runners[0], "jit").unwrap();
        let review = runtime_environment(&host.runners[1], "jit").unwrap();

        assert!(
            trusted.contains("KITHARA_CACHE_TRUST=trusted\n"),
            "{trusted}"
        );
        assert!(review.contains("KITHARA_CACHE_TRUST=review\n"), "{review}");
        assert!(!review.contains("KITHARA_CACHE_TRUST=trusted"), "{review}");
    }

    fn runner(status: &str, busy: bool) -> Registration {
        Registration {
            id: 1,
            name: "gerasim13-04-42".to_owned(),
            status: status.to_owned(),
            busy,
        }
    }

    #[test]
    fn a_busy_offline_runner_does_not_block_a_new_jit_registration() {
        assert!(!is_prunable(&runner("offline", true), "gerasim13-04"));
        assert!(is_prunable(&runner("offline", false), "gerasim13-04"));
        assert!(!is_prunable(&runner("online", false), "gerasim13-04"));
    }

    #[test]
    fn jit_cleanup_cannot_remove_a_similarly_named_runner() {
        let mut registration = runner("offline", false);
        for sibling in [
            "gerasim13-040-42",
            "gerasim13-04-images-42",
            "gerasim13-04-old",
        ] {
            sibling.clone_into(&mut registration.name);
            assert!(!is_prunable(&registration, "gerasim13-04"), "{sibling}");
        }
        format!("gerasim13-04-{}", std::process::id()).clone_into(&mut registration.name);
        assert!(is_prunable(&registration, "gerasim13-04"));
        assert!(!is_prunable(&registration, "gerasim13"));
    }

    #[test]
    fn native_jit_credentials_use_only_the_runners_secret_input() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("native.env");
        write_image_configuration(&path, "one-job-credential").expect("native configuration");
        let text = std::fs::read_to_string(&path).expect("native environment");
        assert_eq!(text, "ACTIONS_RUNNER_INPUT_JITCONFIG=one-job-credential\n");
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(path)
                .expect("native environment metadata")
                .permissions()
                .mode()
                & 0o777,
            consts::OWNER_ONLY
        );
    }
}
