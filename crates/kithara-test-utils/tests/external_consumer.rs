#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Output},
    thread,
};

use kithara_platform::time::{Duration, WallInstant};

fn run(command: &mut Command, directory: &Path, label: &str, seconds: u64) -> Output {
    let stdout = directory.join(format!("{label}.stdout"));
    let stderr = directory.join(format!("{label}.stderr"));
    eprintln!("external-runtime {label}: {command:?}");
    let mut child = command
        .stdout(File::create(&stdout).expect("create stdout capture"))
        .stderr(File::create(&stderr).expect("create stderr capture"))
        .spawn()
        .expect("spawn fixture command");
    let deadline = WallInstant::now() + Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll fixture command") {
            break status;
        }
        if WallInstant::now() >= deadline {
            child.kill().expect("kill timed out command");
            child.wait().expect("reap timed out command");
            panic!("external-runtime {label} exceeded {seconds}s; evidence: {directory:?}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    Output {
        status,
        stdout: fs::read(stdout).expect("read stdout capture"),
        stderr: fs::read(stderr).expect("read stderr capture"),
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("tool stdout is UTF-8")
}

fn forbidden_paths(tree: &str) -> Vec<String> {
    let mut path = Vec::new();
    let mut forbidden = Vec::new();
    for line in tree.lines() {
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        let depth: usize = line[..digits].parse().expect("cargo tree depth");
        let package = line[digits..]
            .split_whitespace()
            .next()
            .expect("cargo tree package");
        path.truncate(depth);
        path.push(package);
        if matches!(
            package,
            "kithara-workspace-hack"
                | "kithara-net"
                | "kithara-events"
                | "kithara-audio"
                | "kithara-play"
                | "reqwest"
                | "wreq"
                | "boring"
                | "boring2"
                | "cpal"
                | "firewheel"
                | "usdt"
        ) {
            forbidden.push(path.join(" -> "));
        }
    }
    assert!(!path.is_empty(), "empty dependency closure");
    forbidden
}

fn prepare(consumer_root: &Path, target: &Path, directory: &Path) -> PathBuf {
    let owner = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = owner.join("tests/fixtures/external-runtime");
    let consumer = consumer_root.join("consumer");
    fs::create_dir_all(consumer.join("src")).expect("create consumer source directory");
    fs::copy(fixture.join("src/lib.rs"), consumer.join("src/lib.rs")).expect("copy fixture source");
    fs::copy(fixture.join("Cargo.lock"), consumer.join("Cargo.lock")).expect("copy fixture lock");
    let mut manifest =
        fs::read_to_string(fixture.join("Cargo.toml")).expect("read fixture manifest");
    let revision = std::env::var("KITHARA_EXTERNAL_RUNTIME_REV").ok();
    let source = revision.as_ref().map(|revision| {
        assert!(revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()));
        let repository = std::env::var("KITHARA_EXTERNAL_RUNTIME_GIT_URL")
            .expect("a pin needs its repository URL");
        let url = url::Url::parse(&repository).expect("valid repository URL");
        assert!(
            url.password().is_none() && (url.scheme() == "ssh" || url.username().is_empty()),
            "git credentials must stay outside the fixture manifest"
        );
        format!("git = {repository:?}, rev = {revision:?}")
    });
    for (relative, package) in [
        ("../../..", "kithara-test-utils"),
        ("../../../../kithara-platform", "kithara-platform"),
        ("../../../../kithara-config", "kithara-config"),
        ("../../../../kithara-derive", "kithara-derive"),
    ] {
        let dependency = source.as_ref().map_or_else(
            || {
                let path = owner
                    .parent()
                    .expect("crate parent directory")
                    .join(package);
                format!("path = {path:?}")
            },
            Clone::clone,
        );
        manifest = manifest.replace(&format!("path = \"{relative}\""), &dependency);
    }
    fs::write(consumer.join("Cargo.toml"), manifest).expect("write consumer manifest");
    if revision.is_some() {
        success(run(
            cargo(&consumer, target, &["generate-lockfile"])
                .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true"),
            directory,
            "pin-lock",
            300,
        ));
    }
    success(run(
        &mut cargo(&consumer, target, &["fetch", "--locked"]),
        directory,
        "fetch",
        300,
    ));
    fs::copy(consumer.join("Cargo.toml"), directory.join("Cargo.toml")).expect("record manifest");
    fs::copy(consumer.join("Cargo.lock"), directory.join("Cargo.lock")).expect("record lock");
    let revision = success(run(
        Command::new("git")
            .arg("-C")
            .arg(owner)
            .args(["rev-parse", "HEAD"]),
        directory,
        "source-revision",
        30,
    ));
    eprintln!(
        "external-runtime source: {}; pin: {:?}; evidence: {directory:?}",
        revision.trim(),
        std::env::var("KITHARA_EXTERNAL_RUNTIME_REV").ok()
    );
    consumer
}

