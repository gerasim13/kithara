#![cfg(not(target_arch = "wasm32"))]

//! `Queue::play` loads the current item through the player, which
//! starts the audio engine first — 130-400 ms against a real output device.
//! A track load completing inside that window must not leave the track
//! `Loaded` over an emptied slot, which turns every later select of it into
//! `PlayError::ItemConsumed` — the rejection the iOS switch storm reports.
//! The finished load is posted to the queue, which runs it only once `play`
//! has returned.
//!
//! The engine-start window is a session gate here, so the interleaving is a
//! rendezvous rather than a timing window. The session is the test's own, so
//! no Host holds the queue: a test thread holds it the way a Host deck does.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    task::{Wake, Waker},
};

use kithara::{
    assets::{AssetStore, StorageBackend},
    audio::ConsumerWakeMode,
    platform::{
        sync::{
            Arc, Mutex,
            mpsc::{self, RecvTimeoutError},
        },
        thread::{JoinHandle, spawn_named},
        time::{Duration, Instant},
        tokio,
    },
    play::{
        AllocatedSlot, Cmd, NodeInputs, PlayError, PlayerConfig, PlayerImpl, Reply, ResourceConfig,
        ResourceSrc, SessionBinding, SessionDispatcher, SessionSampleRate, SharedEq, SlotId,
        player::{Player, PlayerControlSource},
    },
    queue::{Queue, QueueConfig, QueueEvent, TrackSource, Transition},
};
use kithara_integration_tests::{
    bufpool_ext::{TestPools, pools},
    event::TestEvent,
    kithara,
    test_defaults::consts as shared,
    waits::wait_for_event,
};
use kithara_render::bridge::slot_channels;
use kithara_test_fixtures::fixtures::tone_mp3;
use kithara_test_utils::{TestTempDir, temp_dir};

const TRACK_COUNT: usize = 2;
const TICK: Duration = Duration::from_millis(20);

/// Holds the first `StartPlayer` until the test releases it, standing in for
/// the audio-device stream start that makes the window wide on a real device.
struct StartGatedSession {
    gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    next_player: AtomicU64,
    next_slot: AtomicU64,
    nodes: Mutex<Vec<NodeInputs>>,
}

impl StartGatedSession {
    fn new(entered: mpsc::Sender<()>, release: mpsc::Receiver<()>) -> Self {
        Self {
            gate: Mutex::new(Some((entered, release))),
            next_player: AtomicU64::new(1),
            next_slot: AtomicU64::new(0),
            nodes: Mutex::default(),
        }
    }
}

impl SessionDispatcher<TestPools> for StartGatedSession {
    fn exec(&self, cmd: Cmd<TestPools>) -> Result<Reply, PlayError> {
        let reply = match cmd {
            Cmd::StartPlayer { .. } => {
                if let Some((entered, release)) = self.gate.lock().take() {
                    entered.send(()).expect("test holds the entered receiver");
                    release.recv().expect("test holds the release sender");
                }
                Reply::Ok
            }
            Cmd::RegisterPlayer { .. } => {
                Reply::PlayerRegistered(kithara::play::session::RegisteredPlayer {
                    id: self.next_player.fetch_add(1, Ordering::Relaxed),
                    eq: SharedEq::new(10),
                })
            }
            Cmd::AllocateSlot { .. } => {
                let slot = SlotId::new(self.next_slot.fetch_add(1, Ordering::Relaxed));
                let (inputs, control) = slot_channels(SharedEq::new(10));
                self.nodes.lock().push(inputs);
                Reply::SlotAllocated(Box::new(AllocatedSlot::new(control, slot)))
            }
            Cmd::QuerySampleRate => Reply::SampleRate(SessionSampleRate::new(
                None,
                shared::NON_ZERO_SAMPLE_RATE.get(),
            )),
            Cmd::QueryStreamShape => Reply::StreamShape(None),
            _ => Reply::Ok,
        };
        Ok(reply)
    }

    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }
}

enum Signal {
    Wake,
    Stop,
}

struct Wakes(mpsc::Sender<Signal>);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(Signal::Wake);
    }
}

