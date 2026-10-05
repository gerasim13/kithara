use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};

use super::{super::config::CiPins, evict, provision, snapshot, snapshot::SnapshotArgs, verify};
use crate::{ci::host::mac::read_secret, consts};

/// Everything else a client is told, fixed by how a scope is provisioned.
///
/// A host may state these, and what it states is kept; what it leaves out is
/// filled in here. A host file is written once and outlives the code reading
/// it, so a key added later is missing from every file that predates it. When
/// that key was a required one, the prefix, sccache wrote every object to the
/// bucket root, where nothing expires, and the lane's own client refused to
/// start.
fn defaults(endpoint: &str) -> [(&'static str, &'static str); 4] {
    [
        ("SCCACHE_S3_KEY_PREFIX", consts::SCCACHE_PREFIX),
        ("SCCACHE_REGION", consts::CACHE_REGION),
        (
            "SCCACHE_S3_USE_SSL",
            if endpoint.starts_with("https://") {
                "true"
            } else {
                "false"
            },
        ),
        ("AWS_EC2_METADATA_DISABLED", "true"),
    ]
}

/// The whole environment a client is given for one scope's store.
pub(super) fn provisioned_environment(
    bucket: &str,
    endpoint: &str,
    key: &str,
    secret: &str,
) -> Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for (name, value) in consts::CACHE_HOST_KEYS
        .into_iter()
        .zip([bucket, endpoint, key, secret])
    {
        insert_client_environment(&mut environment, name, value)?;
    }
    complete_client_environment(environment)
}

/// Read the restricted environment a cache client may inherit.
pub(crate) fn client_environment(path: &Path) -> Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for line in read_secret(path)?.lines() {
        let (key, value) = line
            .split_once('=')
            .context("invalid cache environment entry")?;
        insert_client_environment(&mut environment, key, value)?;
    }
    complete_client_environment(environment)
}

/// Read the restricted cache credentials injected into a CI job.
pub(crate) fn current_client_environment() -> Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for key in consts::CACHE_HOST_KEYS {
        let value = env::var(key).with_context(|| format!("{key} must be configured"))?;
        insert_client_environment(&mut environment, key, &value)?;
    }
    for (key, _) in defaults("") {
        if let Some(value) = env::var(key).ok().filter(|value| !value.is_empty()) {
            insert_client_environment(&mut environment, key, &value)?;
        }
    }
    complete_client_environment(environment)
}

/// The defaults a job's own processes lack, once the job was given a store.
///
/// sccache reads its configuration from the environment it starts in, so a
/// key the host left out has to be put into the lane's environment rather than
/// only filled in where this crate reads it. A job with no store keeps its
/// local cache and is told nothing. An empty value is a missing one: a runner
/// that forwards an unset variable hands over an empty string, and sccache
/// reads an empty prefix as the bucket root.
pub(crate) fn missing_defaults(
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<(&'static str, &'static str)> {
    let lookup = |key: &str| lookup(key).filter(|value| !value.is_empty());
    let (Some(_), Some(endpoint)) = (lookup("SCCACHE_BUCKET"), lookup("SCCACHE_ENDPOINT")) else {
        return Vec::new();
    };
    defaults(&endpoint)
        .into_iter()
        .filter(|(key, _)| lookup(key).is_none())
        .collect()
}

fn insert_client_environment(
    environment: &mut BTreeMap<String, String>,
    key: &str,
    value: &str,
) -> Result<()> {
    ensure!(
        consts::CACHE_HOST_KEYS.contains(&key) || defaults("").iter().any(|(name, _)| *name == key),
        "unexpected cache environment key"
    );
    ensure!(!value.is_empty(), "empty cache environment value");
    ensure!(
        !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '"' | '\\')),
        "unsafe cache environment value"
    );
    ensure!(
        environment
            .insert(key.to_owned(), value.to_owned())
            .is_none(),
        "duplicate cache environment key"
    );
    Ok(())
}

fn complete_client_environment(
    mut environment: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    for key in consts::CACHE_HOST_KEYS {
        ensure!(
            environment.contains_key(key),
            "the cache environment has no {key}"
        );
    }
    let endpoint = environment
        .get("SCCACHE_ENDPOINT")
        .cloned()
        .unwrap_or_default();
    for (key, value) in defaults(&endpoint) {
        environment
            .entry(key.to_owned())
            .or_insert_with(|| value.to_owned());
    }
    Ok(environment)
}

#[derive(Debug, Args)]
pub(crate) struct CacheArgs {
    #[command(subcommand)]
    command: CacheCommand,
}

