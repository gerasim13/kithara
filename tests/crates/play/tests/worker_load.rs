#![cfg(not(target_arch = "wasm32"))]

use std::num::NonZeroUsize;

use kithara::{
    assets::{AssetStore, StorageBackend},
    audio::AudioConfig,
    file::{File as FileSource, FileConfig, FileSrc},
    platform::time::Duration,
    play::{LoadRefusal, PlayWorker, PlayWorkerConfig},
};
use kithara_integration_tests::usdt_trace::{self, Scope};
use kithara_test_fixtures::fixtures::tone_mp3;
use kithara_test_utils::{TestTempDir, temp_dir};

use crate::bufpool_ext::{TestPools, pools};

/// A local MP3 the worker opens from its own file under `dir`.
fn mp3(
    worker: &PlayWorker<TestPools>,
    dir: &TestTempDir,
    name: &str,
) -> AudioConfig<FileSource<TestPools>> {
    let path = dir.write(name, tone_mp3());
    let store = AssetStore::builder(worker.pools().clone())
        .backend(StorageBackend::Disk {
            root: dir.path().join(format!("{name}.cache")),
        })
        .build();
    let file = FileConfig::for_src(FileSrc::Local(path))
        .store(store)
        .pools(worker.pools().clone())
        .build();
    AudioConfig::<FileSource<TestPools>>::for_stream(file)
        .hint("mp3".to_string())
        .build()
}

fn opened(trace: &Scope) -> usize {
    trace.events_of("source_opened").len()
}

#[kithara::test(tokio, timeout(Duration::from_secs(30)))]
async fn a_load_opens_its_source_once_and_a_load_past_capacity_opens_nothing(
    temp_dir: TestTempDir,
) {
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools())
            .capacity(NonZeroUsize::MIN)
            .build(),
    );
    let trace = usdt_trace::scope();

    let held = worker
        .load(mp3(&worker, &temp_dir, "a.mp3"))
        .await
        .expect("the first load fits the worker");
    assert_eq!(opened(&trace), 1, "a load opens its source once");

    let refused = worker.load(mp3(&worker, &temp_dir, "b.mp3")).await;
    assert!(
        matches!(refused, Err(LoadRefusal::Capacity { capacity: 1 })),
        "a load past capacity is refused for capacity"
    );
    assert_eq!(
        opened(&trace),
        1,
        "a load the worker cannot hold opens nothing"
    );

    drop(held);
    let _reloaded = worker
        .load(mp3(&worker, &temp_dir, "c.mp3"))
        .await
        .expect("a released lane frees its slot");
    assert_eq!(opened(&trace), 2, "the next load opens its own source");
}
