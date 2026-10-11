pub(super) use kithara_assets::{AssetStore, StorageBackend};
use kithara_command::{Live, Port};
pub(super) use kithara_command::{Outcome, Rejection, ScopedReceipt, Seq, When};
use kithara_config::ConfigOwner;
pub(super) use kithara_events::TrackId;
pub(super) use kithara_platform::time::Duration;
pub(super) use kithara_play::{
    DeckMixSettings, DeckMixerConfig, DeckPart, DeckPass, Outbox, PlayWorker, PlayWorkerConfig,
    Player, ResourceConfig, ResourcePrep, ResourceSrc, TrackReceipt, TrackStatus as PlayingStatus,
    mock,
};
use kithara_play::{DeckMixSettingsChange, DeckProtocol, PlayerEvent};
pub(super) use kithara_render::{
    LaneFrame,
    bridge::{Returned, SlotMark},
};
pub(super) use kithara_signal::{AudioSpec, FrameCount, SessionFrame};
pub(super) use kithara_test_utils::{TestTempDir, kithara};

use super::*;
pub(super) use crate::{
    QueueConfig, TrackSource,
    test_pools::{TestPools, pools},
};

#[kithara::test(native)]
fn rejected_live_commands_leave_requested_values_unchanged() {
    use crate::{CrossfadeSettings, QueueConfigPatch, QueueSettings, QueueSettingsChange};

    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let settings = QueueSettings::builder()
        .crossfade(CrossfadeSettings {
            duration: 1.0,
            ..CrossfadeSettings::default()
        })
        .build();
    let mut queue = Queue::new(QueueConfig::builder().prep(prep).settings(settings).build());
    let mut rig = mock::DeckRig::new(DeckMixerConfig::default()).expect("deck rig");
    let change = QueueSettingsChange::Crossfade(CrossfadeSettings {
        duration: 2.0,
        ..CrossfadeSettings::default()
    });

    assert!(matches!(
        with_outbox(&mut queue, &mut rig, |queue, out| {
            Player::apply(
                queue,
                QueueCommand::ConfigureQueue(change, When::At(SessionFrame::new(128))),
                out,
            )
        }),
        Err(PlayError::Untimed)
    ));
    assert_eq!(queue.config.settings.crossfade().duration, 1.0);
    assert_eq!(queue.control().crossfade_settings().duration, 1.0);

    let Err(error) = serde_yaml_ng::from_str::<QueueConfigPatch>("preload_lead: 5.0\n") else {
        panic!("immutable preload lead must not be a document key");
    };
    assert!(error.to_string().contains("preload_lead"));
    assert_eq!(queue.config.preload_lead.as_secs_f32(), 3.5);

    assert!(
        with_outbox(&mut queue, &mut rig, |queue, out| {
            Player::apply(queue, QueueCommand::ConfigureQueue(change, When::Next), out)
        })
        .is_ok()
    );
    assert_eq!(queue.config.settings.crossfade().duration, 2.0);
    assert_eq!(queue.control().crossfade_settings().duration, 2.0);
}

fn mix_command(
    queue: &mut Queue<TestPools>,
    rig: &mut mock::DeckRig<TestPools>,
    mix: &mut Live<DeckMixSettings, DeckProtocol>,
    command: QueueCommand<TestPools>,
) -> Result<Option<Seq>, PlayError> {
    let output = mock::output(None).get();
    let deck = queue.deck.mixer.clone();
    let pass = DeckPass {
        mix: *mix.config(),
        suspended: false,
        now: SessionFrame::new(0),
        delivery: FrameCount::new(128),
        output: &output,
        deck: &deck,
    };
    let mut scope = rig.ring.scope(rig.scope).expect("deck scope");
    let mut out = Outbox::new(&mut scope, &mut rig.dispatcher)
        .in_pass(pass)
        .lend_mix(mix);
    HostedDeck::tick(queue, pass, &mut out);
    Player::apply(queue, command, &mut out)
}

