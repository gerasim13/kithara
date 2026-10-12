use kithara_command::{Batch, Port, Receipt};
use kithara_play::{DeckEvent, DeckMixSettingsChange, TrackCommand};
use kithara_render::bridge::{DeckProtocol, PlaybackFault, Slot};

use super::*;
use crate::{AdvanceReason, QueueSettingsChange, Transition};

struct HandoverRig {
    queue: Queue<TestPools>,
    deck: mock::DeckRig<TestPools>,
    loader: mock::DeckRig<TestPools>,
    dir: TestTempDir,
    ids: [TrackId; 3],
    last_lane: Option<kithara_render::LaneId>,
}

impl HandoverRig {
    #[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
    async fn playing() -> Self {
        hang_tick!();
        let rig = Self::playing_with_sources(["item-1.wav", "item-2.wav", "item-3.wav"]).await;
        hang_reset!();
        rig
    }

    #[kithara::flash(io)]
    #[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
    async fn playing_with_sources(names: [&str; 3]) -> Self {
        hang_tick!();
        let (queue, deck, dir) = empty_queue();
        let mut rig = Self {
            queue,
            deck,
            loader: mock::DeckRig::new(DeckMixerConfig::default()).expect("fixture loader"),
            dir,
            ids: [
                TrackId::allocate(),
                TrackId::allocate(),
                TrackId::allocate(),
            ],
            last_lane: None,
        };
        for name in names {
            hang_tick!();
            let path = rig.dir.path().join(name);
            mock::write_pcm_wav(&path, &[0.5; 2_048], AudioSpec::new(2, mock::SAMPLE_RATE))
                .expect("offline item fixture");
            hang_reset!();
        }
        for (id, name) in rig.ids.into_iter().zip(names).take(1) {
            hang_tick!();
            let path = rig.dir.path().join(name);
            rig.apply(QueueCommand::Append {
                id,
                source: TrackSource::Uri(path.to_str().expect("fixture path").to_owned()),
            })
            .expect("append item");
            hang_reset!();
        }
        rig.select(0, Transition::None).expect("select first");
        hang_tick!();
        rig.load(0, 64.295).await;
        hang_reset!();
        crate::mock::finish(&mut rig.queue, &mut rig.deck);
        assert_eq!(rig.queue.current, Some(rig.ids[0]));
        let slot = rig.slot(0);
        rig.queue.deck.mixer.slots[usize::from(slot.get())].mark = Some(SlotMark {
            session: SessionFrame::new(128),
            lane: LaneFrame::default(),
            position: Duration::ZERO,
        });
        with_outbox(&mut rig.queue, &mut rig.deck, |queue, out| {
            let current = queue.active_current_index().expect("current fixture");
            queue
                .active
                .get_mut(current)
                .expect("current fixture")
                .track
                .tick(SessionFrame::new(128), out);
        });
        rig.queue.insert_entry(
            rig.ids[1],
            TrackSource::Uri(
                rig.dir
                    .path()
                    .join(names[1])
                    .to_str()
                    .expect("fixture path")
                    .to_owned(),
            ),
            crate::queue::types::Placement::Append,
        );
        rig
    }

