use std::num::NonZeroUsize;

use kithara_assets::{AssetStore, StorageBackend};
use kithara_audio::AudioObserverSlot;
use kithara_command::{ScopedReceipt, Seq};
use kithara_events::TrackId;
use kithara_platform::time::{Duration, timeout};
use kithara_play::{
    DeckMixSettings, DeckMixerConfig, DeckPart, DeckPass, HostedDeck, Outbox, PlayWorker,
    PlayWorkerConfig, Player, ResourceConfig, ResourceLoad, ResourcePrep, ResourceSrc,
    TrackReceipt, mock, mock::DeckRig,
};
use kithara_signal::{AudioSpec, FrameCount, SessionFrame};
pub use kithara_test_utils::bufpool::{TestPools, pools};
use kithara_test_utils::{TestTempDir, kithara};

use crate::{Queue, QueueCommand, QueueConfig, TrackSource};

/// Runs a queue operation with the fixture's deck scope and dispatcher.
///
/// # Panics
/// Panics if the fixture's deck scope is absent or the operation panics.
pub fn with_outbox<Value, Run>(
    queue: &mut Queue<TestPools>,
    rig: &mut DeckRig<TestPools>,
    run: Run,
) -> Value
where
    Run: FnOnce(&mut Queue<TestPools>, &mut Outbox<'_, TestPools>) -> Value,
{
    let output = mock::output(None).get();
    let deck = queue.deck.mixer.clone();
    let pass = DeckPass {
        mix: DeckMixSettings::default(),
        suspended: false,
        now: SessionFrame::new(0),
        delivery: FrameCount::new(128),
        output: &output,
        deck: &deck,
    };
    let mut scope = rig.ring.scope(rig.scope).expect("deck scope");
    let mut out = Outbox::new(&mut scope, &mut rig.dispatcher).in_pass(pass);
    run(queue, &mut out)
}

/// Creates two queued fixture tracks with the first selection awaiting its load.
///
/// # Panics
/// Panics if fixture creation, path conversion, or command admission fails.
#[must_use]
pub fn pending_selection() -> (
    Queue<TestPools>,
    TrackId,
    TrackId,
    DeckRig<TestPools>,
    TestTempDir,
) {
    let (mut queue, mut rig, dir) = empty_queue();
    let path = dir.path().join("entry.wav");
    let first = TrackId::allocate();
    let second = TrackId::allocate();
    with_outbox(&mut queue, &mut rig, |queue, out| {
        for id in [first, second] {
            Player::apply(
                queue,
                QueueCommand::Append {
                    id,
                    source: TrackSource::Uri(path.to_str().expect("fixture path").to_owned()),
                },
                out,
            )
            .expect("append fixture");
        }
        Player::apply(
            queue,
            QueueCommand::Select {
                id: first,
                transition: crate::Transition::None,
            },
            out,
        )
        .expect("select first");
    });
    (queue, first, second, rig, dir)
}

/// Opens the fixture source through the retained decoder dispatcher.
///
/// # Panics
/// Panics if fixture preparation or opening fails, or opening makes no progress.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub async fn loaded_fixture(
    rig: &mut DeckRig<TestPools>,
    dir: &TestTempDir,
) -> kithara_render::Loaded<kithara_play::OpenedTrack> {
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let config = ResourceConfig::for_src(ResourceSrc::Path(dir.path().join("entry.wav")))
        .store(
            AssetStore::builder(pools())
                .backend(StorageBackend::Memory)
                .build(),
        )
        .build();
    let prepared = prep
        .prepare(config, &mock::output(None).get())
        .expect("prepared fixture");
    let item = ResourceLoad::new(prepared, Box::new(AudioObserverSlot::default().relay()));
    let mut opening = std::pin::pin!(rig.load_fixture(item, Duration::ZERO));
    let mut loaded = loop {
        hang_tick!();
        if let Ok(loaded) = timeout(Duration::from_millis(10), opening.as_mut()).await {
            hang_reset!();
            break loaded.expect("opened fixture");
        }
    };
    loaded.opened.metadata.title = Some("Loaded fixture".to_owned());
    loaded
}

