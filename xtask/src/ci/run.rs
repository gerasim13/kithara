use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use kithara_devtools::Ctx;
use tracing::info;

use super::{config::CiConfig, environment::CiEnvironment, lane, process::Process};
use crate::config::KitharaExt;

/// One CI job. Lanes are deliberately narrow: a job that does one thing can be
/// retried, skipped, or read on its own, and the step that failed is the job
/// name rather than a line buried in an hour of output. Where two lanes need
/// the same build, they repeat it — the executor keeps a warm target directory,
/// so repeating is cheaper than threading artifacts between jobs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Lane {
    AppleLint,
    AppleMsrv,
    AppleTest,
    AppleTestFlashOff,
    AppleE2e,
    AppleXcframework,
    AppleSwiftTest,
    AppleIos,
    AppleIosTest,
    AppleSafari,
    LinuxSecrets,
    LinuxCheck,
    LinuxWasm,
    LinuxTest,
    LinuxCoverage,
    AndroidBuild,
    AndroidTest,
    WebChromium,
    WebFirefox,
    WebSize,
    WindowsArm64,
    WindowsX64,
    WindowsX64Build,
    DeepRtsan,
    DeepPerf,
    DeepBench,
    DepsDeny,
    DepsUnused,
    DepsFeatures,
    DepsSemver,
    Mutants,
    ReleaseXcframework,
    ReleaseDocs,
    ReleaseWasm,
    ReleaseAndroid,
    ReleasePublish,
}

/// Which shared cache a lane leases. Lanes sharing an executor share its cache,
/// so this follows the runner a job is tagged for, not the operating system it
/// happens to observe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CacheGroup {
    Macos,
    Linux,
    Windows,
    Host,
}

impl Lane {
    pub(crate) fn cache_group(self) -> CacheGroup {
        match self {
            Self::AppleLint
            | Self::AppleMsrv
            | Self::AppleTest
            | Self::AppleTestFlashOff
            | Self::AppleE2e
            | Self::AppleXcframework
            | Self::AppleSwiftTest
            | Self::AppleIos
            | Self::AppleIosTest
            | Self::AppleSafari
            | Self::DeepRtsan
            | Self::DeepPerf
            | Self::DeepBench
            | Self::ReleaseXcframework
            | Self::ReleaseDocs
            | Self::ReleaseWasm => CacheGroup::Macos,
            Self::LinuxSecrets
            | Self::LinuxCheck
            | Self::LinuxWasm
            | Self::LinuxTest
            | Self::LinuxCoverage
            | Self::WebChromium
            | Self::WebFirefox
            | Self::WebSize
            | Self::DepsDeny
            | Self::DepsUnused
            | Self::DepsFeatures
            | Self::DepsSemver
            | Self::Mutants => CacheGroup::Linux,
            Self::WindowsArm64 | Self::WindowsX64 | Self::WindowsX64Build => CacheGroup::Windows,
            Self::AndroidBuild
            | Self::AndroidTest
            | Self::ReleaseAndroid
            | Self::ReleasePublish => CacheGroup::Host,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum PipelineKind {
    Branch,
    MergeRequest,
    Quarantine,
    Main,
    Nightly,
    Weekly,
    Release,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// CI lane to execute.
    lane: Lane,
    /// Pipeline policy used by lanes with fast and full variants.
    #[arg(
        long,
        env = "KITHARA_PIPELINE_KIND",
        value_enum,
        default_value = "merge-request"
    )]
    kind: PipelineKind,
}

pub(crate) fn run(args: &RunArgs, ctx: &Ctx) -> Result<()> {
    let ext = KitharaExt::from_ctx(ctx)?;
    ext.ci.validate()?;
    let host_config = env::var_os("KITHARA_CI_HOST_CONFIG")
        .map(PathBuf::from)
        .context(
            "KITHARA_CI_HOST_CONFIG must point at the host profile installed on this executor",
        )?;
    let ci_config = CiConfig::load(&host_config, &ctx.root.join(&ext.ci.pins))?;
    let environment = CiEnvironment::prepare(ctx, &ci_config, args.lane)?;
    info!(
        lane = ?args.lane,
        kind = ?args.kind,
        cache_root = %environment.cache_root.display(),
        "CI environment prepared"
    );
    let swiftpm_cache = environment.swiftpm_cache.clone();
    let temp = environment.temp.clone();
    let process = Process::new(&ctx.root, environment.vars());
    // sccache is a daemon, and a running one keeps the cache directory it was
    // started with — a client inherits the server's configuration, not its
    // own. An executor that outlives a job therefore carries the previous
    // job's cache location into this one, and a stale location that no longer
    // exists fails every compilation while reporting only that a C compiler
    // exited 254. Retire the server so it restarts with what this job asked
    // for; the cache on disk is untouched.
    process.best_effort("sccache", &["--stop-server"], "retire the compiler cache");

    let result = match args.lane {
        Lane::AppleLint => lane::apple::lint(&process, &ci_config),
        Lane::AppleMsrv => lane::apple::msrv(&process, &ci_config),
        Lane::AppleTest => lane::apple::test(&process, &ci_config, args.kind),
        Lane::AppleTestFlashOff => lane::apple::test_flash_off(&process, &ci_config),
        Lane::AppleE2e => lane::apple::e2e(&process, &ci_config),
        Lane::AppleXcframework => lane::apple::xcframework(&process, &ci_config),
        Lane::AppleSwiftTest => lane::apple::swift_test(&process, &ci_config, &swiftpm_cache),
        Lane::AppleIos => lane::apple::ios(&process, &ci_config),
        Lane::AppleIosTest => lane::apple::ios_test(&process, &ci_config),
        Lane::AppleSafari => lane::apple::safari(&process),
        Lane::LinuxSecrets => lane::linux::secrets(&process),
        Lane::LinuxCheck => lane::linux::check(&process),
        Lane::LinuxWasm => lane::linux::wasm(&process),
        Lane::LinuxTest => lane::linux::test(&process),
        Lane::LinuxCoverage => lane::linux::coverage(&process),
        Lane::AndroidBuild => lane::android::build(&process),
        Lane::AndroidTest => lane::android::test(&process, &ci_config),
        Lane::WebChromium => lane::web::chromium(&process),
        Lane::WebFirefox => lane::web::firefox(&process),
        Lane::WebSize => lane::web::size(&process),
        Lane::WindowsArm64 => lane::windows::tests(&process, "aarch64-pc-windows-msvc"),
        Lane::WindowsX64 => lane::windows::tests(&process, "x86_64-pc-windows-msvc"),
        Lane::WindowsX64Build => lane::windows::build(&process, "x86_64-pc-windows-msvc"),
        Lane::DeepRtsan => lane::deep::rtsan(&process),
        Lane::DeepPerf => lane::deep::perf(&process),
        Lane::DeepBench => lane::deep::bench(&process),
        Lane::DepsDeny => lane::deps::deny(&process),
        Lane::DepsUnused => lane::deps::unused(&process),
        Lane::DepsFeatures => lane::deps::features(&process),
        Lane::DepsSemver => lane::deps::semver(&process),
        Lane::Mutants => lane::deps::mutants(&process),
        Lane::ReleaseXcframework => {
            super::release::xcframework(&process, ctx, &ext, &temp, args.kind)
        }
        Lane::ReleaseDocs => super::release::docs(&process, ctx, &ext),
        Lane::ReleaseWasm => super::release::wasm(&process, ctx, &ext),
        Lane::ReleaseAndroid => super::release::build_android(&process, ctx, &ext),
        Lane::ReleasePublish => super::release::publish(&process, ctx, &ext, args.kind),
    };
    process.best_effort("sccache", &["--show-stats"], "sccache statistics");
    result
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::Path};