    fn apply(&mut self, command: QueueCommand<TestPools>) -> Result<Option<Seq>, PlayError> {
        with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
            Player::apply(queue, command, out)
        })
    }

    fn select(&mut self, index: usize, transition: Transition) -> Result<Option<Seq>, PlayError> {
        if self.queue.track(self.ids[index]).is_none() {
            self.queue.insert_entry(
                self.ids[index],
                TrackSource::Uri(
                    self.dir
                        .path()
                        .join("item-3.wav")
                        .to_str()
                        .expect("fixture path")
                        .to_owned(),
                ),
                crate::queue::types::Placement::Append,
            );
        }
        self.apply(QueueCommand::Select {
            id: self.ids[index],
            transition,
        })
    }

    #[kithara::flash(io)]
    #[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
    async fn load(&mut self, index: usize, duration: f64) {
        use kithara_render::{Dispatched, DispatcherCommand};

        self.deck.opens.drain();
        loop {
            hang_tick!();
            let mut due = self
                .deck
                .opens
                .next_due((), 1)
                .expect("queued dispatcher request");
            let command = due.commands_mut().pop().expect("one dispatcher command");
            assert!(due.commands_mut().is_empty());
            let opened = match command {
                DispatcherCommand::Load(request) => {
                    let wanted = self
                        .queue
                        .active
                        .iter()
                        .find(|active| active.item == self.ids[index])
                        .and_then(|active| active.load)
                        .expect("fixture load")
                        .seq();
                    if due.seq() != wanted {
                        due.commands_mut().push(DispatcherCommand::Load(request));
                        due.defer();
                        continue;
                    }
                    let mut loaded = self
                        .loader
                        .load_request_fixture(*request)
                        .await
                        .expect("actual queued source");
                    self.last_lane = Some(loaded.lane);
                    loaded.opened.duration = Some(Duration::from_secs_f64(duration));
                    due.apply(Dispatched::Loaded(loaded));
                    true
                }
                DispatcherCommand::Release(_) => {
                    due.apply(Dispatched::Released);
                    false
                }
                DispatcherCommand::SetPriority(_, _) => {
                    due.apply(Dispatched::Prioritized);
                    false
                }
            };
            let receipt = self
                .deck
                .dispatcher
                .receipts()
                .next()
                .expect("dispatcher answer");
            with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
                Player::settle(queue, TrackReceipt::Loaded(receipt), out)
            });
            hang_reset!();
            if opened {
                break;
            }
        }
        crate::mock::settle(&mut self.queue, &mut self.deck, SessionFrame::new(0));
    }

    #[kithara::hang_watchdog(timeout = Duration::from_secs(1))]
    async fn arm(&mut self, index: usize) -> Slot {
        hang_tick!();
        let id = self.ids[index];
        with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
            queue.load_track(id, Role::Preloaded, Some(&mock::output(None).get()), out)
        })
        .expect("preload item");
        hang_reset!();
        hang_tick!();
        self.load(index, 162.0).await;
        hang_reset!();
        self.slot(index)
    }

    fn slot(&self, index: usize) -> Slot {
        self.queue
            .active
            .iter()
            .find(|active| active.item == self.ids[index])
            .expect("active item")
            .slot
    }

    fn source(&self, index: usize) -> &str {
        self.deck
            .mixer
            .held(self.slot(index))
            .expect("opened resource source URI")
    }

    fn armed(&self) -> Option<TrackId> {
        self.queue.target.map(|target| target.to).or_else(|| {
            self.queue
                .active
                .iter()
                .find(|active| active.role == Role::Preloaded)
                .map(|active| active.item)
        })
    }

    fn entry(&mut self, index: usize, transition: Transition) -> Vec<Receipt<DeckProtocol>> {
        self.select(index, transition)
            .expect("select prepared item");
        self.deck
            .block(SessionFrame::new(128), 0.0)
            .expect("entry block")
    }

    fn settle(&mut self, receipts: Vec<Receipt<DeckProtocol>>) {
        for receipt in receipts {
            let seq = receipt.seq();
            let (outcome, mut batch) = receipt.into();
            with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
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
    }

    fn event(&mut self, event: DeckEvent) {
        with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
            Player::settle(queue, TrackReceipt::Event(event), out)
        });
    }

    fn outgoing_observation(&mut self, index: usize) {
        let slot = self.slot(index);
        let snapshot = &mut self.queue.deck.mixer.slots[usize::from(slot.get())];
        snapshot.position = 62.3;
        snapshot.duration = 64.295;
        snapshot.mark = Some(SlotMark {
            session: SessionFrame::new(256),
            lane: LaneFrame {
                segment: kithara_signal::SegmentId::FIRST,
                frame: 2_747_430,
            },
            position: Duration::from_secs_f64(62.3),
        });
        with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
            for track in queue.tracks_mut() {
                track.tick(SessionFrame::new(256), out);
            }
        });
        self.queue.publish();
    }

    fn chain(&mut self) {
        self.apply(QueueCommand::ConfigureQueue(
            QueueSettingsChange::Gapless(true),
            When::Next,
        ))
        .expect("gapless policy");
        with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
            queue.next_target(
                Transition::None,
                AdvanceReason::NaturalEof,
                true,
                true,
                Some(&mock::output(None).get()),
                out,
            )
        })
        .expect("arm gapless entry");
        crate::mock::settle(&mut self.queue, &mut self.deck, SessionFrame::new(0));
    }

    fn stitch(&mut self) -> Vec<Receipt<DeckProtocol>> {
        self.deck
            .end(self.slot(0), SessionFrame::new(128))
            .expect("stitch block")
    }

    #[kithara::hang_watchdog]
    fn fill_to_room(&mut self, room: usize) {
        let mut port = self.deck.ring.scope(self.deck.scope).expect("deck scope");
        while port.available() > room {
            hang_tick!();
            port.send(
                When::Next,
                Batch {
                    basis: Vec::new(),
                    commands: vec![DeckPart::Mix(DeckMixSettingsChange::Level(1.0))],
                },
            )
            .expect("fill available deck capacity");
            hang_reset!();
        }
        assert_eq!(port.available(), room);
    }

    #[kithara::hang_watchdog]
    fn assert_no_pending_load(&mut self) {
        use kithara_render::{Dispatched, DispatcherCommand};

        self.deck.opens.drain();
        loop {
            let Some(mut due) = self.deck.opens.next_due((), 1) else {
                break;
            };
            hang_tick!();
            let seq = due.seq();
            let command = due.commands_mut().pop().expect("dispatcher command");
            assert!(due.commands_mut().is_empty());
            match command {
                DispatcherCommand::Load(_) => panic!("promotion reuses the opened lane"),
                DispatcherCommand::SetPriority(_, _) => due.apply(Dispatched::Prioritized),
                DispatcherCommand::Release(_) => due.apply(Dispatched::Released),
            }
            let receipt = self
                .deck
                .dispatcher
                .receipts()
                .next()
                .expect("dispatcher maintenance receipt");
            assert_eq!(receipt.seq(), seq);
            with_outbox(&mut self.queue, &mut self.deck, |queue, out| {
                Player::settle(queue, TrackReceipt::Loaded(receipt), out)
            });
            hang_reset!();
        }
    }

    #[kithara::hang_watchdog]
    fn pending_parts(&mut self) -> Vec<DeckPart> {
        self.deck.ring.publish().expect("publish pending commands");
        self.deck.inbox.drain();
        let mut port = self.deck.inbox.scope(self.deck.scope).expect("deck scope");
        let mut parts = Vec::new();
        while let Some(mut due) = port.next_due(SessionFrame::new(256), 1) {
            hang_tick!();
            parts.append(due.commands_mut());
            due.apply(());
            hang_reset!();
        }
        while self.deck.ring.receipt().is_some() {
            hang_tick!();
            hang_reset!();
        }
        parts
    }

    fn only_current(&mut self, index: usize) {
        let id = self.ids[index];
        self.queue
            .tracks
            .records_mut()
            .retain(|record| record.id == id);
        self.queue.navigation.select(id, &[id]);
        self.queue.config.action_at_item_end = ActionAtItemEnd::Advance;
        self.queue.publish();
    }
}

