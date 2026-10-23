#![cfg(not(target_arch = "wasm32"))]

use std::io::Read;

use kithara::{
    assets::{AssetStore, StorageBackend},
    download::{Downloader, DownloaderConfig, DownloaderEvent},
    events::EventBus,
    file::{File, FileConfig, FileSrc},
    net::{HttpClient, NetOptions},
    platform::{CancelToken, sync::Arc, time::Duration, tokio::task::spawn_blocking},
    stream::Stream,
};
use kithara_integration_tests::{
    Content, Delivery, FixtureBehavior, TestServerHelper,
    bufpool_ext::{TestPools, pools},
};

#[kithara::test(tokio, flash(false), timeout(Duration::from_secs(2)))]
async fn an_injected_downloader_carries_the_remote_file_requests() {
    let bytes: Vec<_> = (0..4096)
        .map(|i| u8::try_from(i % 251).expect("fixture byte fits"))
        .collect();
    let server = TestServerHelper::new().await;
    let fixture = server.register_behavior(FixtureBehavior {
        content: Content::StaticBytes {
            bytes: Arc::new(bytes.clone()),
            content_type: Some("audio/mpeg"),
        },
        delivery: Delivery::Range,
    });
    let pools = pools();
    let downloader = Downloader::new(
        DownloaderConfig::for_client(HttpClient::new(
            NetOptions::default(),
            pools.clone(),
            CancelToken::never(),
        ))
        .soft_timeout(Duration::ZERO)
        .build(),
    );
    let bus = EventBus::new(128);
    let mut events = bus.subscribe::<DownloaderEvent>();
    let config = FileConfig::for_src(FileSrc::Remote(fixture.url()))
        .store(
            AssetStore::builder(pools.clone())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .pools(pools)
        .downloader(downloader)
        .events(bus)
        .build();
    let mut stream = Stream::<File<TestPools>>::new(config)
        .await
        .expect("remote file opens");
    let actual = spawn_blocking(move || {
        let mut actual = Vec::new();
        stream.read_to_end(&mut actual).expect("read complete file");
        actual
    })
    .await
    .expect("reader task");
    assert_eq!(actual, bytes);
    let mut reported = false;
    while let Ok(envelope) = events.try_recv() {
        reported |= matches!(envelope.event, DownloaderEvent::LoadSlow { .. });
    }
    assert!(
        reported,
        "the injected Downloader must carry the file requests"
    );
}
