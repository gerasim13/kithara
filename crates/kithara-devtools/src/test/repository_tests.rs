use std::{fs, path::Path, process::Command};

use tempfile::TempDir;

use super::{
    NextestAction,
    command::lane_command,
    request::TestRequest,
    resolve,
    selection::{requested, select_lane},
    tests::{args_of, envs_of, features_for},
};
use crate::common::project::{ProjectConfig, TestRunner};

#[test]
fn linux_usdt_contract_lanes_keep_their_product_feature_closures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = ProjectConfig::load(&root).expect("load repository config");
    let test = &project.test;
    let request = TestRequest::parse(&[]).expect("parse request");

    for (name, feature) in [
        ("usdt-warp", "kithara-warp-tests/usdt"),
        ("usdt-play-scheduler", "kithara-play/usdt"),
        ("usdt-hls", "kithara-hls-tests/usdt"),
        ("usdt-hls-stress", "kithara-hls-tests/usdt"),
        ("usdt-queue", "kithara-queue-tests/usdt"),
    ] {
        let features = features_for(test, name, &request).expect("features");
        assert!(features.contains(feature), "{name} keeps {feature}");
        assert!(
            !features.contains("usdt-observer"),
            "{name} no longer reaches the removed observer"
        );
    }
}

/// An argument as the snapshot prints it: quoted when the shell would
/// otherwise split it.
fn quoted(arg: &str) -> String {
    if arg.is_empty() || arg.contains(char::is_whitespace) {
        format!("'{arg}'")
    } else {
        arg.to_owned()
    }
}

/// Every lane renders the command the snapshot records, alone and under
/// the two callers the gates are: one that names a profile and one that
/// names a filterset. A change to how lanes become commands then shows
/// up as a reviewed diff instead of a lane that quietly runs something
/// else.
#[test]
fn every_test_lane_renders_the_recorded_command() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let project = ProjectConfig::load(&root).expect("load repository config");
    let callers: [&[&str]; 3] = [
        &[],
        &["--profile", "ci", "--timings"],
        &["-E", "test(seek)"],
    ];
    let mut report = String::new();
    for name in project.test.lanes.keys() {
        for caller in callers {
            let mut args = vec![format!("--lane={name}")];
            args.extend(caller.iter().map(|arg| (*arg).to_owned()));
            let request = TestRequest::parse(&args).expect("parse request");
            let name = select_lane(&project.test, &request).expect("select lane");
            let command = lane_command(&project.test, name, &request).expect("lane command");
            report.push_str(&format!("# {name} [{}]\n", caller.join(" ")));
            report.push_str(&format!("  {}", command.get_program().to_string_lossy()));
            for arg in args_of(&command) {
                report.push(' ');
                report.push_str(&quoted(&arg));
            }
            report.push('\n');
            let mut envs = envs_of(&command);
            envs.sort();
            for (key, value) in envs {
                report.push_str(&format!("    {key}={value}\n"));
            }
        }
    }
    let snapshot =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-lane-commands.txt");
    if std::env::var_os("KITHARA_UPDATE_SNAPSHOT").is_some() {
        fs::write(&snapshot, &report).expect("snapshot is writable");
    }
    let expected = fs::read_to_string(&snapshot).expect("the lane snapshot exists");
    assert_eq!(
        report, expected,
        "a test lane renders a different command than the snapshot records; \
         re-record with KITHARA_UPDATE_SNAPSHOT=1 only when the change is intended"
    );
}

/// A recorded `cargo test` call's options grouped flag by flag, `-p` spelled
/// as `--package`, in a fixed order.
fn flag_groups<S: AsRef<str>>(args: &[S]) -> Vec<(String, Option<String>)> {
    const VALUED: [&str; 6] = [
        "-p",
        "--package",
        "--exclude",
        "--test",
        "--features",
        "--profile",
    ];
    let mut groups = Vec::new();
    let mut iter = args.iter().map(AsRef::as_ref);
    while let Some(flag) = iter.next() {
        let value = VALUED
            .contains(&flag)
            .then(|| iter.next().map(str::to_owned))
            .flatten();
        let flag = if flag == "-p" { "--package" } else { flag };
        groups.push((flag.to_owned(), value));
    }
    groups.sort();
    groups
}

