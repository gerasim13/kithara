use std::{fs, path::Path};

use super::{
    command::lane_command,
    request::TestRequest,
    selection::{features_for, select_lane},
    tests::{args_of, envs_of},
};
use crate::common::project::ProjectConfig;

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
        let lane = &test.lanes[name];
        let features = features_for(test, lane, &request).expect("features");
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
            let (name, lane) = select_lane(&project.test, &request).expect("select lane");
            let command = lane_command(&project, name, lane, &request).expect("lane command");
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