#[kithara::test(native)]
fn rejected_slot_volume_leaves_live_values_unchanged() {
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let mut queue = Queue::new(QueueConfig::builder().prep(prep).build());
    let control = queue.control();
    let mut events = queue.subscribe::<PlayerEvent>();
    let mut mix = Live::<DeckMixSettings, DeckProtocol>::new(DeckMixSettings::default())
        .expect("default mix");
    let mut rig = mock::DeckRig::new(DeckMixerConfig::default()).expect("deck rig");
    {
        let mut scope = rig.ring.scope(rig.scope).expect("deck scope");
        let capacity = scope.available();
        assert!(capacity > 0);
        for _ in 0..capacity {
            scope
                .send(
                    When::Next,
                    kithara_command::Batch {
                        basis: Vec::new(),
                        commands: vec![DeckPart::Mix(DeckMixSettingsChange::Muted(false))],
                    },
                )
                .expect("fill scoped channel");
        }
        assert_eq!(scope.available(), 0);
    }
    assert!(matches!(
        mix_command(&mut queue, &mut rig, &mut mix, QueueCommand::SetVolume(0.4)),
        Err(PlayError::Full("deck"))
    ));
    assert_eq!(f32::from(mix.config().volume()), 1.0);
    assert_eq!(f32::from(mix.projected().volume()), 1.0);
    assert_eq!(control.volume(), 1.0);
    assert!(matches!(
        mix_command(&mut queue, &mut rig, &mut mix, QueueCommand::SetMuted(true)),
        Err(PlayError::Full("deck"))
    ));
    assert!(!mix.config().muted());
    assert!(!mix.projected().muted());
    assert!(!control.is_muted());
    assert!(events.try_recv().is_err());

    rig.ring.close(rig.scope).expect("close scoped channel");
    assert!(matches!(
        mix_command(&mut queue, &mut rig, &mut mix, QueueCommand::SetVolume(0.4)),
        Err(PlayError::Closed)
    ));
    assert_eq!(f32::from(mix.config().volume()), 1.0);
    assert_eq!(f32::from(mix.projected().volume()), 1.0);
    assert_eq!(control.volume(), 1.0);
    assert!(matches!(
        mix_command(&mut queue, &mut rig, &mut mix, QueueCommand::SetMuted(true)),
        Err(PlayError::Closed)
    ));
    assert!(!mix.config().muted());
    assert!(!mix.projected().muted());
    assert!(!control.is_muted());
    assert!(events.try_recv().is_err());

    let mut idle = mock::DeckRig::new(DeckMixerConfig::default()).expect("idle deck rig");
    let seq = mix_command(
        &mut queue,
        &mut idle,
        &mut mix,
        QueueCommand::SetVolume(0.4),
    )
    .expect("idle volume accepted")
    .expect("mix batch");
    assert_eq!(f32::from(mix.config().volume()), 1.0);
    assert_eq!(f32::from(mix.projected().volume()), 0.4);
    assert!(events.try_recv().is_err());
    let receipts = idle
        .block(SessionFrame::new(0), 0.0)
        .expect("idle mix applied");
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].seq(), seq);
    assert!(matches!(receipts[0].outcome(), Outcome::Applied { .. }));
    assert!(mix.settle(&receipts[0]).is_some());
    assert_eq!(f32::from(mix.config().volume()), 0.4);
    let output = mock::output(None).get();
    let deck = queue.deck.mixer.clone();
    let pass = DeckPass {
        mix: *mix.config(),
        suspended: false,
        now: SessionFrame::new(0),
        delivery: FrameCount::new(128),
        output: &output,
        deck: &deck,
    };
    let mut scope = idle.ring.scope(idle.scope).expect("idle scope");
    let mut out = Outbox::new(&mut scope, &mut idle.dispatcher).in_pass(pass);
    HostedDeck::tick(&mut queue, pass, &mut out);
    assert_eq!(control.volume(), 0.4);
    assert!(matches!(events.try_recv().map(|event| event.event),
        Ok(PlayerEvent::VolumeChanged { volume }) if volume == 0.4));
    assert!(
        events.try_recv().is_err(),
        "idle volume emits exactly one event"
    );
}

#[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
async fn arm_current_fixture(
    rig: &mut mock::DeckRig<TestPools>,
    dir: &TestTempDir,
) -> kithara_render::Loaded<kithara_play::OpenedTrack> {
    let mut loaded = std::pin::pin!(loaded_fixture(rig, dir));
    loop {
        hang_tick!();
        if let Ok(loaded) =
            kithara_platform::time::timeout(Duration::from_millis(10), loaded.as_mut()).await
        {
            hang_reset!();
            return loaded;
        }
    }
}

