use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use kithara_devtools::common::tools::ToolsConfig;
use tracing::warn;

use super::process::Process;
use crate::consts;

/// The kache one CI job compiles through.
///
/// The job writes a configuration file into a directory of its own, which is
/// also where the daemon keeps its socket and the job's events. The remote is
/// not the job's: kache reads it from its host layer, which the runner writes
/// for its scope, and the job's file outranks that layer key by key only
/// where it names a key.
pub(crate) struct KacheJob {
    directory: PathBuf,
}

/// Where a job keeps what it compiled, and how much of it.
pub(crate) struct LocalStore<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) max_size: &'a str,
}

impl KacheJob {
    pub(crate) fn prepare(directory: PathBuf, store: Option<LocalStore<'_>>) -> Result<Self> {
        let job = Self { directory };
        let runtime = job.directory.join("runtime");
        // A directory left by a job that died under the same name holds a
        // daemon's socket that nothing answers.
        let _ = fs::remove_dir_all(&job.directory);
        fs::create_dir_all(&runtime)
            .with_context(|| format!("creating kache's directory {}", runtime.display()))?;
        let mut cache = toml::Table::new();
        cache.insert("runtime_dir".into(), path_value(&runtime)?);
        cache.insert(
            "daemon_idle_timeout_secs".into(),
            consts::KACHE_DAEMON_IDLE_SECS.into(),
        );
        if let Some(store) = store {
            cache.insert("local_store".into(), path_value(store.directory)?);
            cache.insert("local_max_size".into(), store.max_size.into());
        }
        let mut config = toml::Table::new();
        config.insert("cache".into(), cache.into());
        fs::write(job.config(), toml::to_string(&config)?)
            .with_context(|| format!("writing {}", job.config().display()))?;
        Ok(job)
    }

    /// What the job's compilations are told: the configuration, kache as
    /// the wrapper, and sccache for what kache hands on. cc-rs puts kache in
    /// front of a build script's C compile on its own, seeing it as the Rust
    /// wrapper; a `CMake` build takes it as its compiler launcher, so the
    /// compiler `CMake` records stays the real one whichever job configured the
    /// build directory.
    ///
    /// The compilations see no CI system: kache keeps a GitHub or GitLab job
    /// that is not a push to a protected branch from writing, whatever its
    /// credentials allow, and every branch job of the fork is such a job. The
    /// scope's credentials are what decide where a job writes.
    pub(crate) fn environment(&self, tools: &ToolsConfig) -> [(&'static str, OsString); 7] {
        [
            ("CMAKE_CXX_COMPILER_LAUNCHER", tools.program("kache").into()),
            ("CMAKE_C_COMPILER_LAUNCHER", tools.program("kache").into()),
            ("GITHUB_ACTIONS", OsString::new()),
            ("GITLAB_CI", OsString::new()),
            ("KACHE_CONFIG", self.config().into_os_string()),
            ("KACHE_FALLBACK", tools.program("sccache").into()),
            ("RUSTC_WRAPPER", tools.program("kache").into()),
        ]
    }

    /// Stops the job's daemon, which first stores what it still holds
    /// queued: kache uploads after the compile that produced an entry has
    /// returned. A stop that fails costs the next job those entries, not
    /// this one its outcome.
    pub(crate) fn stop(&self, process: &Process, tools: &ToolsConfig) {
        let mut command = process.command(tools.program("kache"));
        command
            .args(["daemon", "stop"])
            .env("KACHE_CONFIG", self.config());
        if let Err(error) = process.run_command(&mut command, "stop the job's kache daemon") {
            warn!(
                error = format!("{error:#}"),
                "kache stopped before it stored everything this lane compiled"
            );
        }
    }

    fn config(&self) -> PathBuf {
        self.directory.join("config.toml")
    }
}

/// The kache host layer a runner gives every compile of its jobs, named by
/// `KACHE_HOST_CONFIG`: the store compiles land in when no job places them
/// itself, and the remote of the scope `environment` holds the credentials of.
///
/// The remote has to be in a file. kache strips a remote named only in the
/// environment from a daemon it starts itself, because that daemon outlives
/// the compile that started it, and the daemon then uploads nothing. A job's
/// own configuration outranks this layer key by key, so a job that places its
/// store keeps that choice and still shares through this remote. The
/// credentials stay in the environment, which the daemon inherits.
pub(crate) fn host_config(
    environment: &BTreeMap<String, String>,
    store: &Path,
    max_size: &str,
) -> Result<String> {
    let value = |name: &str| {
        environment
            .get(name)
            .with_context(|| format!("the cache environment has no {name}"))
            .map(String::as_str)
    };
    let mut remote = toml::Table::new();
    remote.insert("type".into(), "s3".into());
    remote.insert("bucket".into(), value("SCCACHE_BUCKET")?.into());
    remote.insert("endpoint".into(), value("SCCACHE_ENDPOINT")?.into());
    remote.insert("region".into(), value("SCCACHE_REGION")?.into());
    remote.insert("prefix".into(), consts::KACHE_PREFIX.into());
    let mut cache = toml::Table::new();
    cache.insert("local_store".into(), path_value(store)?);
    cache.insert("local_max_size".into(), max_size.into());
    cache.insert("remote".into(), remote.into());
    let mut config = toml::Table::new();
    config.insert("cache".into(), cache.into());
    Ok(toml::to_string(&config)?)
}

fn path_value(path: &Path) -> Result<toml::Value> {
    Ok(path
        .to_str()
        .with_context(|| format!("{} is not UTF-8", path.display()))?
        .into())
}

impl Drop for KacheJob {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsStr};

