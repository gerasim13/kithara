#![cfg(not(target_arch = "wasm32"))]

//! A `<segment>.tmp` left behind by a dead writer must not brick the segment.
//!
//! `AtomicChunked::drop` documents the contract this test pins: "a `kill -9`
//! skips `Drop` entirely, in which case the next `AtomicChunked::open` over the
//! same canonical path wipes the stale temp". `open_with_barrier` does not do
//! that. It claims the tmp with `OpenOptions::create_new`, so an orphan from a
//! process that is long gone is indistinguishable from a live sibling writer
//! and every open returns `StorageError::TmpClaimed` forever.
//!
//! HLS then turns that permanent error into silence. `emit_fetch_cmd` logs the
//! acquire failure at DEBUG as "variant switch in flight", reverts the slot to
//! `Missing` and `dispatch_from` pushes the fetch back on the queue — so the
//! slot stays `planned`, `range_wait_phase` keeps reporting `WaitingDemand`,
//! `range_has_failed` stays false, and the decode gate parks for good. No
//! error, no event, no EOF: playback stops mid-track and nothing happens.
//!
//! Field incident (2026-08-20): four 1 MiB `.tmp` orphans from the previous
//! day sat next to the committed segments of one variant. Playback crossed the
//! three committed segments and died at the first orphaned one, permanently,
//! across app restarts.

use std::{fs, io::Read, num::NonZeroUsize, path::PathBuf};

use kithara::{
    assets::{AssetResource, AssetSource, AssetStore, StorageBackend},
    hls::{AbrMode, Hls, HlsConfig},
    platform::{CancelToken, time::Duration, tokio::task::spawn_blocking},
    stream::Stream,
};
use kithara_integration_tests::{
    TestTempDir,
    hls_server::{HlsTestServer, HlsTestServerConfig},
};

struct Consts;
impl Consts {
    const SEGMENT_SIZE: usize = 50_000;
    const SEGMENT_COUNT: usize = 5;
    /// Mid-playlist, so playback has to cross committed segments first and the
    /// stall lands where a listener hears it: in the middle of the track.
    const STALE_SEGMENT: usize = 3;
    const READ_CHUNK: usize = 8 * 1024;
}

#[kithara::test(tokio, serial, timeout(Duration::from_secs(15)), hang_timeout_secs(1))]
async fn stale_tmp_from_a_dead_writer_does_not_brick_a_segment() {
    let temp_dir = TestTempDir::new();
    let server = HlsTestServer::new(HlsTestServerConfig {
        segment_size: Consts::SEGMENT_SIZE,
        segments_per_variant: Consts::SEGMENT_COUNT,
        ..Default::default()
    })
    .await;
    let master_url = server.url("/master.m3u8");
    let stale_url = server.url(&format!("/seg/v0_{}.bin", Consts::STALE_SEGMENT));

    let root = temp_dir.path().to_path_buf();
    let store = AssetStore::builder()
        .backend(StorageBackend::Disk { root: root.clone() })
        .cache_capacity(NonZeroUsize::new(256).unwrap())
        .build();

    // Derive the segment's on-disk path the way the store does, so the orphan
    // cannot be planted somewhere the store never looks. The two assertions
    // after the read prove the derivation was the real one.
    let scope = store
        .scope::<Hls>(&AssetSource::Remote {
            url: master_url.clone(),
            discriminator: None,
        })
        .expect("hls asset scope");
    let key = scope
        .key(&AssetResource::Url(stale_url))
        .expect("stale segment key");
    let canonical = root
        .join(key.asset_root().expect("relative asset root"))
        .join(key.rel_path().expect("relative resource path"));
    let orphan = orphan_tmp_path(&canonical);

    fs::create_dir_all(orphan.parent().expect("orphan parent")).expect("create segment directory");
    fs::write(&orphan, []).expect("plant orphan tmp");

    let config = HlsConfig::for_url(master_url)
        .store(store)
        .cancel(CancelToken::never())
        .initial_abr_mode(AbrMode::manual(0))
        .build();
    let mut stream = Stream::<Hls>::new(config).await.expect("create stream");

    let read_bytes = spawn_blocking(move || {
        let mut buf = vec![0u8; Consts::READ_CHUNK];
        let mut total = 0u64;
        loop {
            let n = stream
                .read(&mut buf)
                .expect("sequential read must not fail");
            if n == 0 {
                break;
            }
            total += n as u64;
        }
        total
    })
    .await
    .expect("blocking read task");

    assert_eq!(
        read_bytes,
        server.total_bytes(),
        "an orphan tmp left by a dead writer stopped playback at segment {}",
        Consts::STALE_SEGMENT
    );
    assert!(
        canonical.is_file(),
        "segment {} never landed at {}",
        Consts::STALE_SEGMENT,
        canonical.display()
    );
    assert!(
        !orphan.exists(),
        "the planted orphan {} survived, so the store never claimed that path",
        orphan.display()
    );
}

/// The temp-file companion path `AtomicChunked` claims for a canonical path:
/// the file *name* plus `.tmp`, sibling in the same directory.
fn orphan_tmp_path(canonical: &PathBuf) -> PathBuf {
    let name = canonical
        .file_name()
        .expect("canonical file name")
        .to_str()
        .expect("utf-8 file name");
    canonical
        .parent()
        .expect("canonical parent")
        .join(format!("{name}.tmp"))
}