fn cargo(consumer: &Path, target: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(consumer)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTDOCFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .args(args);
    command
}

fn external_runtime_closure(consumer: &Path, target: &Path, directory: &Path) {
    success(run(
        cargo(
            consumer,
            target,
            &[
                "metadata",
                "--locked",
                "--all-features",
                "--format-version",
                "1",
            ],
        )
        .env("CARGO_NET_OFFLINE", "true"),
        directory,
        "metadata",
        300,
    ));
    let mut forbidden = Vec::new();
    for triple in [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "wasm32-unknown-unknown",
    ] {
        for features in ["", "devtools", "mock,multithread,hang,devtools"] {
            let label = format!("closure-{triple}-{features}");
            let tree = success(run(
                cargo(
                    consumer,
                    target,
                    &[
                        "tree",
                        "--locked",
                        "--edges",
                        "normal,build",
                        "--prefix",
                        "depth",
                        "--format",
                        "{p} {f}",
                        "--target",
                        triple,
                        "--features",
                        features,
                    ],
                )
                .env("CARGO_NET_OFFLINE", "true"),
                directory,
                &label,
                300,
            ));
            forbidden.extend(forbidden_paths(&tree));
            eprintln!(
                "{label}: {} normal/build nodes (including host macros)",
                tree.lines().count()
            );
        }
    }
    forbidden.sort();
    forbidden.dedup();
    assert!(
        forbidden.is_empty(),
        "forbidden closure:\n{}",
        forbidden.join("\n")
    );
}

#[test]
#[ignore = "independent builds; run with just test external-runtime"]
fn external_runtime() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let target_root =
        workspace.join(std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into()));
    fs::create_dir_all(target_root.join("hang")).unwrap();
    let consumer_directory = tempfile::tempdir().unwrap();
    let target_directory = tempfile::Builder::new()
        .prefix("external-test-runtime-")
        .tempdir_in(&target_root)
        .unwrap();
    let directory = tempfile::Builder::new()
        .prefix("external-test-runtime-")
        .tempdir_in(target_root.join("hang"))
        .unwrap()
        .keep();
    let target = target_directory.path();
    let consumer = prepare(consumer_directory.path(), target, &directory);
    external_runtime_closure(&consumer, target, &directory);
    success(run(
        Command::new("rustc").arg("-vV"),
        &directory,
        "toolchain",
        30,
    ));
    for offline in [false, true] {
        for features in ["", "mock", "multithread", "hang", "devtools"] {
            let label = format!("native-{features}-offline-{offline}");
            let mut command = cargo(
                &consumer,
                target,
                &["test", "--locked", "--features", features],
            );
            command.env("CARGO_NET_OFFLINE", offline.to_string());
            let report = success(run(&mut command, &directory, &label, 300));
            eprintln!("{label}:\n{report}");
        }
        let mut wasm = cargo(
            &consumer,
            target,
            &[
                "check",
                "--tests",
                "--locked",
                "--target",
                "wasm32-unknown-unknown",
                "--features",
                "devtools,mock",
            ],
        );
        wasm.env("CARGO_NET_OFFLINE", offline.to_string());
        success(run(
            &mut wasm,
            &directory,
            &format!("wasm-compile-only-offline-{offline}"),
            300,
        ));
    }

    let hang_target = target.join("hang");
    let mut build = cargo(
        &consumer,
        &hang_target,
        &["test", "--locked", "--features", "hang", "--no-run"],
    );
    build.env("CARGO_NET_OFFLINE", "true");
    success(run(&mut build, &directory, "hang-build", 300));
    let binaries: Vec<_> = fs::read_dir(hang_target.join("debug/deps"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension().is_none()
                && path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("external_test_runtime-")
        })
        .collect();
    assert_eq!(
        binaries.len(),
        1,
        "expected one watchdog test binary: {binaries:?}"
    );
    let mut stuck = Command::new(&binaries[0]);
    stuck
        .args([
            "--ignored",
            "--exact",
            "watchdog::hang_watchdog_fires",
            "--nocapture",
        ])
        .env("KITHARA_HANG_DUMP_DIR", &directory);
    let output = run(&mut stuck, &directory, "hang-child", 10);
    assert!(
        !output.status.success(),
        "watchdog did not reject the stuck child"
    );
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostics.contains("[HangDetector] `external-runtime-hang` no progress for 1s"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("[kithara_hang_detector] hang detected: external-runtime-hang"),
        "{diagnostics}"
    );
    let dump = fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let dump: serde_json::Value = serde_json::from_slice(&fs::read(dump).unwrap()).unwrap();
    assert_eq!(dump["schema"], "kithara.hang.v1");
    assert_eq!(dump["label"], "external-runtime-hang");
}

#[test]
fn closure_oracle_rejects_host_backend() {
    assert_eq!(
        forbidden_paths("0consumer v0.0.0\n1host-macro v0.1.0\n2cpal v0.17.0\n"),
        ["consumer -> host-macro -> cpal"]
    );
}