    use super::*;

    fn written(job: &KacheJob) -> toml::Table {
        toml::from_str(&fs::read_to_string(job.config()).unwrap()).unwrap()
    }

    /// The remote is the runner's to name, in kache's host layer, which a
    /// job's own file outranks key by key: a remote named here would be one
    /// more place the bucket is spelled, and only lanes would see it.
    #[test]
    fn a_jobs_own_file_leaves_the_remote_to_its_runner() {
        let directory = tempfile::tempdir().unwrap();

        let job = KacheJob::prepare(directory.path().join("job"), None).unwrap();

        let config = written(&job);
        let cache = config["cache"].as_table().unwrap();
        assert!(!cache.contains_key("remote"), "{cache:?}");
        assert!(!cache.contains_key("local_store"), "{cache:?}");
    }

    fn store_environment(bucket: &str) -> BTreeMap<String, String> {
        [
            ("SCCACHE_BUCKET", bucket),
            ("SCCACHE_ENDPOINT", "http://kithara-ci-cache:9000"),
            ("SCCACHE_REGION", consts::CACHE_REGION),
            ("AWS_ACCESS_KEY_ID", "key-id"),
            ("AWS_SECRET_ACCESS_KEY", "secret-key"),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
    }

    /// kache drops a remote named only in the environment from the daemon it
    /// starts itself, so the daemon would run local-only and upload nothing.
    /// The host layer is a file the daemon reads, so every compile a runner
    /// serves shares through the scope's bucket, whatever command started it.
    #[test]
    fn a_runners_compiles_share_through_its_scopes_bucket() {
        for bucket in ["kithara-trusted", "kithara-review"] {
            let rendered = host_config(
                &store_environment(bucket),
                Path::new("/cache/kache/store"),
                "50G",
            )
            .unwrap();

            let config: toml::Table = toml::from_str(&rendered).unwrap();
            let cache = config["cache"].as_table().unwrap();
            assert_eq!(cache["local_store"].as_str(), Some("/cache/kache/store"));
            assert_eq!(cache["local_max_size"].as_str(), Some("50G"));
            let remote = cache["remote"].as_table().unwrap();
            assert_eq!(remote["type"].as_str(), Some("s3"), "{bucket}");
            assert_eq!(remote["bucket"].as_str(), Some(bucket));
            assert_eq!(
                remote["endpoint"].as_str(),
                Some("http://kithara-ci-cache:9000")
            );
            assert_eq!(remote["region"].as_str(), Some(consts::CACHE_REGION));
            assert_eq!(remote["prefix"].as_str(), Some(consts::KACHE_PREFIX));
        }
    }

    /// The file is readable by every job the runner serves; the credentials
    /// stay in the environment, which the daemon inherits.
    #[test]
    fn a_runners_kache_file_carries_no_credentials() {
        let rendered = host_config(
            &store_environment("kithara-review"),
            Path::new("/cache/kache/store"),
            "50G",
        )
        .unwrap();

        assert!(!rendered.contains("key-id"), "{rendered}");
        assert!(!rendered.contains("secret-key"), "{rendered}");
    }

    #[test]
    fn a_job_without_a_store_keeps_what_it_compiles_to_itself() {
        let directory = tempfile::tempdir().unwrap();
        let store_root = directory.path().join("store");

        let job = KacheJob::prepare(
            directory.path().join("job"),
            Some(LocalStore {
                directory: &store_root,
                max_size: "4G",
            }),
        )
        .unwrap();

        let config = written(&job);
        let cache = config["cache"].as_table().unwrap();
        assert!(!cache.contains_key("remote"));
        assert_eq!(cache["local_store"].as_str(), store_root.to_str());
        assert_eq!(cache["local_max_size"].as_str(), Some("4G"));
        assert_eq!(
            cache["runtime_dir"].as_str().map(PathBuf::from),
            Some(directory.path().join("job/runtime"))
        );
    }

    /// A cancelled job is killed before it can stop its daemon, which kache
    /// detaches from the job; only the daemon itself can end it then.
    #[test]
    fn a_daemon_whose_job_never_stops_it_exits_once_idle() {
        let directory = tempfile::tempdir().unwrap();

        let job = KacheJob::prepare(directory.path().join("job"), None).unwrap();

        let config = written(&job);
        let timeout = config["cache"]["daemon_idle_timeout_secs"].as_integer();
        assert!(timeout.is_some_and(|seconds| seconds > 0), "{timeout:?}");
    }

    /// A `CMake` build reaches kache as its compiler launcher, never as its
    /// compiler: a build directory another job configured with the real
    /// compiler would otherwise see the compiler change, and `CMake` then
    /// deletes its cache and configures without the flags `cmake-rs` passed,
    /// `-fPIC` among them.
    #[test]
    fn a_job_compiles_through_kache_and_hands_on_to_sccache() {
        let directory = tempfile::tempdir().unwrap();
        let tools = ToolsConfig::default();
        let job = KacheJob::prepare(directory.path().join("job"), None).unwrap();

        let environment = job.environment(&tools);

        assert_eq!(
            environment,
            [
                (
                    "CMAKE_CXX_COMPILER_LAUNCHER",
                    OsString::from(tools.program("kache"))
                ),
                (
                    "CMAKE_C_COMPILER_LAUNCHER",
                    OsString::from(tools.program("kache"))
                ),
                ("GITHUB_ACTIONS", OsString::new()),
                ("GITLAB_CI", OsString::new()),
                ("KACHE_CONFIG", job.config().into_os_string()),
                ("KACHE_FALLBACK", OsString::from(tools.program("sccache"))),
                ("RUSTC_WRAPPER", OsString::from(tools.program("kache"))),
            ]
        );
    }

    /// kache keeps a GitHub or GitLab job read-only unless it is a push to a
    /// protected branch, and every branch job of the fork is a push to an
    /// unprotected one. The scope's credentials decide what a job may write.
    #[test]
    fn kache_sees_no_ci_system_whose_policy_would_keep_a_job_read_only() {
        let directory = tempfile::tempdir().unwrap();
        let job = KacheJob::prepare(directory.path().join("job"), None).unwrap();

        let environment = job.environment(&ToolsConfig::default());

        for marker in ["GITHUB_ACTIONS", "GITLAB_CI"] {
            assert_eq!(
                environment
                    .iter()
                    .find(|(name, _)| *name == marker)
                    .map(|(_, value)| value.as_os_str()),
                Some(OsStr::new("")),
                "{marker}"
            );
        }
    }

    #[test]
    fn a_finished_job_leaves_nothing_of_its_own_behind() {
        let directory = tempfile::tempdir().unwrap();
        let own = directory.path().join("job");

        drop(KacheJob::prepare(own.clone(), None).unwrap());

        assert!(!own.exists());
    }
}