#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// Operate the shared compiler cache through Docker Compose.
    Compose {
        env_file: PathBuf,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Create persistent administrator credentials inside the Compose volume.
    Credentials,
    /// Initialize isolated buckets and client credentials inside Compose.
    Initialize,
    /// Verify cache reuse between two independent compiler daemons.
    Verify { env_file: PathBuf },
    /// Restore and publish immutable trusted Cargo target snapshots.
    Snapshot(SnapshotArgs),
    /// Keep each scope's compiler cache under its budget, evicting the entries
    /// used longest ago.
    Evict,
}

/// Compose against the host's environment file, which also goes to
/// `initialize` whole as `CACHE_ENV_FILE`: it reads a quota under a name built
/// from each scope the host serves, which the Compose file cannot list.
/// Compose resolves that path beside the Compose file, so it is made absolute.
fn compose(env_file: &Path) -> Result<Command> {
    let handed = std::path::absolute(env_file)
        .with_context(|| format!("resolving {}", env_file.display()))?;
    let mut command = Command::new("docker");
    command
        .args(["compose", "--env-file"])
        .arg(env_file)
        .args(["-f", consts::CACHE_COMPOSE_FILE])
        .env("CACHE_ENV_FILE", handed);
    Ok(command)
}

pub(crate) fn run(args: &CacheArgs) -> Result<()> {
    match &args.command {
        CacheCommand::Compose {
            env_file,
            arguments,
        } => {
            let pins = CiPins::load(Path::new(consts::PINS_PATH))?;
            let status = compose(env_file)?
                .args(arguments)
                .env("KITHARA_CACHE_SERVER_IMAGE", &pins.cache_server_image)
                .env("KITHARA_CACHE_CLIENT_IMAGE", &pins.cache_client_image)
                .env("KITHARA_RUST_VERSION", &pins.stable_toolchain)
                .env("KITHARA_RUST_DIGEST", &pins.linux_base_digest)
                .status()
                .context("run cache Compose")?;
            ensure!(status.success(), "cache Compose exited with {status}");
            Ok(())
        }
        CacheCommand::Credentials => provision::credentials(),
        CacheCommand::Initialize => provision::initialize(),
        CacheCommand::Verify { env_file } => verify::run(env_file),
        CacheCommand::Snapshot(args) => snapshot::run(args),
        CacheCommand::Evict => evict::run(),
    }
}