/// Opens and settles the queue's pending fixture load.
///
/// # Panics
/// Panics if the fixture cannot open or the pending dispatcher command cannot settle.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub async fn answer_load(
    queue: &mut Queue<TestPools>,
    rig: &mut DeckRig<TestPools>,
    dir: &TestTempDir,
) {
    hang_tick!();
    let loaded = loaded_fixture(rig, dir).await;
    hang_reset!();
    answer_loaded(queue, rig, loaded);
}

/// Settles pending dispatcher commands through the first fixture load.
///
/// # Panics
/// Panics if a command, fixture, or receipt is missing, unsupported, or stalls.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub fn answer_loaded(
    queue: &mut Queue<TestPools>,
    rig: &mut DeckRig<TestPools>,
    loaded: kithara_render::Loaded<kithara_play::OpenedTrack>,
) {
    use kithara_render::{Dispatched, DispatcherCommand};

    let mut loaded = Some(loaded);
    rig.opens.drain();
    loop {
        hang_tick!();
        let mut due = rig.opens.next_due((), 1).expect("dispatcher command");
        let opens = matches!(due.commands_mut().as_slice(), [DispatcherCommand::Load(_)]);
        match due.commands_mut().as_slice() {
            [DispatcherCommand::Load(_)] => {
                due.apply(Dispatched::Loaded(loaded.take().expect("load fixture")));
            }
            [DispatcherCommand::Release(_)] => due.apply(Dispatched::Released),
            [DispatcherCommand::SetPriority(_, _)] => due.apply(Dispatched::Prioritized),
            _ => panic!("one dispatcher command per batch"),
        }
        let receipt = rig
            .dispatcher
            .receipts()
            .next()
            .expect("dispatcher receipt");
        with_outbox(queue, rig, |queue, out| {
            Player::settle(queue, TrackReceipt::Loaded(receipt), out)
        });
        hang_reset!();
        if opens {
            break;
        }
    }
}

/// Applies and settles the pending deck entry, returning its sequence.
///
/// # Panics
/// Panics if publication, deck scope, entry batch, or receipt is missing or invalid.
pub fn answer_deck(
    queue: &mut Queue<TestPools>,
    rig: &mut DeckRig<TestPools>,
    at: SessionFrame,
) -> Seq {
    rig.ring.publish().expect("publish deck batch");
    rig.inbox.drain();
    rig.inbox
        .scope(rig.scope)
        .expect("deck scope")
        .next_due(at, 1)
        .expect("pending deck batch")
        .apply(());
    let ScopedReceipt::Scope(_, receipt) = rig.ring.receipt().expect("deck receipt") else {
        panic!("entry must answer on its deck scope");
    };
    let seq = receipt.seq();
    let (outcome, mut batch) = receipt.into();
    assert!(
        batch
            .commands
            .iter()
            .any(|part| matches!(part, DeckPart::Attach { .. } | DeckPart::Start { .. }))
    );
    with_outbox(queue, rig, |queue, out| {
        Player::settle(
            queue,
            TrackReceipt::Deck {
                seq,
                outcome: &outcome,
                batch: &mut batch,
            },
            out,
        )
    });
    seq
}

/// Creates an empty queue, command rig, and temporary PCM fixture.
///
/// # Panics
/// Panics if the PCM fixture cannot be written or the command rig cannot be created.
#[must_use]
pub fn empty_queue() -> (Queue<TestPools>, DeckRig<TestPools>, TestTempDir) {
    queue_with_load_settings(NonZeroUsize::new(3).expect("default cap"), false)
}