    use clap::{Parser, ValueEnum};

    use super::{CacheGroup, Lane};
    use crate::Cli;

    /// Lane names cross a language boundary: the enum is Rust, the schedule is
    /// pipeline YAML, and nothing but this test connects them. Renaming a lane
    /// without renaming its job used to fail in CI, on the runner, minutes in.
    fn scheduled_lanes() -> BTreeSet<String> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a workspace root")
            .join(".gitlab/ci");
        let mut lanes = BTreeSet::new();
        for entry in fs::read_dir(&directory).expect("pipeline directory is readable") {
            let path = entry.expect("pipeline entry is readable").path();
            let text = fs::read_to_string(&path).expect("pipeline file is UTF-8");
            lanes.extend(
                text.split("ci run ")
                    .skip(1)
                    .filter_map(|rest| rest.split_whitespace().next())
                    .map(str::to_string),
            );
        }
        lanes
    }

    /// The image a Linux job starts in is named twice: in the reviewed pins,
    /// which every machine builds and provisions from, and in the pipeline that
    /// runs the job. Nothing but this test connects them, and a machine that
    /// has built one tag cannot serve a pipeline asking for the other — which
    /// is how a host ended up with an image its own provisioning refused.
    #[test]
    fn the_linux_pipeline_starts_the_image_the_pins_name() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask has a workspace root");
        let pins = crate::ci::config::CiPins::load(&root.join(crate::ci::config::PINS_PATH))
            .expect("the reviewed pins load");
        let common = fs::read_to_string(root.join(".gitlab/ci/common.yml"))
            .expect("the shared pipeline definition is readable");
        let declared = common
            .lines()
            .find_map(|line| line.trim().strip_prefix("image: "))
            .expect("the Linux job names an image");

        assert_eq!(
            declared, pins.linux_image,
            "the pipeline and the pins name different images"
        );
    }

    #[test]
    fn ci_lane_is_typed_and_uses_modular_names() {
        assert!(Cli::try_parse_from(["xtask", "ci", "run", "apple-lint"]).is_ok());
        assert!(
            Cli::try_parse_from(["xtask", "ci", "run", "linux-test", "--kind", "quarantine"])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["xtask", "ci", "run", "apple"]).is_err());
        assert!(Cli::try_parse_from(["xtask", "ci", "run", "unknown"]).is_err());
    }

    #[test]
    fn lanes_lease_the_cache_of_the_executor_they_run_on() {
        assert_eq!(Lane::ReleaseXcframework.cache_group(), CacheGroup::Macos);
        assert_eq!(Lane::WebFirefox.cache_group(), CacheGroup::Linux);
        assert_eq!(Lane::ReleaseAndroid.cache_group(), CacheGroup::Host);
        assert_eq!(Lane::WindowsX64Build.cache_group(), CacheGroup::Windows);
    }

    #[test]
    fn the_pipeline_schedules_every_lane_and_only_real_lanes() {
        let scheduled = scheduled_lanes();
        for name in &scheduled {
            assert!(
                Lane::from_str(name, false).is_ok(),
                "a pipeline job runs `{name}`, which is not a CI lane"
            );
        }
        for lane in Lane::value_variants() {
            let name = lane
                .to_possible_value()
                .expect("every lane is reachable from the command line");
            assert!(
                scheduled.contains(name.get_name()),
                "no pipeline job runs the {} lane",
                name.get_name()
            );
        }
    }
}