fn detached(parts: &[DeckPart]) -> Vec<Slot> {
    parts
        .iter()
        .filter_map(|part| match part {
            DeckPart::Detach { slot } => Some(*slot),
            _ => None,
        })
        .collect()
}

#[kithara::hang_watchdog]
fn events(receiver: &mut kithara_events::EventReceiver<QueueEvent>) -> Vec<QueueEvent> {
    let mut events = Vec::new();
    while let Ok(envelope) = receiver.try_recv() {
        hang_tick!();
        events.push(envelope.event);
        hang_reset!();
    }
    events
}

#[kithara::test(tokio)]
async fn arm_next_loads_item_and_returns_src() {
    let mut rig = HandoverRig::playing().await;
    let slot = rig.arm(1).await;
    assert_eq!(rig.armed(), Some(rig.ids[1]));
    assert_eq!(
        rig.source(1),
        rig.dir.path().join("item-2.wav").to_str().expect("source")
    );
    assert_eq!(rig.queue.active.get(1).expect("armed track").slot, slot);
    assert_eq!(
        rig.queue.track(rig.ids[1]).expect("record").status,
        TrackStatus::Loaded
    );
}

#[kithara::test(tokio)]
async fn a_rejected_retained_chain_does_not_rewrite_the_new_selection() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    let seq = rig
        .queue
        .active
        .iter()
        .find_map(|active| match active.role {
            Role::Incoming { batch } if active.item == rig.ids[1] => batch,
            _ => None,
        })
        .expect("accepted chained entry");
    rig.select(2, Transition::None).expect("new selection");
    let target = rig.queue.target.expect("new selection remains pending");
    let index = rig
        .queue
        .incoming_index(rig.ids[1])
        .expect("retained chain");
    with_outbox(&mut rig.queue, &mut rig.deck, |queue, out| {
        queue.transition_settled(
            index,
            &kithara_play::Settled::Rejected {
                seq,
                reason: Rejection::Stale,
            },
            out,
        )
    })
    .expect("retained rejection settles independently");
    let remaining = rig.queue.target.expect("new selection survives");
    assert_eq!(remaining.to, target.to);
    assert_eq!(remaining.bound, target.bound);
    assert_eq!(remaining.continued, target.continued);
    assert_eq!(remaining.stale, target.stale);
}

