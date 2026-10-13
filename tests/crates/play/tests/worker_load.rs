#![cfg(not(target_arch = "wasm32"))]

use std::{future::poll_fn, num::NonZeroUsize, task::Poll};

use kithara::{
    assets::{AssetStore, StorageBackend},
    audio::AudioObserverSlot,
    platform::time::Duration,
    play::{
        DispatcherProtocol, LoadRefusal, PlayWorker, PlayWorkerConfig, ResourceConfig,
        ResourceLoad, ResourceSrc,
    },
    warp::{SpeedCurve, WarpConfig},
};
use kithara_command::{Batch, ChannelConfig, Outcome, Receipt, Rejection, Sender, When, channel};
use kithara_integration_tests::usdt_trace::{self, Scope};
use kithara_render::{Dispatched, DispatcherCommand, LaneStart, LoadRequest, ServiceClass};
use kithara_test_fixtures::fixtures::tone_mp3;
use kithara_test_utils::{TestTempDir, temp_dir};

use crate::bufpool_ext::{TestPools, pools};

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

    let (mut sender, inbox) = channel::<Loads>(ChannelConfig::builder().build());
    let _dispatcher = worker
        .start_dispatcher(inbox)
        .expect("the worker owns its dispatcher");
    sender
        .send(When::Next, load(&worker, &temp_dir, "a.mp3"))
        .expect("first load batch");
    let receipt = answered(&mut sender).await;
    let (
        Outcome::Applied {
            data: Dispatched::Loaded(held),
            ..
        },
        _,
    ) = receipt.into()
    else {
        panic!("the first load fits the worker");
    };
    assert_eq!(opened(&trace), 1, "a load opens its source once");

    sender
        .send(When::Next, load(&worker, &temp_dir, "b.mp3"))
        .expect("second load batch");
    let refused = answered(&mut sender).await;
    assert!(
        matches!(
            refused.outcome(),
            Outcome::Rejected(Rejection::Refused(LoadRefusal::Capacity { capacity: 1 }))
        ),
        "a load past capacity is refused for capacity"
    );
    assert_eq!(
        opened(&trace),
        1,
        "a load the worker cannot hold opens nothing"
    );
    sender
        .send(
            When::Next,
            Batch {
                basis: Vec::new(),
                commands: vec![DispatcherCommand::Release(held.lane)],
            },
        )
        .expect("release lane batch");
    assert!(matches!(
        answered(&mut sender).await.outcome(),
        Outcome::Applied {
            data: Dispatched::Released,
            ..
        }
    ));
    sender
        .send(When::Next, load(&worker, &temp_dir, "c.mp3"))
        .expect("reload batch");
    let reloaded = answered(&mut sender).await;
    assert!(
        matches!(
            reloaded.outcome(),
            Outcome::Applied {
                data: Dispatched::Loaded(_),
                ..
            }
        ),
        "a released lane frees its slot"
    );
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
        commands: vec![DispatcherCommand::Load(Box::new(LoadRequest {
            class: ServiceClass::Warm,
            item: ResourceLoad::new(config, Box::new(AudioObserverSlot::default().relay())),
            position: Duration::ZERO,
            start,
            inbox: worker.lane_channel().1,
        }))],
    }
}

