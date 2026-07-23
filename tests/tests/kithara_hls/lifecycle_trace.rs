#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs::{self, File as FsFile},
    io::{self, BufWriter, Write},
    path::Path,
    process::Command,
};

use kithara::{
    hls::{AbrMode, Hls, HlsConfig},
    platform::{CancelToken, time::Duration, tokio::task::spawn_blocking},
    stream::Stream,
};
use kithara_integration_tests::{PackagedTestServer, TestTempDir, temp_dir};
use kithara_test_utils::probe::capture::{ProbeEvent, install as install_recorder};
use serde_json::json;

use crate::lifecycle_trace::{assert_asset_lifecycle, capture_file_remote};

const TRACE_DIR: &str = "/Volumes/Render/dev/multistream-read/s1-traces";

#[derive(Clone, Copy)]
enum TraceScenario {
    FileRemote,
    PackagedAes128,
    PackagedPlain3,
}

impl TraceScenario {
    fn name(self) -> &'static str {
        match self {
            Self::FileRemote => "file-remote",
            Self::PackagedAes128 => "packaged-aes128",
            Self::PackagedPlain3 => "packaged-plain3",
        }
    }
}

fn event_seq(event: &ProbeEvent) -> u64 {
    event.seq().expect("lifecycle probe must carry seq")
}

async fn capture_hls(url: url::Url, root: &Path) -> Vec<ProbeEvent> {
    fs::create_dir_all(root).expect("create HLS trace cache root");
    let recorder = install_recorder();
    let config = HlsConfig::for_url(url)
        .store(kithara_integration_tests::disk_asset_store(root))
        .cancel(CancelToken::never())
        .initial_abr_mode(AbrMode::manual(0))
        .build();
    let mut stream = Stream::<Hls>::new(config)
        .await
        .expect("create packaged HLS stream");
    let reader = spawn_blocking(move || io::copy(&mut stream, &mut io::sink()));
    let committed = recorder
        .wait_for_probe_async(
            |event| event.target == "kithara_assets_probe" && event.probe_name() == Some("commit"),
            Duration::from_secs(15),
        )
        .await;
    assert!(
        committed.is_some(),
        "packaged HLS fixture must reach a commit attempt"
    );
    let copied = reader
        .await
        .expect("packaged HLS reader task")
        .expect("drain packaged HLS stream");
    assert!(copied > 0, "packaged HLS fixture must yield bytes");
    recorder.snapshot()
}

fn host_name() -> String {
    let output = Command::new("hostname")
        .output()
        .expect("execute hostname for trace file name");
    assert!(output.status.success(), "hostname command must succeed");
    let hostname = String::from_utf8(output.stdout).expect("hostname must be UTF-8");
    let hostname = hostname.trim();
    assert!(!hostname.is_empty(), "hostname must not be empty");
    hostname
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn write_trace(scenario: &str, hostname: &str, events: &[ProbeEvent]) -> io::Result<()> {
    let trace_dir = Path::new(TRACE_DIR);
    fs::create_dir_all(trace_dir)?;
    let path = trace_dir.join(format!("{scenario}-{hostname}.jsonl"));
    let mut writer = BufWriter::new(FsFile::create(path)?);
    let mut ordered = events.to_vec();
    ordered.sort_by_key(event_seq);

    ordered.into_iter().try_for_each(|event| {
        let fields: BTreeMap<&str, u64> = event
            .fields
            .iter()
            .map(|(key, value)| (*key, *value))
            .collect();
        let string_fields: BTreeMap<&str, &str> = event
            .string_fields
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let line = json!({
            "target": event.target.as_str(),
            "probe_name": event.probe_name(),
            "fields": fields,
            "string_fields": string_fields,
        });
        serde_json::to_writer(&mut writer, &line).map_err(io::Error::other)?;
        writer.write_all(b"\n")
    })?;
    writer.flush()
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "2")
)]
async fn asset_lifecycle_trace_packaged_plain3_invariants(temp_dir: TestTempDir) {
    let server = PackagedTestServer::new().await;
    let events = capture_hls(server.url("/master.m3u8"), temp_dir.path()).await;

    assert_asset_lifecycle(&events, "packaged-plain3");
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(20)),
    env(KITHARA_HANG_TIMEOUT_SECS = "2")
)]
async fn asset_lifecycle_trace_packaged_aes128_invariants(temp_dir: TestTempDir) {
    let server = PackagedTestServer::new().await;
    let events = capture_hls(server.url("/master-encrypted.m3u8"), temp_dir.path()).await;

    assert_asset_lifecycle(&events, "packaged-aes128");
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(25)),
    env(KITHARA_HANG_TIMEOUT_SECS = "3")
)]
#[ignore = "diagnostic: writes trace dump; run with --run-ignored"]
#[case::file_remote(TraceScenario::FileRemote)]
#[case::packaged_plain3(TraceScenario::PackagedPlain3)]
#[case::packaged_aes128(TraceScenario::PackagedAes128)]
async fn asset_lifecycle_trace_dump(temp_dir: TestTempDir, #[case] scenario: TraceScenario) {
    let events = match scenario {
        TraceScenario::FileRemote => capture_file_remote(&temp_dir).await,
        TraceScenario::PackagedAes128 => {
            let server = PackagedTestServer::new().await;
            capture_hls(server.url("/master-encrypted.m3u8"), temp_dir.path()).await
        }
        TraceScenario::PackagedPlain3 => {
            let server = PackagedTestServer::new().await;
            capture_hls(server.url("/master.m3u8"), temp_dir.path()).await
        }
    };
    let hostname = host_name();

    write_trace(scenario.name(), &hostname, &events).expect("write lifecycle trace");
}