#[kithara::test(tokio)]
async fn finish_disarms_and_ignores_stale_ticket() {
    let (mut queue, id, _, mut rig, dir) = pending_selection();
    let stale = rig
        .open(Err(kithara_play::LoadRefusal::Cancelled))
        .expect("old terminal dispatcher receipt");
    with_outbox(&mut queue, &mut rig, Queue::cancel_target).expect("cancel old target");
    let current_seq = with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(
            queue,
            QueueCommand::Select {
                id,
                transition: crate::Transition::None,
            },
            out,
        )
    })
    .expect("select replacement")
    .expect("replacement open");
    assert_ne!(stale.seq(), current_seq);
    let current = queue.tracks.records()[0]
        .load
        .as_ref()
        .expect("replacement guard")
        .token()
        .clone();
    let watch = queue.tracks.records()[0]
        .load
        .as_ref()
        .expect("replacement guard")
        .watch()
        .clone();
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::settle(queue, TrackReceipt::Loaded(stale), out)
    });
    assert_eq!(
        queue.tracks.records()[0].status,
        TrackStatus::Loading,
        "stale finish must not evict"
    );
    assert!(
        queue.tracks.records()[0].load.is_some(),
        "stale finish must retain the current guard"
    );
    queue
        .tracks
        .apply_report(crate::loading::LoadReport::Slow { id, watch });
    assert_eq!(queue.tracks.records()[0].status, TrackStatus::Slow);
    assert!(!current.is_cancelled());
    let loaded = arm_current_fixture(&mut rig, &dir).await;
    answer_loaded(&mut queue, &mut rig, loaded);
    assert!(
        queue.tracks.records()[0].load.is_none(),
        "finished attempt must leave the record vacant"
    );
    assert!(!current.is_cancelled());
    with_outbox(&mut queue, &mut rig, Queue::cancel_target).expect("release completed target");
    let next = with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(
            queue,
            QueueCommand::Select {
                id,
                transition: crate::Transition::None,
            },
            out,
        )
    })
    .expect("begin following attempt")
    .expect("following open");
    assert_ne!(next, current_seq);
    assert!(queue.tracks.records()[0].load.is_some());
    assert!(
        !queue.tracks.records()[0]
            .load
            .as_ref()
            .expect("following guard")
            .token()
            .is_cancelled()
    );
}

#[kithara::test(tokio)]
async fn atomic_track_id_cas_arm_then_disarm() {
    let (mut queue, mut rig, dir) = empty_queue();
    queue.config.action_at_item_end = ActionAtItemEnd::None;
    let source = TrackSource::Uri(
        dir.path()
            .join("entry.wav")
            .to_str()
            .expect("fixture path")
            .to_owned(),
    );
    with_outbox(&mut queue, &mut rig, |queue, out| {
        for id in [TrackId(2), TrackId(3), TrackId(4)] {
            Player::apply(
                queue,
                QueueCommand::Append {
                    id,
                    source: source.clone(),
                },
                out,
            )
            .expect("append arm fixture");
        }
        Player::apply(
            queue,
            QueueCommand::Select {
                id: TrackId(2),
                transition: crate::Transition::None,
            },
            out,
        )
        .expect("select sounding predecessor");
    });
    let loaded = arm_current_fixture(&mut rig, &dir).await;
    answer_loaded(&mut queue, &mut rig, loaded);
    crate::mock::finish(&mut queue, &mut rig);
    assert_eq!(queue.current, Some(TrackId(2)));
    assert!(
        queue.current_end().is_some(),
        "deadline guard must see a sounding predecessor"
    );
    let seq = with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(
            queue,
            QueueCommand::Select {
                id: TrackId(3),
                transition: crate::Transition::None,
            },
            out,
        )
    })
    .expect("arm track three")
    .expect("open sequence");
    let incoming = queue.incoming_index(TrackId(3)).expect("armed track");
    queue.active.get_mut(incoming).expect("incoming slot").role =
        Role::Incoming { batch: Some(seq) };
    let ids = queue.track_ids();
    assert_eq!(queue.navigation.next(&ids, true, false), Some(TrackId(4)));
    let unrelated = with_outbox(&mut queue, &mut rig, |queue, out| {
        queue.load_track(
            TrackId(4),
            Role::Preloaded,
            Some(&mock::output(None).get()),
            out,
        )
    })
    .expect("unrelated preload")
    .expect("distinct open sequence");
    assert_ne!(seq, unrelated);
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(
            queue,
            QueueCommand::SetActionAtItemEnd(ActionAtItemEnd::Advance),
            out,
        )
        .expect("enable automatic admission with a pending target");
        for _ in 0..2 {
            queue
                .tick_deadlines(SessionFrame::new(256), Some(&mock::output(None).get()), out)
                .expect("deadline admission while track three is armed");
        }
    });
    assert_eq!(
        queue.target.map(|target| target.to),
        Some(TrackId(3)),
        "a second arm must not replace the armed track"
    );
    queue
        .transition_applied(unrelated, SessionFrame::new(384))
        .expect("unrelated receipt");
    assert_eq!(queue.target.map(|target| target.to), Some(TrackId(3)));
    assert_eq!(queue.current, Some(TrackId(2)));
    queue
        .transition_applied(seq, SessionFrame::new(384))
        .expect("matching receipt");
    assert_eq!(queue.current, Some(TrackId(3)));
    assert!(queue.target.is_none());
}