#[kithara::test(tokio)]
async fn arm_next_idempotent_for_same_index() {
    let mut rig = HandoverRig::playing().await;
    let first = rig.arm(1).await;
    let before = rig.source(1).to_owned();
    let count = rig.queue.active.len();
    with_outbox(&mut rig.queue, &mut rig.deck, |queue, out| {
        queue.load_track(
            rig.ids[1],
            Role::Preloaded,
            Some(&mock::output(None).get()),
            out,
        )
    })
    .expect("repeat preload");
    assert_eq!(rig.slot(1), first);
    assert_eq!(rig.source(1), before);
    assert_eq!(rig.armed(), Some(rig.ids[1]));
    assert_eq!(rig.queue.active.len(), count);
    rig.deck.opens.drain();
    assert!(
        rig.deck.opens.next_due((), 1).is_none(),
        "idempotent preload does not reopen the source"
    );
}

#[kithara::test]
fn arm_next_returns_none_for_empty_slot() {
    let (mut queue, mut deck, _dir) = empty_queue();
    let result = with_outbox(&mut queue, &mut deck, |queue, out| {
        queue.next_target(
            Transition::None,
            AdvanceReason::NaturalEof,
            true,
            true,
            Some(&mock::output(None).get()),
            out,
        )
    })
    .expect("empty queue has no successor");
    assert!(result.is_none());
    assert!(queue.target.is_none());
    assert_eq!(queue.active.len(), 0);
}

#[kithara::test(tokio)]
async fn arm_next_replaces_previously_armed_slot() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    let before = rig.source(1).to_owned();
    rig.select(2, Transition::None)
        .expect("replace preload through selection");
    rig.load(2, 162.0).await;
    assert_ne!(before, rig.source(2));
    assert_eq!(rig.armed(), Some(rig.ids[2]));
}

#[kithara::test(tokio)]
async fn commit_next_advances_index_and_publishes_event() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    let mut receiver = rig.queue.control().subscribe();
    let receipts = rig.entry(1, Transition::None);
    rig.settle(receipts);
    assert_eq!(rig.queue.control().current_index(), Some(1));
    assert_eq!(rig.armed(), None);
    let first_eight: Vec<_> = (0..8).filter_map(|_| receiver.try_recv().ok()).collect();
    assert!(first_eight.iter().any(|envelope| {
        matches!(envelope.event, QueueEvent::CurrentTrackChanged { id: Some(id) } if id == rig.ids[1])
    }), "the current-item announcement is among the first eight events");
}

#[kithara::test(tokio)]
async fn commit_next_index_mismatch_returns_typed_error() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    let unknown = TrackId::allocate();
    let error = with_outbox(&mut rig.queue, &mut rig.deck, |queue, out| {
        queue.apply_command(
            QueueCommand::Select {
                id: unknown,
                transition: Transition::Crossfade,
            },
            Some(&mock::output(None).get()),
            out,
        )
    })
    .expect_err("selection requires a known identity");
    assert!(matches!(error, QueueError::UnknownTrackId(requested) if requested == unknown));
    assert_eq!(rig.armed(), Some(rig.ids[1]));
    assert_eq!(rig.queue.current, Some(rig.ids[0]));
}

#[kithara::test]
fn commit_next_without_arm_returns_not_ready() {
    let (mut queue, mut deck, _dir) = empty_queue();
    let error = with_outbox(&mut queue, &mut deck, |queue, out| {
        queue.send_transition(0, SessionFrame::new(128), out)
    })
    .expect_err("there is no prepared transition");
    assert!(matches!(error, PlayError::NotReady));
}

#[kithara::test(tokio)]
async fn commit_next_publishes_snapshot_before_current_item_changed() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    let mut receiver = rig.queue.control().subscribe();
    let receipts = rig.entry(1, Transition::Crossfade);
    assert_eq!(
        rig.queue.current,
        Some(rig.ids[0]),
        "sending does not publish a new leader"
    );
    assert!(
        events(&mut receiver)
            .iter()
            .all(|event| !matches!(event, QueueEvent::CurrentTrackChanged { .. }))
    );
    rig.settle(receipts);
    assert!(matches!(
        receiver.try_recv().expect("crossfade start").event,
        QueueEvent::CrossfadeStarted { .. }
    ));
    assert_eq!(rig.queue.control().duration_seconds(), Some(162.0));
    assert!(matches!(receiver.try_recv().expect("current change").event,
        QueueEvent::CurrentTrackChanged { id: Some(id) } if id == rig.ids[1]));
    assert!(matches!(receiver.try_recv().expect("advance reason").event,
        QueueEvent::CurrentTrackAdvance { id: Some(id), reason: AdvanceReason::UserSelect } if id == rig.ids[1]));
    assert_eq!(rig.queue.control().duration_seconds(), Some(162.0));
}