/// Creates an empty queue with the given loader cap and autoplay setting, its
/// command rig, and a temporary PCM fixture.
///
/// # Panics
/// Panics if the PCM fixture cannot be written or the command rig cannot be created.
#[must_use]
pub fn queue_with_load_settings(
    max_concurrent_loads: NonZeroUsize,
    should_autoplay: bool,
) -> (Queue<TestPools>, DeckRig<TestPools>, TestTempDir) {
    let dir = TestTempDir::new();
    let path = dir.path().join("entry.wav");
    mock::write_pcm_wav(&path, &[0.5; 2_048], AudioSpec::new(2, mock::SAMPLE_RATE))
        .expect("float WAV fixture");
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let store = AssetStore::builder(pools())
        .backend(StorageBackend::Memory)
        .build();
    let mut queue = Queue::new(
        QueueConfig::builder()
            .prep(prep)
            .store(store)
            .max_concurrent_loads(max_concurrent_loads)
            .should_autoplay(should_autoplay)
            .build(),
    );
    queue.clock = Some((SessionFrame::new(0), FrameCount::new(128)));
    queue.deck.mixer.sample_rate = mock::SAMPLE_RATE.get();
    let rig = DeckRig::new(DeckMixerConfig::default()).expect("fixture command ring");
    (queue, rig, dir)
}

/// Settles fixture events and ticks the queue at frame zero.
///
/// # Panics
/// Panics if the fixture's deck scope is absent or event settlement stalls.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub fn tick_fixture(queue: &mut Queue<TestPools>, rig: &mut DeckRig<TestPools>) {
    let events = rig.events.drain();
    let output = mock::output(None).get();
    let mut deck = queue.deck.mixer.clone();
    deck.metrics = rig.mixer.metrics();
    let pass = DeckPass {
        mix: DeckMixSettings::default(),
        suspended: false,
        now: SessionFrame::new(0),
        delivery: FrameCount::new(128),
        output: &output,
        deck: &deck,
    };
    let mut scope = rig.ring.scope(rig.scope).expect("deck scope");
    let mut out = Outbox::new(&mut scope, &mut rig.dispatcher).in_pass(pass);
    for event in events {
        hang_tick!();
        HostedDeck::settle(queue, TrackReceipt::Event(event), pass, &mut out);
        hang_reset!();
    }
    HostedDeck::tick(queue, pass, &mut out);
}

/// Applies the fixture deck block and settles its receipts at the supplied frame.
///
/// # Panics
/// Panics if the deck block fails, its scope is absent, or receipt settlement stalls.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub fn settle(queue: &mut Queue<TestPools>, rig: &mut DeckRig<TestPools>, at: SessionFrame) {
    for receipt in rig.block(at, 0.0).expect("deck block") {
        hang_tick!();
        let seq = receipt.seq();
        let (outcome, mut batch) = receipt.into();
        with_outbox(queue, rig, |queue, out| {
            Player::settle(
                queue,
                TrackReceipt::Deck {
                    seq,
                    outcome: &outcome,
                    batch: &mut batch,
                },
                out,
            )
        });
        hang_reset!();
    }
}

/// Settles the fixture's three entry blocks without changing their frame positions.
///
/// # Panics
/// Panics if a fixture deck block or its receipt settlement fails or stalls.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub fn finish(queue: &mut Queue<TestPools>, rig: &mut DeckRig<TestPools>) {
    for at in [0, 128, 256] {
        hang_tick!();
        settle(queue, rig, SessionFrame::new(at));
        hang_reset!();
    }
}

/// Creates a queue whose first fixture track has completed selection.
///
/// # Panics
/// Panics if fixture creation, load admission, or deck entry fails or stalls.
#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
pub async fn selected() -> (
    Queue<TestPools>,
    TrackId,
    TrackId,
    DeckRig<TestPools>,
    TestTempDir,
) {
    let (mut queue, first, second, mut rig, dir) = pending_selection();
    hang_tick!();
    answer_load(&mut queue, &mut rig, &dir).await;
    hang_reset!();
    finish(&mut queue, &mut rig);
    (queue, first, second, rig, dir)
}