#[kithara::test]
fn a_host_pass_publishes_the_deck_mixer_counters() {
    let prep = ResourcePrep::builder()
        .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
        .build();
    let mut queue = Queue::new(QueueConfig::builder().prep(prep).build());
    let control = queue.control();
    assert_eq!(control.mixer_blocks(), 0);
    let mut deck = kithara_play::DeckSnapshot::new(DeckMixerConfig::default());
    deck.blocks = 7;
    let output = mock::output(None).get();
    let pass = DeckPass {
        mix: DeckMixSettings::default(),
        suspended: false,
        now: SessionFrame::new(0),
        delivery: FrameCount::new(128),
        output: &output,
        deck: &deck,
    };
    let mut rig = mock::DeckRig::new(DeckMixerConfig::default()).expect("deck rig");
    let mut scope = rig.ring.scope(rig.scope).expect("deck scope");
    let mut out = Outbox::new(&mut scope, &mut rig.dispatcher).in_pass(pass);
    HostedDeck::tick(&mut queue, pass, &mut out);
    assert_eq!(control.mixer_blocks(), 7);
    assert_eq!(control.mixer_metrics(), deck.metrics);
}

#[kithara::test(tokio)]
async fn an_item_is_loaded_on_its_load_receipt_before_its_attach_applies() {
    let (mut queue, first, _, mut rig, dir) = pending_selection();
    let mut events = queue.subscribe::<QueueEvent>();
    answer_load(&mut queue, &mut rig, &dir).await;
    assert_eq!(
        queue.track(first).expect("record").status,
        TrackStatus::Loaded
    );
    assert_eq!(
        queue
            .track(first)
            .expect("record")
            .metadata()
            .title
            .as_deref(),
        Some("Loaded fixture")
    );
    assert!(queue.current_track().is_none());
    assert_eq!(
        queue
            .tracks_active()
            .next()
            .expect("incoming track")
            .snapshot()
            .status,
        PlayingStatus::Loading
    );
    let mut ready = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|envelope| matches!(envelope.event, QueueEvent::NextTrackReady { id, .. } if id == first)).count();
    assert_eq!(ready, 1);
    answer_deck(&mut queue, &mut rig, SessionFrame::new(0));
    ready += std::iter::from_fn(|| events.try_recv().ok())
        .filter(|envelope| matches!(envelope.event, QueueEvent::NextTrackReady { id, .. } if id == first)).count();
    assert_eq!(
        queue.track(first).expect("record").status,
        TrackStatus::Loaded
    );
    assert_eq!(ready, 1);
}