#[kithara::test(tokio)]
async fn committed_item_outlives_a_block_the_outgoing_item_was_rendering() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    let receipts = rig.entry(1, Transition::Crossfade);
    rig.settle(receipts);
    rig.outgoing_observation(0);
    assert_eq!(rig.queue.control().duration_seconds(), Some(162.0));
    assert_eq!(rig.queue.control().position_seconds(), Some(0.0));
}

#[kithara::test(tokio)]
async fn a_rejected_fade_in_leaves_the_playhead_on_the_playing_item() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.outgoing_observation(0);
    rig.fill_to_room(0);
    let error = rig
        .select(1, Transition::Crossfade)
        .expect_err("the full deck refuses FadeIn");
    assert!(matches!(error, PlayError::Full("deck")));
    assert_eq!(rig.queue.control().duration_seconds(), Some(64.295));
    assert_eq!(rig.queue.control().position_seconds(), Some(62.3));
}

#[kithara::test(tokio)]
async fn pending_next_state_maps_activated_bool() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    assert!(!rig.queue.current.is_some_and(|id| id == rig.ids[1]));
    let receipts = rig.entry(1, Transition::None);
    assert!(!rig.queue.current.is_some_and(|id| id == rig.ids[1]));
    rig.settle(receipts);
    assert!(rig.queue.current.is_some_and(|id| id == rig.ids[1]));
    assert_eq!(
        rig.queue
            .active
            .iter()
            .find(|active| active.item == rig.ids[1])
            .expect("activated item")
            .role,
        Role::Current
    );
}

#[kithara::test(tokio)]
async fn select_item_on_armed_index_promotes_armed_slot() {
    let mut rig = HandoverRig::playing().await;
    let armed = rig.arm(1).await;
    let source = rig.source(1).to_owned();
    let receipts = rig.entry(1, Transition::None);
    assert!(receipts.iter().all(
        |receipt| !receipt.batch().commands.iter().any(|part| matches!(
            part,
            DeckPart::Detach { .. } | DeckPart::Attach { .. } | DeckPart::Replace { .. }
        ))
    ));
    rig.settle(receipts);
    assert_eq!(rig.queue.control().current_index(), Some(1));
    assert_eq!(rig.armed(), None);
    assert_eq!(rig.slot(1), armed);
    assert_eq!(rig.source(1), source);
    rig.assert_no_pending_load();
}

#[kithara::test(tokio)]
#[should_panic(expected = "promotion reuses the opened lane")]
async fn the_promotion_reuse_check_rejects_an_additional_load() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.select(2, Transition::None)
        .expect("another selection loads");
    rig.assert_no_pending_load();
}

#[kithara::test(tokio)]
async fn unarm_next_clears_when_not_activated_and_unloads() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    let source = rig.source(1).to_owned();
    with_outbox(&mut rig.queue, &mut rig.deck, Queue::cancel_auto).expect("unarm preload");
    let parts = rig.pending_parts();
    assert_eq!(rig.armed(), None);
    assert_eq!(
        source,
        rig.dir.path().join("item-2.wav").to_str().expect("source")
    );
    assert!(
        parts
            .iter()
            .any(|part| matches!(part, DeckPart::Detach { .. }))
    );
}

#[kithara::test(tokio)]
async fn select_item_clears_pending_next_and_unloads_preloaded_track() {
    let mut rig = HandoverRig::playing().await;
    let armed_slot = rig.arm(1).await;
    let source = rig.source(1).to_owned();
    rig.select(2, Transition::None)
        .expect("select another item");
    let parts = rig.pending_parts();
    assert_eq!(detached(&parts), [armed_slot]);
    rig.load(2, 162.0).await;
    crate::mock::finish(&mut rig.queue, &mut rig.deck);
    assert_eq!(rig.armed(), None);
    assert_eq!(rig.queue.control().current_index(), Some(2));
    assert_eq!(
        source,
        rig.dir.path().join("item-2.wav").to_str().expect("source")
    );
}

#[kithara::test(tokio)]
async fn selecting_another_item_cancels_the_successor_preload_instead_of_unloading_it() {
    let mut rig = HandoverRig::playing().await;
    let second_slot = rig.arm(1).await;
    rig.chain();
    let stitched = rig.stitch();
    rig.select(2, Transition::None).expect("selection accepted");
    let commands = rig.pending_parts();
    assert_eq!(detached(&commands), []);
    assert!(
        commands
            .iter()
            .any(|part| matches!(part, DeckPart::Stop { slot, .. } if *slot == second_slot))
    );
    rig.settle(stitched);
    rig.load(2, 162.0).await;
    crate::mock::finish(&mut rig.queue, &mut rig.deck);
    assert_eq!(
        rig.queue.control().current_index(),
        Some(2),
        "the applied selection leads over the successor it fades out"
    );
}

