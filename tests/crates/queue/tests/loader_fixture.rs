#![cfg(not(target_arch = "wasm32"))]

use kithara::{
    events::{EventReceiver, TrackId},
    platform::time::{self, Duration},
    queue::{QueueControl, QueueEvent, TrackStatus},
};
use kithara_integration_tests::{event::TestEvent, offline::OfflinePlayerHarness};
use kithara_test_fixtures::asset::Asset;

use crate::bufpool_ext::TestPools;

/// The queue source naming a fixture track on disk.
pub(crate) fn source(track: &Asset) -> String {
    track
        .path()
        .expect("a queue fixture track lives on disk")
        .to_string_lossy()
        .into_owned()
}

pub(crate) async fn append_loaded(
    harness: &OfflinePlayerHarness,
    queue: &QueueControl<TestPools>,
    track: &Asset,
) -> TrackId {
    append_source_loaded(harness, queue, source(track)).await
}

pub(crate) async fn append_source_loaded(
    harness: &OfflinePlayerHarness,
    queue: &QueueControl<TestPools>,
    source: String,
) -> TrackId {
    let mut events: EventReceiver<TestEvent> = queue.subscribe();
    let id = harness
        .run(queue, move |q| q.append(source))
        .await
        .expect("append local WAV through Queue loader");
    wait_loaded(&mut events, id).await;
    id
}

pub(crate) async fn wait_loaded(events: &mut EventReceiver<TestEvent>, id: TrackId) {
    let loaded = time::timeout(Duration::from_secs(20), async {
        while let Ok(envelope) = events.recv().await {
            if matches!(
                envelope.event,
                TestEvent::Queue(QueueEvent::TrackStatusChanged {
                    id: seen,
                    status: TrackStatus::Loaded,
                }) if seen == id
            ) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(loaded, "local WAV fixture {id:?} must load through Queue");
}
