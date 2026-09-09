use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, ensure};
use tempfile::TempDir;
use tracing::info;

use crate::ci::host::read_secret;

const CLIENT_KEYS: [&str; 7] = [
    "SCCACHE_BUCKET",
    "SCCACHE_ENDPOINT",
    "SCCACHE_REGION",
    "SCCACHE_S3_USE_SSL",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_EC2_METADATA_DISABLED",
];

struct Probe {
    root: TempDir,
    environment: BTreeMap<String, String>,
}

impl Probe {
    fn client(&self, server: &str) -> Command {
        let mut command = Command::new("sccache");
        command
            .envs(&self.environment)
            .env(
                "SCCACHE_SERVER_UDS",
                self.root.path().join(format!("{server}.sock")),
            )
            .env("SCCACHE_DIR", self.root.path().join("local"))
            .env(
                "SCCACHE_S3_KEY_PREFIX",
                format!("verification{}/", self.root.path().display()),
            );
        command
    }

    fn build(&self, server: &str, name: &str) -> Result<()> {
        let output = self
            .client(server)
            .args([
                "rustc",
                "--crate-name",
                name,
                "--crate-type",
                "lib",
                "--emit=dep-info,metadata,link",
                "--out-dir",
            ])
            .arg(self.root.path().join("out"))
            .arg(self.root.path().join(format!("{name}.rs")))
            .output()
            .context("run compiler-cache probe")?;
        require_success(&output)
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        for server in ["a", "b"] {
            let _ = self
                .client(server)
                .arg("--stop-server")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn require_success(output: &Output) -> Result<()> {
    ensure!(
        output.status.success(),
        "compiler-cache probe failed: {}",
        output.status
    );
    Ok(())
}

fn environment(body: &str) -> Result<BTreeMap<String, String>> {
    let mut environment = BTreeMap::new();
    for line in body.lines() {
        let (key, value) = line
            .split_once('=')
            .context("invalid cache environment entry")?;
        ensure!(
            CLIENT_KEYS.contains(&key),
            "unexpected cache environment key"
        );
        ensure!(!value.is_empty(), "empty cache environment value");
        ensure!(
            environment
                .insert(key.to_owned(), value.to_owned())
                .is_none(),
            "duplicate cache environment key"
        );
    }
    ensure!(
        environment.len() == CLIENT_KEYS.len(),
        "incomplete cache environment"
    );
    Ok(environment)
}

fn require_counter(stats: &str, name: &str, expected: usize) -> Result<()> {
    let count = stats
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)
                .and_then(|value| value.trim().parse::<usize>().ok())
        })
        .with_context(|| format!("sccache did not report {name}"))?;
    ensure!(
        count == expected,
        "{name}: expected {expected}, observed {count}"
    );
    Ok(())
}

pub(super) fn run(env_file: &Path) -> Result<()> {
    let probe = Probe {
        root: tempfile::Builder::new()
            .prefix("ci-cache-")
            .tempdir_in("/tmp")?,
        environment: environment(&read_secret(env_file)?)?,
    };
    fs::create_dir(probe.root.path().join("out"))?;
    for name in ["seed", "target"] {
        fs::write(
            probe.root.path().join(format!("{name}.rs")),
            "pub fn value() -> u64 { 42 }",
        )?;
    }
    probe.build("b", "seed")?;
    probe.build("a", "target")?;
    probe.build("b", "target")?;
    let output = probe.client("b").arg("--show-stats").output()?;
    require_success(&output)?;
    let stats = String::from_utf8(output.stdout)?;
    for (name, count) in [
        ("Cache hits ", 1),
        ("Compilations ", 1),
        ("Cache read errors ", 0),
        ("Cache write errors ", 0),
    ] {
        require_counter(&stats, name, count)?;
    }
    ensure!(
        stats
            .lines()
            .any(|line| line.starts_with("Cache location") && line.contains("s3,")),
        "probe did not use the S3 backend"
    );
    info!(%stats, "independent cache clients reused a write without restart");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_probe_rejects_process_environment_injection() {
        assert!(environment("PATH=/tmp").is_err());
        assert!(
            environment(
                "SCCACHE_BUCKET=one
SCCACHE_BUCKET=two"
            )
            .is_err()
        );
    }

    #[test]
    fn misses_cannot_satisfy_the_cache_hit_check() {
        assert!(
            require_counter(
                "Cache hits 0
Compilations 2",
                "Cache hits ",
                1
            )
            .is_err()
        );
        assert!(
            require_counter(
                "Cache hits 1
Cache hits (Rust) 1",
                "Cache hits ",
                1
            )
            .is_ok()
        );
    }
}