#[kithara::test(tokio)]
#[kithara::hang_watchdog]
async fn a_refused_selection_keeps_a_successor_already_stitched_in_and_reports_it() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    let stitched = rig.stitch();
    for _ in 0..rig.deck.dispatcher.available() {
        hang_tick!();
        rig.deck
            .dispatcher
            .send(
                When::Next,
                Batch {
                    basis: Vec::new(),
                    commands: vec![kithara_render::DispatcherCommand::SetPriority(
                        rig.last_lane.expect("opened successor lane"),
                        kithara_play::ServiceClass::Warm,
                    )],
                },
            )
            .expect("fill dispatcher ring without consuming deck capacity");
        hang_reset!();
    }
    assert_eq!(rig.deck.dispatcher.available(), 0);
    assert!(
        matches!(
            rig.select(2, Transition::None),
            Err(PlayError::Full("dispatcher"))
        ),
        "the selection's load is refused"
    );
    assert_eq!(detached(&rig.pending_parts()), []);
    rig.deck.opens.drain();
    while let Some(mut due) = rig.deck.opens.next_due((), 1) {
        hang_tick!();
        assert!(
            !due.commands_mut()
                .iter()
                .any(|command| matches!(command, kithara_render::DispatcherCommand::Load(_))),
            "the refused selection sends no load"
        );
        due.apply(kithara_render::Dispatched::Prioritized);
        hang_reset!();
    }
    rig.settle(stitched);
    assert_eq!(rig.queue.control().current_index(), Some(1));
}

#[kithara::test(tokio)]
async fn re_arming_keeps_a_successor_already_stitched_in_and_reports_it() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    let stitched = rig.stitch();
    rig.select(2, Transition::None).expect("re-arm accepted");
    assert_eq!(detached(&rig.pending_parts()), []);
    rig.settle(stitched);
    assert_eq!(rig.queue.control().current_index(), Some(1));
    assert_eq!(rig.armed(), Some(rig.ids[2]));
}

#[kithara::test(tokio)]
async fn a_re_armed_successor_the_audio_thread_stitched_in_is_promoted() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    rig.select(2, Transition::None).expect("re-arm accepted");
    rig.load(2, 162.0).await;
    let applied = rig
        .deck
        .block(SessionFrame::new(128), 0.0)
        .expect("third entry");
    rig.event(DeckEvent::Ended {
        slot: rig.slot(0),
        at: SessionFrame::new(128),
    });
    rig.settle(applied);
    assert_eq!(rig.queue.control().current_index(), Some(2));
    assert_eq!(rig.armed(), None);
}

#[kithara::test(tokio)]
#[kithara::hang_watchdog]
async fn a_chained_successor_that_ends_in_its_stitch_block_is_reported() {
    let mut rig = HandoverRig::playing().await;
    let second = rig.arm(1).await;
    rig.chain();
    let applied = rig.stitch();
    rig.event(DeckEvent::Ended {
        slot: rig.slot(0),
        at: SessionFrame::new(128),
    });
    rig.event(DeckEvent::Ended {
        slot: second,
        at: SessionFrame::new(128),
    });
    rig.settle(applied);
    assert_eq!(rig.queue.control().current_index(), Some(1));
    assert_eq!(rig.armed(), None);
}

#[kithara::test(tokio)]
async fn a_withdrawn_successor_that_ends_in_its_stitch_block_is_reported() {
    let mut rig = HandoverRig::playing().await;
    let second = rig.arm(1).await;
    rig.chain();
    let applied = rig.stitch();
    rig.fill_to_room(1);
    assert!(rig.select(2, Transition::None).is_err());
    rig.event(DeckEvent::Ended {
        slot: rig.slot(0),
        at: SessionFrame::new(128),
    });
    rig.event(DeckEvent::Ended {
        slot: second,
        at: SessionFrame::new(128),
    });
    rig.settle(applied);
    assert_eq!(rig.queue.control().current_index(), Some(1));
}

