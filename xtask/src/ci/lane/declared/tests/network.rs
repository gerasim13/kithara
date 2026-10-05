use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    future::pending,
    process::Command,
    sync::mpsc,
    time::{Duration, Instant},
};

use axum::{Router, http::StatusCode, routing::get};
use kithara_devtools::common::tools::ToolsConfig;
use kithara_platform::tokio::{runtime::Builder, task::spawn_blocking};
use kithara_test_utils::TestHttpServer;

use super::super::run;
use crate::{
    child,
    ci::{
        config::{fixture, workspace_root},
        process::Process,
        run::PipelineKind,
    },
    config::KitharaExt,
};

const STALL_URL: &str = "KITHARA_TEST_DEPS_DENY_STALL_URL";
const LANE_PROBE: &str = "ci::lane::declared::tests::network::deps_deny_stall_probe";
const HTTP_PROBE: &str = "ci::lane::declared::tests::network::deps_deny_http_probe";

#[test]
fn a_deps_deny_network_stall_fails_within_the_declared_lane_deadline() {
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let (request, received) = mpsc::channel();
        let router = Router::new().route(
            "/stalled-advisories",
            get(move || {
                let request = request.clone();
                async move {
                    request.send(()).unwrap();
                    pending::<()>().await;
                    StatusCode::OK
                }
            }),
        );
        let server = TestHttpServer::new(router).await;
        let url = server.url("stalled-advisories").to_string();
        let started = Instant::now();
        let output = spawn_blocking(move || {
            child::output(
                Command::new(env::current_exe().unwrap())
                    .args(["--ignored", "--exact", LANE_PROBE, "--nocapture"])
                    .env(STALL_URL, url),
                None,
                Duration::from_secs(70),
            )
        })
        .await
        .unwrap();

        received
            .try_recv()
            .expect("the dependency step must send a real HTTP request before it stalls");
        println!("observed a deps-deny HTTP request held without a response");
        let output = output.expect(
            "the deps-deny lane outlived its one-minute deadline and required the outer backstop",
        );
        assert!(
            output.status.success(),
            "the lane probe failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(started.elapsed() < Duration::from_secs(70));
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("licence and advisory gate"),
            "the timed-out dependency step must remain visible in the failure report",
        );
    });
}

#[test]
#[ignore = "subprocess fixture for the real deps-deny lane deadline"]
fn deps_deny_stall_probe() {
    let root = workspace_root();
    let mut ext = KitharaExt::load(root).unwrap();
    let lane = ext.ci.lanes.get_mut("deps-deny").unwrap();
    assert_eq!(lane.timeout_minutes, 30);
    assert_eq!(lane.os, ["linux"]);
    assert_eq!(lane.program, "just");
    assert_eq!(lane.steps.len(), 1);
    assert_eq!(lane.steps[0].args, ["deps", "deny"]);
    assert_eq!(lane.steps[0].label, "licence and advisory gate");

    lane.timeout_minutes = 1;
    lane.os = vec![env::consts::OS.to_owned()];
    lane.steps[0].args = ["--ignored", "--exact", HTTP_PROBE, "--nocapture"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let tools: ToolsConfig = serde_json::from_value(serde_json::json!({
        "just": { "program": env::current_exe().unwrap() },
        "rc": { "program": root.join("missing-deps-deny-fixture-cache-client") },
    }))
    .unwrap();
    let process = Process::new(
        root,
        BTreeMap::from([(
            OsString::from(STALL_URL),
            env::var_os(STALL_URL).unwrap(),
        )]),
    );
    let started = Instant::now();
    let error = run(
        &process,
        lane,
        &fixture().pins,
        &tools,
        PipelineKind::Quarantine,
        None,
    )
    .expect_err("a dependency request that never receives a response must fail");
    let error = format!("{error:#}");
    assert!(error.contains("deadline"), "{error}");
    assert!(error.contains("licence and advisory gate"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(65));
    println!("{error}");
}

#[test]
#[ignore = "HTTP client subprocess held by the dependency-audit fixture"]
fn deps_deny_http_probe() {
    let url = env::var(STALL_URL).unwrap();
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .unwrap();
    let _response = client.get(url).send().unwrap();
    panic!("the stalled dependency endpoint must never answer");
}
