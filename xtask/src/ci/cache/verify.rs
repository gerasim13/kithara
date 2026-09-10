use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

use anyhow::{Context, Result, ensure};
use tempfile::TempDir;
use tracing::info;

use super::client_environment;

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
        environment: client_environment(env_file)?,
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
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn cache_probe_rejects_process_environment_injection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.env");
        fs::write(&path, "PATH=/tmp").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(client_environment(&path).is_err());
        fs::write(&path, "SCCACHE_BUCKET=one\nSCCACHE_BUCKET=two").unwrap();
        assert!(client_environment(&path).is_err());
        fs::write(&path, "SCCACHE_BUCKET=one\\two").unwrap();
        assert!(client_environment(&path).is_err());
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
