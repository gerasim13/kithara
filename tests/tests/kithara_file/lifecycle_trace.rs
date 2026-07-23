#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use kithara::{
    file::{File, FileConfig, FileSrc},
    platform::{sync::Arc, time::Duration, tokio::task::spawn_blocking},
    stream::Stream,
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, TestServerHelper, TestTempDir, temp_dir,
};
use kithara_test_utils::probe::capture::{ProbeEvent, install as install_recorder};

const NONE_U64: u64 = u64::MAX;

fn event_seq(event: &ProbeEvent) -> u64 {
    event.seq().expect("lifecycle probe must carry seq")
}

fn event_key(event: &ProbeEvent) -> u64 {
    event
        .u64("key")
        .expect("key-bearing lifecycle probe must carry key")
}

fn event_len(event: &ProbeEvent, field: &str) -> Option<u64> {
    event.u64(field).filter(|value| *value != NONE_U64)
}

pub(crate) fn assert_asset_lifecycle(events: &[ProbeEvent], scenario: &str) {
    let mut lifecycle: Vec<&ProbeEvent> = events
        .iter()
        .filter(|event| event.target == "kithara_assets_probe")
        .filter(|event| {
            matches!(
                event.probe_name(),
                Some(
                    "acquire_resource_with_ctx"
                        | "commit"
                        | "fail"
                        | "open_resource_with_ctx"
                        | "reactivate"
                        | "wrap_reader"
                        | "wrap_writer"
                )
            )
        })
        .collect();
    lifecycle.sort_by_key(|event| event_seq(event));

    assert!(
        !lifecycle.is_empty(),
        "{scenario}: expected lifecycle probes from kithara-assets"
    );

    let mut backend_accesses: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut wrapped_resources: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut pending_wrapper_seqs = Vec::new();
    let mut reactivate_seqs = Vec::new();
    let mut active_pending = 0usize;
    let mut pending_count = 0usize;
    let mut terminal_count = 0usize;
    let mut commit_count = 0usize;

    for event in lifecycle {
        let seq = event_seq(event);
        match event.probe_name() {
            Some("acquire_resource_with_ctx" | "open_resource_with_ctx") => {
                backend_accesses
                    .entry(event_key(event))
                    .or_default()
                    .push(seq);
            }
            Some("wrap_writer") => {
                wrapped_resources
                    .entry(event_key(event))
                    .or_default()
                    .push(seq);
                pending_wrapper_seqs.push(seq);
                active_pending += 1;
                pending_count += 1;
            }
            Some("wrap_reader") => {
                wrapped_resources
                    .entry(event_key(event))
                    .or_default()
                    .push(seq);
            }
            Some("commit") => {
                assert!(
                    active_pending > 0,
                    "{scenario}: commit attempt at seq {seq} has no preceding pending wrapper"
                );
                active_pending -= 1;
                terminal_count += 1;
                commit_count += 1;
            }
            Some("fail") => {
                assert!(
                    active_pending > 0,
                    "{scenario}: fail at seq {seq} has no preceding pending wrapper"
                );
                active_pending -= 1;
                terminal_count += 1;
            }
            Some("reactivate") => reactivate_seqs.push(seq),
            _ => {}
        }
    }

    assert!(
        !backend_accesses.is_empty(),
        "{scenario}: expected at least one key-bearing backend access"
    );
    for (key, access_seqs) in backend_accesses {
        let wrappers = wrapped_resources.get(&key).unwrap_or_else(|| {
            panic!("{scenario}: backend key {key} was never wrapped for production use")
        });
        for access_seq in access_seqs {
            assert!(
                wrappers.iter().any(|wrapper_seq| *wrapper_seq > access_seq),
                "{scenario}: backend key {key} at seq {access_seq} has no later wrapper"
            );
        }
    }
    for reactivate_seq in reactivate_seqs {
        assert!(
            pending_wrapper_seqs
                .iter()
                .any(|wrapper_seq| *wrapper_seq > reactivate_seq),
            "{scenario}: reactivation at seq {reactivate_seq} has no later pending wrapper"
        );
    }

    assert_eq!(
        active_pending, 0,
        "{scenario}: pending wrappers must reach a terminal operation"
    );
    assert_eq!(
        pending_count, terminal_count,
        "{scenario}: pending wrappers and terminal operations must balance globally"
    );
    assert!(
        commit_count > 0,
        "{scenario}: scenario must exercise a commit attempt"
    );
}