/// The next receipt from the dispatcher's configured worker owner.
#[kithara_test_utils::kithara::hang_watchdog(ctx = String)]
async fn answered(sender: &mut Sender<Loads>) -> Receipt<Loads> {
    // WHY: The dispatcher may be opening a fixture on an uncounted real thread.
    let _real_io = kithara::platform::flash::real_io();
    let budget = __hang_detector.remaining();
    let receipt = poll_fn(|cx| {
        hang_tick!("dispatcher waits for a load or priority answer".to_owned());
        sender.hold(cx.waker().clone());
        sender.receipts().next().map_or(Poll::Pending, Poll::Ready)
    });
    let receipt = kithara::platform::time::timeout(budget, receipt).await;
    sender.release();
    let receipt = receipt.expect("dispatcher answers within its watchdog budget");
    hang_reset!();
    receipt
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
    let _dispatcher = worker
        .start_dispatcher(inbox)
        .expect("the worker owns its dispatcher");

    let first = sender
        .send(When::Next, load(&worker, &temp_dir, "a.mp3"))
        .expect("the channel has room");
    let receipt = answered(&mut sender).await;
    assert_eq!(receipt.seq(), first);
    let (
        Outcome::Applied {
            data: Dispatched::Loaded(held),
            ..
        },
        _,
    ) = receipt.into()
    else {
        panic!("the first load fits the worker");
    };
    assert_eq!(opened(&trace), 1, "a load opens its source once");

    let second = sender
        .send(When::Next, load(&worker, &temp_dir, "b.mp3"))
        .expect("the channel has room");
    let receipt = answered(&mut sender).await;
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
    sender
        .send(
            When::Next,
            Batch {
                basis: Vec::new(),
                commands: vec![DispatcherCommand::SetPriority(
                    held.lane,
                    ServiceClass::Audible,
                )],
            },
        )
        .expect("prioritize resident lane");
    assert!(
        matches!(
            answered(&mut sender).await.outcome(),
            Outcome::Applied {
                data: Dispatched::Prioritized,
                ..
            }
        ),
        "the dispatcher runs while its sender lives"
    );
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

#[cfg(not(target_os = "android"))]
#[kithara::test(native, tokio, timeout(Duration::from_secs(30)))]
#[kithara_test_utils::kithara::hang_watchdog(ctx = String)]
async fn a_nonzero_initial_media_load_reaches_its_raw_source_target_without_reopening() {
    use std::num::NonZeroU32;

    use kithara::{
        audio::{
            AudioDecoderConfig, AudioObserverSlot, DecoderBackend as ObservedBackend,
            DecoderChangeCause, DecoderEvent,
        },
        decode::{DecoderBackend, GaplessMode},
        events::{EventBus, TrackId},
        play::{
            DeckMixSettings, DeckMixerConfig, DeckPass, Outbox, OutputSnapshot, Player,
            PlayerConfig, PlayerImpl, SessionOutputView, Slot, StreamShape, TrackCommand,
            TrackReceipt, TrackSettings, TrackStatus, mock,
        },
        signal::{AudioSpec, FrameCount, SessionFrame},
    };
    use kithara_command::ScopedReceipt;
    use kithara_test_fixtures::assets::rhythm_mp3_deck_b_120bpm_48k;

    fn with_load_pass<R>(
        mixer: &mut mock::MixerRig,
        dispatcher: &mut Sender<Loads>,
        output: &OutputSnapshot,
        now: SessionFrame,
        run: impl FnOnce(&mut Outbox<'_, TestPools>) -> R,
    ) -> R {
        let deck = mixer.ends.snapshot.read();
        let mut scope = mixer.ring.scope(mixer.scope).expect("live mixer scope");
        let mut out = Outbox::new(&mut scope, dispatcher).in_pass(DeckPass {
            mix: DeckMixSettings::default(),
            suspended: false,
            now,
            delivery: FrameCount::new(0),
            output,
            deck,
        });
        run(&mut out)
    }

    let encoded = rhythm_mp3_deck_b_120bpm_48k();
    let rate = NonZeroU32::new(48_000).expect("source and host rate");
    let spec = AudioSpec::new(2, rate);
    let media_origin = spec.duration_for(1_105).expect("tagged recording origin");
    let target = Duration::from_secs(2);
    let expected_raw = media_origin + target;
    let expected_raw_frame = spec
        .frame_at(expected_raw)
        .expect("initial raw target frame");
    let trace = usdt_trace::scope();
    let worker = PlayWorker::new(PlayWorkerConfig::builder(pools()).build());
    let bus = EventBus::default();
    let mut decoder_events = bus.subscribe::<DecoderEvent>();
    let config: ResourceConfig<TestPools> = ResourceConfig::for_src(ResourceSrc::Path(
        encoded.path().expect("stored MP3 fixture").to_owned(),
    ))
    .store(
        AssetStore::builder(worker.pools().clone())
            .backend(StorageBackend::Memory)
            .build(),
    )
    .worker(worker.clone())
    .host_sample_rate(rate)
    .events(bus)
    .decoder(
        AudioDecoderConfig::builder()
            .backend(DecoderBackend::Symphonia)
            .gapless_mode(GaplessMode::MediaOnly)
            .build(),
    )
    .build();
    let item = ResourceLoad::new(config, Box::new(AudioObserverSlot::default().relay()));
    let (mut dispatcher, inbox) = channel::<Loads>(ChannelConfig::builder().build());
    let _driver = worker
        .start_dispatcher(inbox)
        .expect("one dispatcher owner");
    let output = SessionOutputView::new(rate).get();
    let shape = StreamShape::new(NonZeroU32::new(64).expect("host block"), rate);
    let mut mixer =
        mock::MixerRig::new(DeckMixerConfig::default(), shape, worker.pools()).expect("real mixer");
    let slot = Slot::new(0);
    let mut player = PlayerImpl::<TestPools>::new(PlayerConfig {
        item: TrackId::allocate(),
        slot: Some(slot),
        settings: TrackSettings::default(),
    })
    .expect("player settings");
    let loaded = with_load_pass(
        &mut mixer,
        &mut dispatcher,
        &output,
        SessionFrame::new(0),
        |out| {
            player.apply(
                TrackCommand::Load {
                    item,
                    position: target,
                },
                out,
            )
        },
    )
    .expect("nonzero media Load")
    .expect("one Load sequence");
    assert_eq!(player.snapshot().position, target);
    let receipt = answered(&mut dispatcher).await;
    assert_eq!(receipt.seq(), loaded);
    let Outcome::Applied {
        data: Dispatched::Loaded(_),
        ..
    } = receipt.outcome()
    else {
        panic!("the real dispatcher opens the tagged MP3");
    };
    assert_eq!(opened(&trace), 1);
    let mut initial_changes = Vec::new();
    while let Ok(envelope) = decoder_events.try_recv() {
        hang_tick!("initial decoder receipt evidence".to_owned());
        if let DecoderEvent::DecoderChanged { backend, cause, .. } = envelope.event {
            initial_changes.push((backend, cause));
        }
        hang_reset!();
    }
    assert_eq!(
        initial_changes,
        vec![(ObservedBackend::Symphonia, DecoderChangeCause::Initial)]
    );
    with_load_pass(
        &mut mixer,
        &mut dispatcher,
        &output,
        SessionFrame::new(0),
        |out| player.settle(TrackReceipt::Loaded(receipt), out),
    );

    let mut left = [0.0; 64];
    let mut right = [0.0; 64];
    mixer
        .block(SessionFrame::new(0), [&mut left, &mut right])
        .expect("actual attachment block");
    let raw = mixer.ends.snapshot.read().slots[usize::from(slot.get())];
    let mark = raw.mark.expect("first real source mark");
    assert_eq!(raw.media_origin, Some(media_origin));
    assert_eq!(
        spec.frame_at(mark.position).expect("raw marked frame"),
        expected_raw_frame
    );
    assert_eq!(mark.position, expected_raw);
    let mut attached = Vec::new();
    while let Some(receipt) = mixer.ring.receipt() {
        hang_tick!("actual attachment receipt".to_owned());
        let ScopedReceipt::Scope(scope, receipt) = receipt else {
            panic!("scoped attachment answer");
        };
        assert_eq!(scope, mixer.scope);
        attached.push(receipt);
        hang_reset!();
    }
    assert!(
        !attached.is_empty(),
        "the mixer returns its actual attachment"
    );
    for receipt in attached {
        let seq = receipt.seq();
        let (outcome, mut batch) = receipt.into();
        with_load_pass(
            &mut mixer,
            &mut dispatcher,
            &output,
            SessionFrame::new(0),
            |out| {
                player.settle(
                    TrackReceipt::Deck {
                        seq,
                        outcome: &outcome,
                        batch: &mut batch,
                    },
                    out,
                )
            },
        );
    }
    with_load_pass(
        &mut mixer,
        &mut dispatcher,
        &output,
        SessionFrame::new(0),
        |out| player.tick(SessionFrame::new(0), out),
    );
    assert_eq!(player.snapshot().status, TrackStatus::Loaded);
    assert_eq!(player.snapshot().position, target);
    let started = with_load_pass(
        &mut mixer,
        &mut dispatcher,
        &output,
        SessionFrame::new(64),
        |out| player.apply(TrackCommand::Play { at: When::Next }, out),
    )
    .expect("start loaded source")
    .expect("Start sequence");
    mixer
        .block(SessionFrame::new(64), [&mut left, &mut right])
        .expect("consume actual source PCM");
    assert!(left.iter().chain(&right).all(|sample| sample.is_finite()));
    assert!(left.iter().chain(&right).any(|sample| sample.abs() > 0.0));
    let consumed = mixer.ends.snapshot.read().slots[usize::from(slot.get())];
    assert_eq!(consumed.media_origin, Some(media_origin));
    assert_eq!(
        spec.frame_at(consumed.mark.expect("consumed source mark").position)
            .expect("consumed raw frame"),
        expected_raw_frame + 64,
    );
    let mut played = Vec::new();
    while let Some(receipt) = mixer.ring.receipt() {
        hang_tick!("actual Start receipt".to_owned());
        let ScopedReceipt::Scope(scope, receipt) = receipt else {
            panic!("scoped Start answer");
        };
        assert_eq!(scope, mixer.scope);
        played.push(receipt);
        hang_reset!();
    }
    assert!(played.iter().any(|receipt| receipt.seq() == started
        && matches!(receipt.outcome(), Outcome::Applied { at, .. } if *at == SessionFrame::new(64))));
    for receipt in played {
        let seq = receipt.seq();
        let (outcome, mut batch) = receipt.into();
        with_load_pass(
            &mut mixer,
            &mut dispatcher,
            &output,
            SessionFrame::new(128),
            |out| {
                player.settle(
                    TrackReceipt::Deck {
                        seq,
                        outcome: &outcome,
                        batch: &mut batch,
                    },
                    out,
                )
            },
        );
    }
    with_load_pass(
        &mut mixer,
        &mut dispatcher,
        &output,
        SessionFrame::new(128),
        |out| player.tick(SessionFrame::new(128), out),
    );
    assert_eq!(
        player.snapshot().status,
        TrackStatus::Playing {
            since: SessionFrame::new(64)
        }
    );
    assert_eq!(
        spec.frame_at(player.snapshot().position)
            .expect("public media frame after actual PCM"),
        spec.frame_at(target).expect("commanded media frame") + 64,
    );
    assert_eq!(
        opened(&trace),
        1,
        "initial media translation never reopens the source"
    );
}
