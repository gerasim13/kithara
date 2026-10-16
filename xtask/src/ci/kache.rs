use std::{
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
/// kache's daemon takes its remote only from a configuration file, so the job
/// writes one into a directory of its own, which is also where the daemon
/// keeps its socket and the job's events. The remote is the bucket the job's
/// credentials name, the one sccache shares its compiles through: each scope
/// reads what its own jobs wrote and never another scope's.
pub(crate) struct KacheJob {
    directory: PathBuf,
}

/// Where a job keeps what it compiled, and how much of it.
pub(crate) struct LocalStore<'a> {
    pub(crate) directory: &'a Path,
    pub(crate) max_size: &'a str,
}

impl KacheJob {
    pub(crate) fn prepare(
        directory: PathBuf,
        store: Option<LocalStore<'_>>,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let job = Self { directory };
        let runtime = job.directory.join("runtime");
        // A directory left by a job that died under the same name holds a
        // daemon's socket that nothing answers.
        let _ = fs::remove_dir_all(&job.directory);
        fs::create_dir_all(&runtime)
            .with_context(|| format!("creating kache's directory {}", runtime.display()))?;
        let mut cache = toml::Table::new();
        cache.insert("runtime_dir".into(), path_value(&runtime)?);
        if let Some(store) = store {
            cache.insert("local_store".into(), path_value(store.directory)?);
            cache.insert("local_max_size".into(), store.max_size.into());
        }
        let present = |name: &str| lookup(name).filter(|value| !value.is_empty());
        if let (Some(bucket), Some(endpoint)) =
            (present("SCCACHE_BUCKET"), present("SCCACHE_ENDPOINT"))
        {
            let mut remote = toml::Table::new();
            remote.insert("type".into(), "s3".into());
            remote.insert("bucket".into(), bucket.into());
            remote.insert("endpoint".into(), endpoint.into());
            remote.insert("region".into(), consts::CACHE_REGION.into());
            remote.insert("prefix".into(), consts::KACHE_PREFIX.into());
            cache.insert("remote".into(), remote.into());
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
    use std::{collections::HashMap, ffi::OsStr};

    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>();
        move |name| pairs.get(name).cloned()
    }

    fn written(job: &KacheJob) -> toml::Table {
        toml::from_str(&fs::read_to_string(job.config()).unwrap()).unwrap()
    }

    fn store(credentials: &str) -> [(&'static str, String); 2] {
        [
            ("SCCACHE_BUCKET", credentials.to_owned()),
            (
                "SCCACHE_ENDPOINT",
                "http://kithara-ci-cache:9000".to_owned(),
            ),
        ]
    }

    /// A job keeps kache's objects in the bucket its credentials write, as
    /// sccache does: a remote the job may only read would never fill, because
    /// no host runs every lane as a protected branch's push.
    #[test]
    fn every_job_shares_its_compiles_through_its_own_scopes_bucket() {
        let directory = tempfile::tempdir().unwrap();
        for credentials in ["kithara-trusted", "kithara-review"] {
            let pairs = store(credentials);
            let pairs = pairs
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect::<Vec<_>>();

            let job = KacheJob::prepare(directory.path().join(credentials), None, &lookup(&pairs))
                .unwrap();

            let config = written(&job);
            let cache = config["cache"].as_table().unwrap();
            assert!(!cache.contains_key("remote_readonly"), "{credentials}");
            let remote = cache["remote"].as_table().unwrap();
            assert_eq!(remote["type"].as_str(), Some("s3"));
            assert_eq!(remote["bucket"].as_str(), Some(credentials));
            assert_eq!(remote["prefix"].as_str(), Some(consts::KACHE_PREFIX));
            assert_eq!(
                remote["endpoint"].as_str(),
                Some("http://kithara-ci-cache:9000")
            );
            assert_eq!(remote["region"].as_str(), Some(consts::CACHE_REGION));
        }
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
            &lookup(&[]),
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

    /// A `CMake` build reaches kache as its compiler launcher, never as its
    /// compiler: a build directory another job configured with the real
    /// compiler would otherwise see the compiler change, and `CMake` then
    /// deletes its cache and configures without the flags `cmake-rs` passed,
    /// `-fPIC` among them.
    #[test]
    fn a_job_compiles_through_kache_and_hands_on_to_sccache() {
        let directory = tempfile::tempdir().unwrap();
        let tools = ToolsConfig::default();
        let job = KacheJob::prepare(directory.path().join("job"), None, &lookup(&[])).unwrap();

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
        let job = KacheJob::prepare(directory.path().join("job"), None, &lookup(&[])).unwrap();

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

        drop(KacheJob::prepare(own.clone(), None, &lookup(&[])).unwrap());

        assert!(!own.exists());
    }
}