#[kithara::test(tokio)]
async fn a_successor_whose_cancel_the_ring_refused_is_reported_when_stitched_in_later() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    rig.fill_to_room(0);
    assert!(with_outbox(&mut rig.queue, &mut rig.deck, Queue::withdraw_transition).is_err());
    let commands = rig.pending_parts();
    assert!(
        !commands
            .iter()
            .any(|part| matches!(part, DeckPart::Stop { .. })),
        "the ring refused cancellation"
    );
    let applied = rig.stitch();
    rig.select(2, Transition::None).expect("select third");
    rig.load(2, 162.0).await;
    crate::mock::finish(&mut rig.queue, &mut rig.deck);
    let third = rig.slot(2);
    rig.event(DeckEvent::Ended {
        slot: third,
        at: SessionFrame::new(256),
    });
    rig.settle(applied);
    assert_eq!(rig.queue.control().current_index(), Some(1));
}

#[kithara::test(tokio)]
async fn a_re_armed_successor_that_fails_in_its_stitch_block_is_reported() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    rig.select(2, Transition::None).expect("re-arm third");
    rig.load(2, 162.0).await;
    let applied = rig
        .deck
        .block(SessionFrame::new(128), 0.0)
        .expect("third stitch block");
    rig.event(DeckEvent::Ended {
        slot: rig.slot(0),
        at: SessionFrame::new(128),
    });
    rig.event(DeckEvent::Failed {
        slot: rig.slot(2),
        at: SessionFrame::new(128),
        fault: PlaybackFault::OutputRangeUnavailable,
    });
    rig.settle(applied);
    assert_eq!(rig.queue.control().current_index(), Some(2));
    assert_eq!(rig.armed(), None);
}

#[kithara::test(tokio)]
async fn an_unloaded_successor_whose_cancel_the_ring_refused_leaves_the_question() {
    let mut rig = HandoverRig::playing().await;
    rig.arm(1).await;
    rig.chain();
    rig.fill_to_room(0);
    assert!(with_outbox(&mut rig.queue, &mut rig.deck, Queue::withdraw_transition).is_err());
    rig.pending_parts();
    rig.select(2, Transition::None).expect("re-arm third");
    rig.load(2, 162.0).await;
    let applied = rig
        .deck
        .block(SessionFrame::new(128), 0.0)
        .expect("applied detach and third entry");
    let outgoing = rig.slot(0);
    rig.settle(applied);
    rig.event(DeckEvent::Ended {
        slot: outgoing,
        at: SessionFrame::new(128),
    });
    assert_eq!(rig.queue.control().current_index(), Some(2));
    assert_eq!(rig.armed(), None);
    assert!(
        rig.queue
            .active
            .iter()
            .all(|active| active.item != rig.ids[1])
    );
}

#[kithara::test(tokio)]
async fn eof_playback_stopped_notification_maps_to_item_end_event() {
    let mut rig = HandoverRig::playing().await;
    rig.only_current(0);
    let mut receiver = rig.queue.control().subscribe();
    rig.event(DeckEvent::Ended {
        slot: rig.slot(0),
        at: SessionFrame::new(128),
    });
    let observed = events(&mut receiver);
    assert_eq!(observed.len(), 1);
    assert!(matches!(observed[0], QueueEvent::QueueEnded));
}

#[kithara::test(tokio)]
async fn playback_stopped_notification_does_not_map_to_item_end_event() {
    let mut rig = HandoverRig::playing().await;
    rig.only_current(0);
    let mut receiver = rig.queue.control().subscribe();
    rig.apply(QueueCommand::Pause { at: When::Next })
        .expect("ordinary stop");
    let stopped = rig
        .deck
        .block(SessionFrame::new(128), 0.0)
        .expect("stop block");
    rig.settle(stopped);
    assert!(
        events(&mut receiver).is_empty(),
        "ordinary stop is not a natural end"
    );
}

#[kithara::test(tokio)]
async fn end_of_the_held_slot_is_the_leading_track() {
    let mut rig = HandoverRig::playing().await;
    rig.only_current(0);
    let held = rig.slot(0);
    let current = rig.queue.current_track().expect("leader").snapshot();
    assert_eq!(
        (current.as_ref().item, current.as_ref().slot),
        (rig.ids[0], Some(held))
    );
    assert_eq!(
        rig.source(0),
        rig.dir.path().join("item-1.wav").to_str().expect("source")
    );
    assert_eq!(
        rig.queue.active.get(0).expect("held track").role,
        Role::Current
    );
    let mut receiver = rig.queue.control().subscribe();
    rig.event(DeckEvent::Ended {
        slot: held,
        at: SessionFrame::new(128),
    });
    let observed = events(&mut receiver);
    assert_eq!(observed.len(), 1);
    assert!(matches!(observed[0], QueueEvent::QueueEnded));
}

