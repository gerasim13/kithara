use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use kithara_devtools::Ctx;
use serde::Deserialize;
use tracing::info;

const CONFIG_PATH: &str = ".config/mutation-suites.toml";

#[derive(Debug, Args)]
pub(crate) struct MutantsArgs {
    #[command(subcommand)]
    command: MutantsCommand,
}

#[derive(Debug, Subcommand)]
enum MutantsCommand {
    /// List the explicitly allowed mutation suites.
    List,
    /// Run one explicitly allowed mutation suite.
    Run {
        /// Suite name from .config/mutation-suites.toml.
        suite: String,
        /// Output directory for cargo-mutants reports.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Concurrent cargo-mutants jobs.
        #[arg(long, default_value_t = 1)]
        jobs: usize,
    },
    /// Run every explicitly allowed mutation suite sequentially.
    RunAll {
        /// Parent output directory for cargo-mutants reports.
        #[arg(long, default_value = "target/mutants-ci")]
        output: PathBuf,
        /// Concurrent cargo-mutants jobs within one suite.
        #[arg(long, default_value_t = 1)]
        jobs: usize,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationConfig {
    suite: Vec<MutationSuite>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationSuite {
    name: String,
    package: String,
    files: Vec<PathBuf>,
    #[serde(default)]
    features: Vec<String>,
    test_filters: Vec<String>,
    timeout_seconds: u64,
}

pub(crate) fn run(args: &MutantsArgs, ctx: &Ctx) -> Result<()> {
    let config = MutationConfig::load(&ctx.root)?;
    match &args.command {
        MutantsCommand::List => {
            for suite in &config.suite {
                info!(
                    suite = suite.name,
                    package = suite.package,
                    files = suite
                        .files
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                    tests = suite.test_filters.join(","),
                    "focused mutation suite"
                );
            }
            Ok(())
        }
        MutantsCommand::Run {
            suite,
            output,
            jobs,
        } => {
            let suite = config.named(suite)?;
            let output = output.as_deref().map_or_else(
                || ctx.root.join("target/mutants").join(&suite.name),
                |path| rooted(&ctx.root, path),
            );
            suite.execute(&ctx.root, &output, *jobs)
        }
        MutantsCommand::RunAll { output, jobs } => {
            let output = rooted(&ctx.root, output);
            for suite in &config.suite {
                suite.execute(&ctx.root, &output.join(&suite.name), *jobs)?;
            }
            Ok(())
        }
    }
}

fn rooted(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

impl MutationConfig {
    fn load(root: &Path) -> Result<Self> {
        let path = root.join(CONFIG_PATH);
        let content =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self =
            toml::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
        config.validate(root)?;
        Ok(config)
    }

    fn validate(&self, root: &Path) -> Result<()> {
        if self.suite.is_empty() {
            bail!("{CONFIG_PATH} must define at least one [[suite]]");
        }

        let mut names = BTreeSet::new();
        for suite in &self.suite {
            suite.validate(root)?;
            if !names.insert(&suite.name) {
                bail!("duplicate mutation suite: {}", suite.name);
            }
        }
        Ok(())
    }

    fn named(&self, name: &str) -> Result<&MutationSuite> {
        self.suite
            .iter()
            .find(|suite| suite.name == name)
            .with_context(|| format!("unknown mutation suite: {name}"))
    }
}

impl MutationSuite {
    fn validate(&self, root: &Path) -> Result<()> {
        if self.name.is_empty()
            || !self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            bail!(
                "mutation suite names must use lowercase ASCII, digits, and dashes: {}",
                self.name
            );
        }
        if self.package.trim().is_empty() {
            bail!("mutation suite {} has no package", self.name);
        }
        if self.files.is_empty() {
            bail!("mutation suite {} has no production files", self.name);
        }
        for file in &self.files {
            if file.is_absolute()
                || file.extension().and_then(|ext| ext.to_str()) != Some("rs")
                || file
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
                || !root.join(file).is_file()
            {
                bail!(
                    "mutation suite {} has invalid production file: {}",
                    self.name,
                    file.display()
                );
            }
            if !file.starts_with("crates")
                || !file.components().any(|part| part.as_os_str() == "src")
            {
                bail!(
                    "mutation suite {} may mutate only crate src files: {}",
                    self.name,
                    file.display()
                );
            }
        }
        if self.test_filters.is_empty()
            || self
                .test_filters
                .iter()
                .any(|filter| !filter.starts_with("test(") || !filter.ends_with(')'))
        {
            bail!(
                "mutation suite {} must define explicit nextest test() filters",
                self.name
            );
        }
        if !(10..=900).contains(&self.timeout_seconds) {
            bail!(
                "mutation suite {} timeout must be between 10 and 900 seconds",
                self.name
            );
        }
        Ok(())
    }

    fn execute(&self, root: &Path, output: &Path, jobs: usize) -> Result<()> {
        if jobs == 0 {
            bail!("mutation jobs must be positive");
        }
        fs::create_dir_all(output)
            .with_context(|| format!("creating mutation output {}", output.display()))?;
        info!(suite = self.name, "starting mutation suite");
        let status = self
            .command(root, output, jobs)
            .status()
            .with_context(|| format!("running mutation suite {}", self.name))?;
        if !status.success() {
            bail!("mutation suite {} failed with {status}", self.name);
        }
        Ok(())
    }

    fn command(&self, root: &Path, output: &Path, jobs: usize) -> Command {
        let mut command = Command::new("cargo");
        command
            .current_dir(root)
            .arg("mutants")
            .arg("--package")
            .arg(&self.package)
            .arg("--baseline=run")
            .arg("--test-tool=nextest")
            .arg("--profile=test-release")
            .arg("--no-shuffle")
            .arg("--jobs")
            .arg(jobs.to_string())
            .arg("--timeout")
            .arg(self.timeout_seconds.to_string())
            .arg("--minimum-test-timeout")
            .arg(self.timeout_seconds.to_string())
            .arg("--output")
            .arg(output)
            .arg("--cargo-test-arg=--lib")
            .arg("--cargo-test-arg=-E")
            .arg(format!(
                "--cargo-test-arg={}",
                combined_test_filter(&self.test_filters)
            ));

        for file in &self.files {
            command.arg("--file").arg(file);
        }
        if !self.features.is_empty() {
            command.arg("--features").arg(self.features.join(","));
        }
        command
    }
}

fn combined_test_filter(filters: &[String]) -> String {
    if filters.len() == 1 {
        filters[0].clone()
    } else {
        format!("any({})", filters.join(","))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn write_fixture(root: &Path, content: &str) {
        let source = root.join("crates/example/src/lib.rs");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(source, "pub fn value() -> bool { true }\n").unwrap();
        fs::create_dir_all(root.join(".config")).unwrap();
        fs::write(root.join(CONFIG_PATH), content).unwrap();
    }

    #[test]
    fn suite_command_is_package_file_and_unit_test_scoped() {
        let root = tempdir().unwrap();
        write_fixture(
            root.path(),
            r#"
[[suite]]
name = "small"
package = "example"
files = ["crates/example/src/lib.rs"]
test_filters = ["test(/tests::value/)"]
timeout_seconds = 30
"#,
        );
        let config = MutationConfig::load(root.path()).unwrap();
        let suite = config.named("small").unwrap();
        let command = suite.command(root.path(), Path::new("out"), 2);
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|args| args == ["--package", "example"]));
        assert!(
            args.windows(2)
                .any(|args| args == ["--file", "crates/example/src/lib.rs"])
        );
        assert!(args.contains(&"--cargo-test-arg=--lib".to_string()));
        assert!(args.contains(&"--cargo-test-arg=test(/tests::value/)".to_string()));
        assert!(!args.contains(&"--workspace".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("--test-workspace")));
    }

    #[test]
    fn rejects_unscoped_or_escaping_files() {
        let root = tempdir().unwrap();
        write_fixture(
            root.path(),
            r#"
[[suite]]
name = "small"
package = "example"
files = ["../outside.rs"]
test_filters = ["test(/tests::value/)"]
timeout_seconds = 30
"#,
        );

        assert!(MutationConfig::load(root.path()).is_err());
    }
}
