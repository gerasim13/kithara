#![cfg(not(target_arch = "wasm32"))]

use std::{
    future::{Future, poll_fn},
    num::NonZeroUsize,
    pin::{Pin, pin},
    task::Poll,
};

use kithara::{
    assets::{AssetStore, StorageBackend},
    audio::{AudioConfig, AudioObserverSlot},
    file::{File as FileSource, FileConfig, FileSrc},
    platform::time::Duration,
    play::{
        DispatcherProtocol, LoadRefusal, PlayWorker, PlayWorkerConfig, ResourceConfig,
        ResourceLoad, ResourceSrc, dispatch,
    },
};
use kithara_command::{Batch, ChannelConfig, Outcome, Receipt, Rejection, Sender, When, channel};
use kithara_render::{DispatcherCommand, LaneStart, LoadRequest};
use kithara::warp::{SpeedCurve, WarpConfig};
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

    let held = kithara_integration_tests::mock::load_audio(&worker, mp3(&worker, &temp_dir, "a.mp3"))
        .await
        .expect("the first load fits the worker");
    assert_eq!(opened(&trace), 1, "a load opens its source once");

    let refused = kithara_integration_tests::mock::load_audio(&worker, mp3(&worker, &temp_dir, "b.mp3")).await;
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
    let _reloaded = kithara_integration_tests::mock::load_audio(&worker, mp3(&worker, &temp_dir, "c.mp3"))
        .await
        .expect("a released lane frees its slot");
    assert_eq!(opened(&trace), 2, "the next load opens its own source");
}

type Loads = DispatcherProtocol<ResourceLoad<TestPools>>;

/// A one-load batch opening a local MP3 from its own file under `dir`.
fn load(worker: &PlayWorker<TestPools>, dir: &TestTempDir, name: &str) -> Batch<Loads> {
    let path = dir.write(name, tone_mp3());
    let store = AssetStore::builder(worker.pools().clone())
        .backend(StorageBackend::Disk {
            root: dir.path().join(format!("{name}.cache")),
        })
        .build();
    let warp = WarpConfig::builder().build();
    let start = LaneStart {
        speed: SpeedCurve::Constant(warp.speed()),
        keylock: warp.keylock(),
        backend: warp.backend(),
    };
    let config = ResourceConfig::for_src(ResourceSrc::Path(path))
        .store(store)
        .worker(worker.clone())
        .warp(warp)
        .build();
    Batch {
        basis: Vec::new(),
        commands: vec![DispatcherCommand::Load(LoadRequest {
            item: ResourceLoad::new(config, Box::new(AudioObserverSlot::default().relay())),
            position: Duration::ZERO,
            start,
            inbox: worker.lane_channel().1,
        })],
    }
}

/// The next receipt, driving the dispatcher until it answers one.
async fn answered(
    sender: &mut Sender<Loads>,
    mut dispatcher: Pin<&mut impl Future<Output = ()>>,
) -> Receipt<Loads> {
    poll_fn(|cx| {
        assert!(
            dispatcher.as_mut().poll(cx).is_pending(),
            "the dispatcher runs while its sender lives"
        );
        sender.receipts().next().map_or(Poll::Pending, Poll::Ready)
    })
    .await
}

#[kithara::test(tokio, timeout(Duration::from_secs(30)))]
async fn the_dispatcher_opens_each_load_once_and_answers_a_load_past_capacity_for_capacity(
    temp_dir: TestTempDir,
) {
    let worker = PlayWorker::new(
        PlayWorkerConfig::builder(pools())
            .capacity(NonZeroUsize::MIN)
            .build(),
    );
    let trace = usdt_trace::scope();
    let (mut sender, inbox) = channel::<Loads>(ChannelConfig::builder().build());
    let mut dispatcher = pin!(dispatch(inbox));

    let first = sender
        .send(When::Next, load(&worker, &temp_dir, "a.mp3"))
        .expect("the channel has room");
    let receipt = answered(&mut sender, dispatcher.as_mut()).await;
    assert_eq!(receipt.seq(), first);
    let (Outcome::Applied { data: held, .. }, _) = receipt.into() else {
        panic!("the first load fits the worker");
    };
    assert_eq!(opened(&trace), 1, "a load opens its source once");

    let second = sender
        .send(When::Next, load(&worker, &temp_dir, "b.mp3"))
        .expect("the channel has room");
    let receipt = answered(&mut sender, dispatcher.as_mut()).await;
    assert_eq!(receipt.seq(), second);
    assert!(
        matches!(
            receipt.outcome(),
            Outcome::Rejected(Rejection::Refused(LoadRefusal::Capacity { capacity: 1 }))
        ),
        "a load past capacity is answered for capacity: {:?}",
        receipt.outcome()
    );
    assert_eq!(
        opened(&trace),
        1,
        "a load the worker cannot hold opens nothing"
    );
    drop(held);
}

/// The worker refills every deck's ring, and a ring drains while its worker
/// waits for a CPU, so the worker asks the OS to run it ahead of ordinary work.
/// Whether the OS grants that is the machine's policy, not the worker's.
#[kithara::test(tokio, flash(false), timeout(Duration::from_secs(30)))]
async fn the_play_worker_asks_the_os_to_schedule_it_as_an_audio_feed() {
    let trace = usdt_trace::scope();
    let _worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());

    trace
        .wait_for(|events| {
            events
                .iter()
                .any(|event| event.probe == "thread_class" && event.field("audio_feed") == Some(1))
        })
        .await;
    drop(trace);
}