/// Holds `queue` the way a Host deck thread does: runs the commands posted to
/// it on their wake and ticks it between them. `Signal::Stop` drops the queue.
fn hold(mut queue: Queue<TestPools>) -> (mpsc::Sender<Signal>, JoinHandle<()>) {
    let (signals, received) = mpsc::channel();
    let waker = Waker::from(Arc::new(Wakes(signals.clone())));
    let thread = spawn_named("queue-holder", move || {
        queue.hold(waker);
        queue.drain();
        loop {
            match received.recv_timeout(Instant::now() + TICK) {
                Ok(Signal::Wake) => queue.drain(),
                Err(RecvTimeoutError::Timeout) => {
                    Player::tick(&mut queue).expect("an open queue ticks");
                }
                Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    (signals, thread)
}

/// A local fixture per track: the load has to run and land asynchronously,
/// but nothing about this test depends on how long it takes — the gate owns
/// the ordering — so it stays off the shared test server.
#[kithara::fixture]
fn gated_paths(temp_dir: TestTempDir, tone_mp3: &'static [u8]) -> (TestTempDir, Vec<PathBuf>) {
    let paths = (0..TRACK_COUNT)
        .map(|index| {
            let path = temp_dir.path().join(format!("gated-{index}.mp3"));
            fs::write(&path, tone_mp3).expect("fixture must be writable");
            path
        })
        .collect();
    (temp_dir, paths)
}

fn resource_config(path: &Path, store: &AssetStore<TestPools>) -> ResourceConfig<TestPools> {
    ResourceConfig::for_src(
        ResourceSrc::parse(path.to_string_lossy()).expect("absolute fixture path"),
    )
    .store(store.clone())
    .build()
}

#[kithara::test(tokio, multi_thread, timeout(Duration::from_secs(180)))]
async fn a_track_play_consumed_mid_load_can_be_selected_again(
    gated_paths: (TestTempDir, Vec<PathBuf>),
) {
    let (temp_dir, paths) = gated_paths;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let session = Arc::new(StartGatedSession::new(entered_tx, release_rx));
    let pools = pools();
    let store = AssetStore::builder(pools.clone())
        .backend(StorageBackend::Disk {
            root: temp_dir.path().into(),
        })
        .build();
    let player = PlayerImpl::new(
        PlayerConfig::builder()
            .sample_rate(shared::NON_ZERO_SAMPLE_RATE)
            .worker(kithara::play::PlayWorker::new(
                kithara::play::PlayWorkerConfig::builder(pools).build(),
            ))
            .session(SessionBinding::new(session, shared::NON_ZERO_SAMPLE_RATE))
            .build(),
    );
    let queue = Queue::new(
        QueueConfig::builder()
            .player(player)
            .store(store.clone())
            .build(),
    );
    let queue_control = queue.control();
    let (holder, holder_thread) = hold(queue);
    let mut status_rx = queue_control.subscribe();

    let configs: Vec<_> = (0..TRACK_COUNT)
        .map(|index| resource_config(&paths[index], &store))
        .collect();
    let ids = tokio::task::spawn_blocking({
        let queue_control = queue_control.clone();
        move || {
            configs
                .into_iter()
                .map(|config| queue_control.append(TrackSource::Config(Box::new(config))))
                .collect::<Result<Vec<_>, _>>()
        }
    })
    .await
    .expect("append must join")
    .expect("queue is open while fixtures are appended");

    // `play` is issued while every track is still loading, exactly as the iOS
    // surface does, and parks inside the engine start.
    let playing = tokio::task::spawn_blocking({
        let queue_control = queue_control.clone();
        move || queue_control.play()
    });
    tokio::task::spawn_blocking(move || entered_rx.recv())
        .await
        .expect("gate task must join")
        .expect("play must reach the engine start");

    release_tx.send(()).expect("gate is still parked");
    playing.await.expect("play must join");

    // The first track's load, finished while play was inside the engine
    // start, is applied once play has returned.
    wait_for_event(
        &mut status_rx,
        "the first track's load landing after play left the engine start",
        |event| {
            matches!(
                event,
                TestEvent::Queue(QueueEvent::NextTrackReady { id, .. }) if *id == ids[0]
            )
        },
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|error| panic!("precondition: {error}"));

    tokio::task::spawn_blocking({
        let queue_control = queue_control.clone();
        let id = ids[1];
        move || queue_control.select(id, Transition::None)
    })
    .await
    .expect("select must join")
    .expect("selecting the second track must be accepted");
    wait_for_event(
        &mut status_rx,
        "the second track becoming current",
        |event| {
            matches!(
                event,
                TestEvent::Queue(QueueEvent::CurrentTrackChanged { id: Some(id) }) if *id == ids[1]
            )
        },
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|error| panic!("precondition: {error}"));

    tokio::task::spawn_blocking({
        let queue_control = queue_control.clone();
        let id = ids[0];
        move || queue_control.select(id, Transition::None)
    })
    .await
    .expect("select must join")
    .unwrap_or_else(|error| {
        panic!(
            "switching back to the track `play` consumed was rejected: {error} — the \
             queue still reports it as holding a resource the player no longer has"
        )
    });

    tokio::task::spawn_blocking({
        let queue_control = queue_control.clone();
        move || queue_control.clear()
    })
    .await
    .expect("clear must join");
    holder
        .send(Signal::Stop)
        .expect("the holder runs until stopped");
    tokio::task::spawn_blocking(move || holder_thread.join())
        .await
        .expect("join task must finish")
        .expect("the holder must not panic");
}