#[kithara::test(tokio)]
async fn a_pause_before_the_selected_item_enters_makes_it_current_paused() {
    let (mut queue, first, _, mut rig, dir) = pending_selection();
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(queue, QueueCommand::Pause { at: When::Next }, out)
    })
    .expect("pause pending selection");
    answer_load(&mut queue, &mut rig, &dir).await;
    assert_eq!(queue.current, None);
    answer_deck(&mut queue, &mut rig, SessionFrame::new(0));
    assert_eq!(queue.current, Some(first));
    let active = queue
        .active
        .get(queue.active_current_index().expect("current slot"))
        .expect("slot");
    assert_eq!(active.role, Role::Current);
    assert_eq!(
        active.track.snapshot().status,
        PlayingStatus::Paused { at: Duration::ZERO }
    );
    assert!(queue.target.is_none());
    rig.ring.publish().expect("publish paused entry");
    rig.inbox.drain();
    assert!(
        rig.inbox
            .scope(rig.scope)
            .expect("deck scope")
            .next_due(SessionFrame::new(128), 1)
            .is_none()
    );
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(queue, QueueCommand::Play { at: When::Next }, out)
    })
    .expect("play the current paused track");
    answer_deck(&mut queue, &mut rig, SessionFrame::new(128));
    assert!(matches!(
        queue
            .current_track()
            .expect("current track")
            .snapshot()
            .status,
        PlayingStatus::Playing { .. }
    ));
}

#[kithara::test(tokio)]
async fn play_with_a_pending_selection_enters_that_selection() {
    let (mut queue, first, second, mut rig, dir) = pending_selection();
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(queue, QueueCommand::Play { at: When::Next }, out)
    })
    .expect("play pending selection");
    assert_eq!(queue.target.expect("selected target").to, first);
    answer_load(&mut queue, &mut rig, &dir).await;
    answer_deck(&mut queue, &mut rig, SessionFrame::new(0));
    assert!(matches!(
        queue
            .active
            .get(queue.incoming_index(first).expect("incoming first"))
            .expect("slot")
            .role,
        Role::Incoming { batch: Some(_) }
    ));
    answer_deck(&mut queue, &mut rig, SessionFrame::new(128));
    assert_eq!(queue.current, Some(first));
    assert_ne!(queue.current, Some(second));
    assert!(queue.target.is_none());
}

#[kithara::test(tokio)]
#[case::stop_first(false)]
#[case::stale_first(true)]
async fn a_pause_over_a_scheduled_entry_settles_current_paused(#[case] reverse: bool) {
    let (mut queue, first, _, mut rig, dir) = pending_selection();
    answer_load(&mut queue, &mut rig, &dir).await;
    answer_deck(&mut queue, &mut rig, SessionFrame::new(0));
    with_outbox(&mut queue, &mut rig, |queue, out| {
        Player::apply(queue, QueueCommand::Pause { at: When::Next }, out)
    })
    .expect("withdraw the scheduled entry");
    rig.ring.publish().expect("publish Stop");
    rig.inbox.drain();
    {
        let mut level = rig.inbox.scope(rig.scope).expect("deck scope");
        let mut stop = level
            .next_due(SessionFrame::new(0), 1)
            .expect("Stop before Start");
        for part in stop.commands_mut() {
            let DeckPart::Stop { slot, .. } = *part else {
                panic!("withdrawal only stops the incoming slot");
            };
            *part = DeckPart::Returned(Returned::Stopped {
                slot,
                resume: SlotMark {
                    session: SessionFrame::new(0),
                    lane: LaneFrame::default(),
                    position: Duration::ZERO,
                },
            });
        }
        stop.apply(());
    }
    let mut receipts = std::iter::from_fn(|| rig.ring.receipt()).collect::<Vec<_>>();
    assert_eq!(receipts.len(), 2);
    assert_eq!(
        receipts
            .iter()
            .filter(|receipt| matches!(
                receipt, ScopedReceipt::Scope(_, receipt)
                    if matches!(receipt.outcome(), Outcome::Rejected(Rejection::Stale))
            ))
            .count(),
        1
    );
    if reverse {
        receipts.reverse();
    }
    for receipt in receipts {
        let ScopedReceipt::Scope(_, receipt) = receipt else {
            panic!("withdrawal must answer on its deck scope");
        };
        let seq = receipt.seq();
        let (outcome, mut batch) = receipt.into();
        with_outbox(&mut queue, &mut rig, |queue, out| {
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
    }
    assert_eq!(queue.current, Some(first));
    assert_eq!(
        queue
            .current_track()
            .expect("current track")
            .snapshot()
            .status,
        PlayingStatus::Paused { at: Duration::ZERO }
    );
    assert!(queue.target.is_none());
    rig.ring.publish().expect("publish settled entry");
    rig.inbox.drain();
    assert!(
        rig.inbox
            .scope(rig.scope)
            .expect("deck scope")
            .next_due(SessionFrame::new(128), 1)
            .is_none()
    );
}
