#![cfg(not(target_arch = "wasm32"))]

use std::io::Read;

use kithara::{
    events::AbrMode,
    hls::{Hls, HlsConfig},
    platform::{CancelToken, time::Duration, tokio::task::spawn_blocking},
    stream::Stream,
};
use kithara_integration_tests::{
    TestTempDir,
    fixture_protocol::DelayRule,
    hls_server::{HlsTestServer, HlsTestServerConfig},
    kithara, rt_cancel, temp_dir,
};

const STORM_ROUNDS: usize = 10;
const VARIANT_COUNT: usize = 3;
const SEGMENTS_PER_VARIANT: usize = 12;
const FIRST_SEGMENT_DELAY_MS: u64 = 2_000;
const BANDWIDTHS: [u64; VARIANT_COUNT] = [64_000, 192_000, 512_000];
const CAPS: [Option<u64>; VARIANT_COUNT] = [Some(96_000), Some(256_000), None];

/// LABA-429: toggling auto/manual ABR and bandwidth caps while the first
/// segment is buffering must not wedge the stream; a subsequent read must
/// still produce bytes.
#[kithara::test(tokio, timeout(Duration::from_secs(120)))]
async fn abr_mode_storm_does_not_wedge_loading(temp_dir: TestTempDir, rt_cancel: CancelToken) {
    let server = HlsTestServer::new(HlsTestServerConfig {
        variant_count: VARIANT_COUNT,
        segments_per_variant: SEGMENTS_PER_VARIANT,
        variant_bandwidths: Some(BANDWIDTHS.to_vec()),
        delay_rules: vec![DelayRule {
            segment_eq: Some(0),
            delay_ms: FIRST_SEGMENT_DELAY_MS,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await;

    let config = HlsConfig::for_url(server.url("/master.m3u8"))
        .store(kithara_integration_tests::disk_asset_store(temp_dir.path()))
        .cancel(rt_cancel)
        .initial_abr_mode(AbrMode::Auto(None))
        .build();
    let mut stream = Stream::<Hls>::new(config).await.expect("create HLS stream");
    let handle = stream
        .abr_handle()
        .expect("HLS stream must expose an ABR handle");
    assert_eq!(
        handle.variants().len(),
        VARIANT_COUNT,
        "precondition: the ABR storm requires three variants"
    );

    let read = spawn_blocking(move || {
        let mut buffer = [0u8; 64 * 1024];
        stream.read(&mut buffer)
    });

    for round in 0..STORM_ROUNDS {
        let variant = round % VARIANT_COUNT;
        let mode = if round % 2 == 0 {
            AbrMode::manual(variant)
        } else {
            AbrMode::Auto(None)
        };
        handle
            .set_mode(mode)
            .expect("storm variant must exist in the fixture");
        handle.set_max_bandwidth_bps(CAPS[variant]);
        kithara::platform::time::sleep(Duration::from_millis(100)).await;
    }
    handle
        .set_mode(AbrMode::Auto(None))
        .expect("return to automatic ABR");
    handle.set_max_bandwidth_bps(None);

    let bytes = read
        .await
        .expect("read task panicked")
        .expect("read after the ABR storm must not error");
    assert!(
        bytes > 0,
        "LABA-429: no bytes arrived after the ABR mode and bitrate-cap storm"
    );
}
