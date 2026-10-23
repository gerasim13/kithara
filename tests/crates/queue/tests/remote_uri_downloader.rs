#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    assets::{AssetStore, StorageBackend},
    host::HostConfig,
    platform::time::Duration,
    play::{PlayWorker, PlayWorkerConfig, ResourcePrep},
    queue::{Queue, QueueConfig, QueueEvent, TrackSource, TrackStatus, Transition},
};
use kithara_integration_tests::{
    TestServerHelper,
    bufpool_ext::pools,
    offline::{OfflineQueue, QueueTicker, RENDER_PACE},
};
use kithara_test_fixtures::SignalAsset;

#[kithara::test(tokio, flash(false), timeout(Duration::from_secs(2)))]
async fn a_remote_uri_track_takes_the_queue_downloader() {
    let server = TestServerHelper::new().await;
    let url = server.signal(SignalAsset::MP3_SINE1K_48K_1S);
    let region = pools();
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(
            PlayWorkerConfig::builder(region.clone()).build(),
        ))
        .build();
    let queue = Queue::new(
        QueueConfig::builder()
            .prep(prep)
            .store(
                AssetStore::builder(region.clone())
                    .backend(StorageBackend::Memory)
                    .build(),
            )
            .build(),
    );
    let queue = OfflineQueue::paced(HostConfig::offline(region).build(), queue, RENDER_PACE)
        .await
        .expect("offline queue");
    let ticker = QueueTicker::spawn(queue.control(), Duration::from_millis(10));
    let mut events = queue.subscribe::<QueueEvent>();
    let id = queue
        .run(move |q| q.append(TrackSource::Uri(url.to_string())))
        .await
        .expect("append remote URI");
    let _ = queue.run(move |q| q.select(id, Transition::None)).await;
    loop {
        let envelope = events.recv().await.expect("queue stays open");
        if let QueueEvent::TrackStatusChanged {
            id: changed,
            status,
        } = envelope.event
        {
            if changed != id {
                continue;
            }
            match status {
                TrackStatus::Failed(error) => {
                    assert!(error.contains("requires a Downloader"), "{error}");
                    break;
                }
                TrackStatus::Loaded => panic!("a remote URI loaded without a Downloader"),
                _ => {}
            }
        }
    }
    drop(ticker);
}