/// Fails with what the client said on stderr when it exited unsuccessfully.
pub(super) fn require_success(output: &Output, what: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

pub(super) fn required(name: &str) -> Result<String> {
    let value = env::var(name).with_context(|| format!("{name} must be configured"))?;
    ensure!(!value.trim().is_empty(), "{name} must not be empty");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use clap::Parser;

    use super::*;
    use crate::ci::config::workspace_root;

    fn host_file(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.env");
        fs::write(&path, contents).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        (directory, path)
    }

    /// The Linux fleet's host files were written before the prefix existed.
    /// Refusing them left the lane without its source layer, and sccache,
    /// which is not asked, wrote to the bucket root.
    #[test]
    fn a_host_file_written_before_the_prefix_still_yields_it() {
        let (_directory, path) = host_file(
            "SCCACHE_BUCKET=kithara-review\nSCCACHE_ENDPOINT=http://kithara-ci-cache:9000\n\
             SCCACHE_REGION=us-east-1\nSCCACHE_S3_USE_SSL=false\nAWS_ACCESS_KEY_ID=key\n\
             AWS_SECRET_ACCESS_KEY=secret\nAWS_EC2_METADATA_DISABLED=true\n",
        );

        let environment = client_environment(&path).expect("an older host file is complete");

        assert_eq!(
            environment.get("SCCACHE_S3_KEY_PREFIX").map(String::as_str),
            Some(consts::SCCACHE_PREFIX)
        );
    }

    #[test]
    fn a_host_names_only_its_store_and_the_credentials_for_it() {
        let (_directory, path) = host_file(
            "SCCACHE_BUCKET=kithara-trusted\nSCCACHE_ENDPOINT=https://cache\n\
             AWS_ACCESS_KEY_ID=key\nAWS_SECRET_ACCESS_KEY=secret\n",
        );

        let environment = client_environment(&path).expect("the host said all it has to");

        for (key, value) in [
            ("SCCACHE_S3_KEY_PREFIX", consts::SCCACHE_PREFIX),
            ("SCCACHE_REGION", consts::CACHE_REGION),
            ("SCCACHE_S3_USE_SSL", "true"),
            ("AWS_EC2_METADATA_DISABLED", "true"),
        ] {
            assert_eq!(
                environment.get(key).map(String::as_str),
                Some(value),
                "{key}"
            );
        }
    }

    #[test]
    fn what_a_host_states_is_kept() {
        let (_directory, path) = host_file(
            "SCCACHE_BUCKET=kithara-review\nSCCACHE_ENDPOINT=http://cache\n\
             SCCACHE_REGION=eu-central-1\nAWS_ACCESS_KEY_ID=key\nAWS_SECRET_ACCESS_KEY=secret\n",
        );

        let environment = client_environment(&path).unwrap();

        assert_eq!(
            environment.get("SCCACHE_REGION").map(String::as_str),
            Some("eu-central-1")
        );
    }

    #[test]
    fn a_host_file_without_credentials_is_refused_by_name() {
        let (_directory, path) = host_file(
            "SCCACHE_BUCKET=kithara-review\nSCCACHE_ENDPOINT=http://cache\nAWS_ACCESS_KEY_ID=key\n",
        );

        let error = client_environment(&path).expect_err("no secret, no client");

        assert!(
            error.to_string().contains("AWS_SECRET_ACCESS_KEY"),
            "{error}"
        );
    }

    #[test]
    fn a_job_given_a_store_is_told_what_its_host_left_out() {
        let environment = BTreeMap::from([
            ("SCCACHE_BUCKET", "kithara-review"),
            ("SCCACHE_ENDPOINT", "http://kithara-ci-cache:9000"),
            ("SCCACHE_REGION", "us-east-1"),
        ]);

        let missing = missing_defaults(|key| environment.get(key).map(|value| (*value).to_owned()));

        assert_eq!(
            missing,
            [
                ("SCCACHE_S3_KEY_PREFIX", consts::SCCACHE_PREFIX),
                ("SCCACHE_S3_USE_SSL", "false"),
                ("AWS_EC2_METADATA_DISABLED", "true"),
            ]
        );
    }

    #[test]
    fn a_job_handed_an_empty_prefix_is_given_the_default() {
        let environment = BTreeMap::from([
            ("SCCACHE_BUCKET", "kithara-review"),
            ("SCCACHE_ENDPOINT", "http://kithara-ci-cache:9000"),
            ("SCCACHE_S3_KEY_PREFIX", ""),
        ]);

        let missing = missing_defaults(|key| environment.get(key).map(|value| (*value).to_owned()));

        assert!(
            missing.contains(&("SCCACHE_S3_KEY_PREFIX", consts::SCCACHE_PREFIX)),
            "{missing:?}"
        );
    }

    #[test]
    fn a_job_without_a_store_is_told_nothing() {
        assert!(missing_defaults(|_| None).is_empty());
    }

    /// `initialize` reads a quota under a name built from each scope the host
    /// serves. Compose hands a container only the variables its file names,
    /// so the quota a host named for one scope never reached it, and every
    /// initialize flattened that scope to the shared quota.
    #[test]
    fn initialize_is_handed_the_environment_file_the_stack_is_started_with() {
        let host = Path::new("docker/ci-cache/linux.env");

        let command = compose(host).unwrap();

        let handed = command
            .get_envs()
            .find(|(key, _)| *key == "CACHE_ENV_FILE")
            .and_then(|(_, value)| value)
            .map(PathBuf::from);
        assert_eq!(handed, Some(env::current_dir().unwrap().join(host)));
        let stack: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            &fs::read_to_string(workspace_root().join(consts::CACHE_COMPOSE_FILE)).unwrap(),
        )
        .unwrap();
        assert!(
            stack["services"]["initialize"]["env_file"]
                .as_str()
                .is_some_and(|file| file.starts_with("${CACHE_ENV_FILE")),
            "initialize must read the file CACHE_ENV_FILE names"
        );
    }

    /// A named volume lives wherever the Docker daemon keeps it - inside
    /// colima's virtual machine on the Mac - so a stack brought up after that
    /// daemon was reset starts with an empty store and a new admin password,
    /// and every client key the runners hold stops working. Everything the
    /// stack keeps has to sit in a directory the host names on its own disk.
    #[test]
    fn the_stack_keeps_its_state_only_in_directories_the_host_names() {
        let stack: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            &fs::read_to_string(workspace_root().join(consts::CACHE_COMPOSE_FILE)).unwrap(),
        )
        .unwrap();

        assert!(
            stack.get("volumes").is_none(),
            "the stack declares named volumes"
        );
        let services = stack["services"].as_mapping().unwrap();
        let mut mounts = 0;
        for (name, service) in services {
            for mount in service["volumes"].as_sequence().into_iter().flatten() {
                let mount = mount.as_str().unwrap();
                let (source, _) = mount.split_once("}:").unwrap_or((mount, ""));
                assert!(
                    source.starts_with("${CACHE_") && source.contains("_VOLUME:?"),
                    "{name:?} mounts {mount}, which is not a host directory the environment must name"
                );
                mounts += 1;
            }
        }
        assert!(mounts > 0, "the stack mounts nothing");
    }

    fn stack() -> serde_yaml_ng::Value {
        serde_yaml_ng::from_str(
            &fs::read_to_string(workspace_root().join(consts::CACHE_COMPOSE_FILE)).unwrap(),
        )
        .unwrap()
    }

    /// The store sends its audit log to the evictor by container name, and
    /// its egress filter refuses a private address whose origin it was not
    /// told to allow. An endpoint that drifted from the evictor's name or port
    /// would leave every read unseen, and the evictor would age entries by
    /// their writes alone.
    #[test]
    fn the_store_sends_its_audit_log_to_the_evictor() {
        let stack = stack();
        let cache = &stack["services"]["cache"];
        let environment = &cache["environment"];
        let setting = |name: &str| {
            environment[name]
                .as_str()
                .unwrap_or_else(|| panic!("the store sets no {name}"))
        };

        let endpoint =
            reqwest::Url::parse(setting("RUSTFS_AUDIT_WEBHOOK_ENDPOINT_RECENCY")).unwrap();

        assert_eq!(setting("RUSTFS_AUDIT_ENABLE"), "true");
        assert_eq!(setting("RUSTFS_AUDIT_WEBHOOK_ENABLE_RECENCY"), "on");
        assert_eq!(
            endpoint.host_str(),
            stack["services"]["evict"]["container_name"].as_str()
        );
        assert_eq!(endpoint.port(), Some(consts::EVICT_PORT));
        let origin = endpoint.origin().ascii_serialization();
        assert!(
            setting("RUSTFS_OUTBOUND_ALLOW_ORIGINS")
                .split(',')
                .any(|allowed| allowed == origin),
            "the store's egress filter refuses {origin}"
        );
        let queue = setting("RUSTFS_AUDIT_WEBHOOK_QUEUE_DIR_RECENCY");
        assert!(
            cache["tmpfs"]
                .as_sequence()
                .into_iter()
                .flatten()
                .filter_map(serde_yaml_ng::Value::as_str)
                .any(|mount| mount.split(':').next() == Some(queue)),
            "the audit queue {queue} is not in memory"
        );
    }

    /// The evictor reads the scopes and the administrator's credentials the
    /// way `initialize` does, starts only once the buckets exist, and nothing
    /// outside the stack reaches the port the audit log arrives on.
    #[test]
    fn the_evictor_runs_after_initialize_and_publishes_no_port() {
        let stack = stack();
        let evict = &stack["services"]["evict"];

        let entrypoint = evict["entrypoint"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap())
            .collect::<Vec<_>>();

        assert!(
            crate::Cli::try_parse_from(&entrypoint).is_ok(),
            "{entrypoint:?}"
        );
        assert_eq!(entrypoint.last(), Some(&"evict"));
        assert_eq!(
            evict["env_file"],
            stack["services"]["initialize"]["env_file"]
        );
        assert_eq!(
            evict["depends_on"]["initialize"]["condition"].as_str(),
            Some("service_completed_successfully")
        );
        assert!(
            evict["volumes"]
                .as_sequence()
                .into_iter()
                .flatten()
                .filter_map(serde_yaml_ng::Value::as_str)
                .any(|mount| mount.ends_with(":/config:ro")),
            "the evictor cannot read the administrator's credentials"
        );
        assert!(evict.get("ports").is_none(), "the evictor publishes a port");
    }

    /// Job containers share the network they reach the store on, and the
    /// evictor takes every delivery on trust: a job that could post to it
    /// could age the trusted scope's entries or hold its recounts forever.
    #[test]
    fn only_the_store_reaches_the_evictor() {
        let stack = stack();
        let services = stack["services"].as_mapping().unwrap();
        let joined = |service: &serde_yaml_ng::Value| {
            service["networks"]
                .as_sequence()
                .into_iter()
                .flatten()
                .filter_map(serde_yaml_ng::Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };

        let evict = joined(&stack["services"]["evict"]);

        assert!(!evict.is_empty(), "the evictor joins no network");
        for network in &evict {
            assert_eq!(
                stack["networks"][network.as_str()]["internal"].as_bool(),
                Some(true),
                "{network} leaves the stack"
            );
            for (name, service) in services {
                let name = name.as_str().unwrap();
                let joins = joined(service).contains(network);
                assert_eq!(
                    joins,
                    matches!(name, "cache" | "evict"),
                    "{name} and {network}"
                );
            }
        }
    }

    #[test]
    fn provisioning_writes_what_a_client_reads() {
        let written = provisioned_environment("kithara-review", "http://cache", "key", "secret")
            .expect("provisioning names every host key");

        assert_eq!(
            written.len(),
            consts::CACHE_HOST_KEYS.len() + defaults("").len()
        );
        assert_eq!(
            written.get("SCCACHE_S3_KEY_PREFIX").map(String::as_str),
            Some(consts::SCCACHE_PREFIX)
        );
    }
}