fn assert_file_lifecycle(events: &[ProbeEvent], scenario: &str) {
    let resource_keys: BTreeSet<u64> = events
        .iter()
        .filter(|event| event.target == "kithara_assets_probe")
        .filter(|event| event.probe_name() == Some("acquire_resource_with_ctx"))
        .map(event_key)
        .collect();
    assert_eq!(
        resource_keys.len(),
        1,
        "{scenario}: File trace must have one acquired resource key"
    );
    let resource = *resource_keys
        .first()
        .expect("File trace resource key must exist");

    let mut lifecycle: Vec<&ProbeEvent> = events
        .iter()
        .filter(|event| event.target == "kithara_file_probe")
        .filter(|event| {
            matches!(
                event.probe_name(),
                Some("build_fetch_cmd" | "finalize_fetch" | "set_phase")
            )
        })
        .collect();
    lifecycle.sort_by_key(|event| event_seq(event));

    assert!(
        !lifecycle.is_empty(),
        "{scenario}: expected File lifecycle probes for key {resource}"
    );

    let mut active_fetch: Option<(u64, u64)> = None;
    let mut first_fetch_seq = None;
    let mut last_finalize_seq = None;
    let mut last_phase = None;
    let mut complete_seq = None;
    let mut bytes_written = 0u64;

    for event in lifecycle {
        let seq = event_seq(event);
        match event.probe_name() {
            Some("build_fetch_cmd") => {
                assert!(
                    active_fetch.is_none(),
                    "{scenario}: key {resource} started overlapping File fetches"
                );
                let resume_from = event
                    .u64("resume_from")
                    .expect("build_fetch_cmd must record resume_from");
                active_fetch = Some((resume_from, seq));
                first_fetch_seq.get_or_insert(seq);
            }
            Some("finalize_fetch") => {
                let (resume_from, start_seq) = active_fetch
                    .take()
                    .expect("finalize_fetch must follow build_fetch_cmd");
                assert_eq!(
                    event.u64("resume_from"),
                    Some(resume_from),
                    "{scenario}: key {resource} finalize must preserve its start offset"
                );
                assert!(
                    start_seq < seq,
                    "{scenario}: key {resource} fetch must finalize after it starts"
                );
                bytes_written = bytes_written
                    .checked_add(
                        event
                            .u64("bytes_written")
                            .expect("finalize_fetch must record bytes_written"),
                    )
                    .expect("File trace byte total must fit u64");
                last_finalize_seq = Some(seq);
            }
            Some("set_phase") => {
                let phase = event
                    .u64("phase")
                    .expect("set_phase must record its real phase parameter");
                assert!(
                    matches!(phase, 1 | 2),
                    "{scenario}: key {resource} recorded invalid File phase {phase}"
                );
                if let Some(previous) = last_phase {
                    assert!(
                        previous <= phase,
                        "{scenario}: key {resource} File phases must be monotone"
                    );
                }
                last_phase = Some(phase);
                if phase == 2 {
                    assert!(
                        complete_seq.replace(seq).is_none(),
                        "{scenario}: key {resource} entered Complete more than once"
                    );
                }
            }
            _ => {}
        }
    }

    assert!(
        active_fetch.is_none(),
        "{scenario}: key {resource} fetch must finalize before the trace ends"
    );
    assert_eq!(
        last_phase,
        Some(2),
        "{scenario}: key {resource} must finish in Complete"
    );

    let commit_events: Vec<&ProbeEvent> = events
        .iter()
        .filter(|event| event.target == "kithara_assets_probe")
        .filter(|event| event.probe_name() == Some("commit"))
        .collect();
    assert_eq!(
        commit_events.len(),
        1,
        "{scenario}: key {resource} must have one commit attempt"
    );
    let commit = commit_events[0];
    let commit_seq = event_seq(commit);
    let final_len =
        event_len(commit, "final_len").expect("remote File commit must carry a concrete final_len");

    assert_eq!(
        bytes_written, final_len,
        "{scenario}: key {resource} finalized bytes must equal requested commit length"
    );
    assert!(
        first_fetch_seq.is_some_and(|seq| seq < commit_seq),
        "{scenario}: key {resource} fetch must start before commit"
    );
    assert!(
        last_finalize_seq.is_some_and(|seq| seq < commit_seq),
        "{scenario}: key {resource} final fetch must enter finalize before commit"
    );
    assert!(
        complete_seq.is_some_and(|seq| commit_seq < seq),
        "{scenario}: key {resource} commit must precede File Complete"
    );
}

pub(crate) async fn capture_file_remote(temp_dir: &TestTempDir) -> Vec<ProbeEvent> {
    let recorder = install_recorder();
    let helper = TestServerHelper::new().await;
    let bytes = Arc::new(
        (0..256 * 1024)
            .map(|index| u8::try_from(index % 251).expect("fixture byte"))
            .collect(),
    );
    let handle = helper.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes,
            content_type: Some("audio/mpeg"),
        },
        delivery: Delivery::Range,
    });
    let config = FileConfig::for_src(FileSrc::Remote(handle.url()))
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .build();
    let mut stream = Stream::<File>::new(config)
        .await
        .expect("create remote File stream");

    let reader = spawn_blocking(move || io::copy(&mut stream, &mut io::sink()));
    let committed = recorder
        .wait_for_probe_async(
            |event| event.target == "kithara_assets_probe" && event.probe_name() == Some("commit"),
            Duration::from_secs(10),
        )
        .await;
    assert!(
        committed.is_some(),
        "remote File fixture must reach a commit attempt"
    );
    let copied = reader
        .await
        .expect("remote File reader task")
        .expect("drain remote File stream");
    assert!(copied > 0, "remote File fixture must yield bytes");

    recorder.snapshot()
}

#[kithara::test(
    tokio,
    native,
    serial,
    timeout(Duration::from_secs(15)),
    env(KITHARA_HANG_TIMEOUT_SECS = "2")
)]
async fn asset_lifecycle_trace_file_remote_invariants(temp_dir: TestTempDir) {
    let events = capture_file_remote(&temp_dir).await;

    assert_asset_lifecycle(&events, "file-remote");
    assert_file_lifecycle(&events, "file-remote");
}