/// Contract 15: the pinned nextest asks `cargo test` to build exactly the
/// cargo arguments a lane derives, so the typed options are the build every
/// runner of the lane gets and the build the planner reads.
///
/// Each nextest lane lists through the pinned nextest with `CARGO` pointed at
/// a script that answers `cargo metadata` with the real cargo and records any
/// other call instead of running it.
#[cfg(unix)]
#[test]
fn the_pinned_nextest_builds_exactly_the_cargo_arguments_a_lane_derives() {
    use std::os::unix::fs::PermissionsExt;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pins: toml::Table = toml::from_str(
        &fs::read_to_string(root.join(".config/ci-pins.toml")).expect("pins are readable"),
    )
    .expect("pins are TOML");
    let pin = pins["cargo_tools"]["cargo-nextest"]
        .as_str()
        .expect("nextest is pinned");
    let version = Command::new("cargo-nextest")
        .args(["nextest", "--version"])
        .output()
        .expect("cargo-nextest is on PATH");
    assert!(
        String::from_utf8_lossy(&version.stdout).contains(pin),
        "the contract is the pinned nextest's: install it with \
         `cargo install cargo-nextest --version {pin} --locked`"
    );
    let temp = TempDir::new().expect("temp dir");
    let shim = temp.path().join("cargo");
    fs::write(
        &shim,
        "#!/bin/sh\n\
         if [ \"$1\" = metadata ] || [ \"$2\" = metadata ]; then exec \"$REAL_CARGO\" \"$@\"; fi\n\
         printf '%s\\n' \"$@\" > \"$SHIM_LOG\"\n",
    )
    .expect("write the cargo shim");
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("make the shim runnable");
    let real_cargo = std::env::var_os("CARGO").expect("the test runner names the cargo it uses");
    let project = ProjectConfig::load(&root).expect("load repository config");
    let test = &project.test;
    let lanes = test
        .lanes
        .iter()
        .filter(|(_, lane)| matches!(lane.runner, TestRunner::Nextest(_)))
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(!lanes.is_empty(), "the repository configures nextest lanes");

    let mismatch = |lane: &str| {
        let resolved = resolve(test, &requested(test, lane, None).expect("lane")).expect("resolve");
        let command = resolved
            .command(NextestAction::List, &[])
            .expect("list command");
        let log = temp.path().join(format!("{lane}.args"));
        let output = Command::new("cargo-nextest")
            .args(command.get_args())
            .envs(
                command
                    .get_envs()
                    .filter_map(|(key, value)| value.map(|value| (key, value))),
            )
            .env("CARGO", &shim)
            .env("REAL_CARGO", &real_cargo)
            .env("SHIM_LOG", &log)
            .current_dir(&root)
            .output()
            .expect("run the pinned nextest");
        let Ok(recorded) = fs::read_to_string(&log) else {
            return Some(format!(
                "{lane}: nextest never called cargo test: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        };
        let args = recorded
            .lines()
            .filter(|arg| !arg.starts_with("--color"))
            .collect::<Vec<_>>();
        let build = [
            "test",
            "--no-run",
            "--message-format",
            "json-render-diagnostics",
        ];
        let Some(options) = args.strip_prefix(build.as_slice()) else {
            return Some(format!("{lane}: nextest asked cargo for {args:?}"));
        };
        let derived = resolved.cargo_args("--profile", false);
        (flag_groups(options) != flag_groups(&derived)).then(|| {
            format!(
                "{lane}: nextest asked cargo test for {options:?}; the lane derives {derived:?}"
            )
        })
    };
    let mismatch = &mismatch;
    let failures = std::thread::scope(|scope| {
        let workers = lanes
            .chunks(lanes.len().div_ceil(4))
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .copied()
                        .filter_map(mismatch)
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("a lane worker finishes"))
            .collect::<Vec<_>>()
    });

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