#[kithara::test(tokio)]
async fn end_of_a_slot_the_phase_does_not_hold_is_a_background_track() {
    let mut rig = HandoverRig::playing().await;
    let background = rig.arm(1).await;
    let active = rig
        .queue
        .active
        .iter()
        .find(|active| active.item == rig.ids[1])
        .expect("background track");
    assert_eq!(
        (active.item, active.slot, active.role),
        (rig.ids[1], background, Role::Preloaded)
    );
    assert_ne!(background, rig.slot(0));
    assert_eq!(
        rig.source(1),
        rig.dir.path().join("item-2.wav").to_str().expect("source")
    );
    let mut receiver = rig.queue.control().subscribe();
    rig.event(DeckEvent::Ended {
        slot: background,
        at: SessionFrame::new(128),
    });
    assert_eq!(rig.queue.current, Some(rig.ids[0]));
    assert!(events(&mut receiver).is_empty());
}

#[kithara::test(tokio)]
async fn the_outgoing_half_of_a_crossfade_is_not_the_leading_track() {
    let mut rig = HandoverRig::playing_with_sources(["item-1.wav"; 3]).await;
    let same = rig.source(0).to_owned();
    rig.queue.tracks.records_mut()[1].source = TrackSource::Uri(same.clone());
    rig.arm(1).await;
    let receipts = rig.entry(1, Transition::Crossfade);
    rig.settle(receipts);
    let outgoing = rig.slot(0);
    let active = rig
        .queue
        .active
        .iter()
        .find(|active| active.item == rig.ids[0])
        .expect("outgoing track");
    assert_eq!(
        (active.item, active.slot, active.role),
        (rig.ids[0], outgoing, Role::Outgoing)
    );
    assert_eq!(rig.source(0), same);
    assert_eq!(rig.source(1), same);
    let mut receiver = rig.queue.control().subscribe();
    rig.event(DeckEvent::Ended {
        slot: outgoing,
        at: SessionFrame::new(128),
    });
    assert_eq!(rig.queue.current, Some(rig.ids[1]));
    assert!(
        events(&mut receiver).is_empty(),
        "an outgoing end never advances a second time"
    );
}

#[kithara::test(tokio)]
async fn the_promoted_half_of_a_crossfade_is_the_leading_track() {
    let mut rig = HandoverRig::playing_with_sources(["item-1.wav"; 3]).await;
    let same = rig.source(0).to_owned();
    rig.queue.tracks.records_mut()[1].source = TrackSource::Uri(same.clone());
    rig.arm(1).await;
    let receipts = rig.entry(1, Transition::Crossfade);
    rig.settle(receipts);
    let promoted = rig.slot(1);
    let active = rig
        .queue
        .active
        .iter()
        .find(|active| active.item == rig.ids[1])
        .expect("promoted track");
    assert_eq!(
        (active.item, active.slot, active.role),
        (rig.ids[1], promoted, Role::Current)
    );
    assert_eq!(rig.source(0), same);
    assert_eq!(rig.source(1), same);
    rig.only_current(1);
    let mut receiver = rig.queue.control().subscribe();
    rig.event(DeckEvent::Ended {
        slot: promoted,
        at: SessionFrame::new(128),
    });
    let observed = events(&mut receiver);
    assert_eq!(observed.len(), 1);
    assert!(matches!(observed[0], QueueEvent::QueueEnded));
}

#[kithara::test(tokio)]
async fn start_of_a_slot_the_phase_does_not_hold_is_a_background_item() {
    let mut rig = HandoverRig::playing().await;
    let background = rig.arm(1).await;
    let mut receiver = rig.queue.control().subscribe();
    with_outbox(&mut rig.queue, &mut rig.deck, |queue, out| {
        queue
            .active
            .iter_mut()
            .find(|active| active.item == rig.ids[1])
            .expect("background item")
            .track
            .apply(
                TrackCommand::Play {
                    at: When::At(SessionFrame::new(128)),
                },
                out,
            )
    })
    .expect("background start");
    let started = rig
        .deck
        .block(SessionFrame::new(128), 0.0)
        .expect("start block");
    rig.settle(started);
    let active = rig
        .queue
        .active
        .iter()
        .find(|active| active.item == rig.ids[1])
        .expect("background item");
    assert_eq!(
        (active.item, active.slot, active.role),
        (rig.ids[1], background, Role::Preloaded)
    );
    assert!(matches!(
        active.track.snapshot().as_ref().status,
        PlayingStatus::Playing { .. }
    ));
    assert_eq!(
        rig.source(1),
        rig.dir.path().join("item-2.wav").to_str().expect("source")
    );
    assert_eq!(rig.queue.current, Some(rig.ids[0]));
    assert!(events(&mut receiver).is_empty());
}
