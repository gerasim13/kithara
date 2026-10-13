use std::{num::NonZeroU32, ops::Deref, task::Waker};

use kithara::{
    bufpool::HasPool,
    events::TrackId,
    host::{DeckId, HostCommand, HostCore},
    link::{
        GridAnswer, LinkedDeck, LinkedFactory, LinkedHost, LinkedPlayer, LinkedSnapshot,
        TempoTrajectory,
    },
    platform::sync::{Arc, Mutex},
    play::{
        Bound, DeckPass, HostedDeck, Outbox, PlayError, PlayWorker, Player, PlayerConfig,
        PlayerFactory, PlayerImpl, Position, ResourcePrep, Settled, Track, TrackCommand,
        TrackFactory, TrackReceipt, TrackSettings, TrackSettingsChange, TrackSnapshot,
    },
    queue::{Queue, QueueCommand, QueueControl, TrackSource, Transition},
    signal::{FrameCount, SessionFrame},
};
use kithara_command::{Outcome, Rejection, Seq, When};
use kithara_render::{
    LoadRefusal,
    bridge::{DeckPart, SlotSnapshot},
    rt::DeckMixerConfig,
};

use super::OfflineHostHarness;

pub type LinkedOwner<S> = LinkedHost<S, HostCore<S, dyn LinkedDeck<S>>>;
pub type LinkedQueue<S> = Queue<S, LinkedFactory<ObservedFactory<S>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkVerdict {
    Applied,
    Capacity,
    Stale,
    Cancelled,
    Refused,
}

pub enum LinkProbeCommand<S: HasPool<u8> + Send + Sync + 'static> {
    Track {
        item: TrackId,
        command: TrackCommand<S>,
    },
    Incoming {
        item: TrackId,
        source: TrackSource<S>,
        cue: Option<Position>,
    },
}

struct LinkProbes<S: HasPool<u8> + Send + Sync + 'static> {
    commands: Vec<LinkProbeCommand<S>>,
    wake: Option<Waker>,
}

#[derive(Default)]
pub struct LinkObservation {
    pub tracks: Vec<LinkedSnapshot<TrackSnapshot>>,
    pub consumed: Vec<(TrackId, SlotSnapshot)>,
    pub consumed_rate: Option<NonZeroU32>,
    pub placements: Vec<LinkPlacement>,
    pub placement_applied: Vec<(TrackId, Seq, bool)>,
    pub placement_receipts: Vec<(TrackId, LinkReceiptOrigin, Settled)>,
    pub sync_requests: Vec<(bool, Option<Seq>)>,
    pub settled: Vec<(TrackId, Settled)>,
    pub loads: Vec<(TrackId, Seq)>,
    pub cues: Vec<(TrackId, Seq)>,
    pub starts: Vec<(TrackId, Seq)>,
    pub commands: Vec<(TrackId, Seq)>,
    pub dispatcher: Vec<(Seq, LinkVerdict)>,
    pub deck: Vec<(Seq, LinkVerdict)>,
    pub ready_cues: Vec<(TrackId, Seq)>,
    pub timeline: Vec<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkReceiptOrigin {
    Deck,
    Lane,
}

#[derive(Debug)]
pub struct LinkPlacement {
    pub item: TrackId,
    pub seq: Seq,
    pub aligned: bool,
    pub to: Position,
    pub at: SessionFrame,
    pub now: SessionFrame,
    pub delivery: FrameCount,
    pub before: TrackSnapshot,
    pub consumed: SlotSnapshot,
    pub planned: Result<(Position, f32), PlayError>,
}

pub struct ObservedFactory<S> {
    observation: Arc<Mutex<LinkObservation>>,
    schema: std::marker::PhantomData<fn() -> S>,
}

impl<S> ObservedFactory<S> {
    pub fn new(observation: Arc<Mutex<LinkObservation>>) -> Self {
        Self {
            observation,
            schema: std::marker::PhantomData,
        }
    }
}

pub struct ObservedTrack<S> {
    inner: PlayerImpl<S>,
    observation: Arc<Mutex<LinkObservation>>,
}

impl<S> TrackFactory<S> for ObservedFactory<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Track = ObservedTrack<S>;

    fn track(&self, config: PlayerConfig) -> Result<Self::Track, PlayError> {
        Ok(ObservedTrack {
            inner: PlayerFactory.track(config)?,
            observation: self.observation.clone(),
        })
    }
}

impl<S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static> ObservedTrack<S> {
    fn trace(&self, event: &str, details: &serde_json::Value, out: &Outbox<'_, S>) {
        let snapshot = self.inner.snapshot();
        let pass = out.pass();
        let slot = pass.and_then(|pass| {
            snapshot
                .slot
                .and_then(|slot| pass.deck.slots.get(usize::from(slot.get())).copied())
        });
        self.observation.lock().timeline.push(serde_json::json!({
            "event": event,
            "details": details,
            "item": u64::from(snapshot.item),
            "pass": pass.map(|pass| serde_json::json!({"now": i64::from(pass.now), "delivery": pass.delivery.get(), "sample_rate": pass.deck.sample_rate})),
            "slot": slot.map(|slot| format!("{slot:?}")),
            "track": format!("{snapshot:?}"),
        }));
    }

    fn record(&self, settled: &Settled, origin: Option<LinkReceiptOrigin>) {
        if !matches!(settled, Settled::Pending) {
            let item = self.inner.snapshot().item;
            let mut observation = self.observation.lock();
            observation.settled.push((item, settled.clone()));
            if let Some(origin) = origin {
                observation
                    .placement_receipts
                    .push((item, origin, settled.clone()));
            }
        }
    }
}

impl<S> Player<S> for ObservedTrack<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Command = TrackCommand<S>;
    type Snapshot = TrackSnapshot;

    delegate::delegate! {
        to self.inner {
            fn entry(&self, bound: Bound) -> Option<SessionFrame>;

            fn snapshot(&self) -> Self::Snapshot;
        }
    }

    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>) {
        self.inner.tick(now, out);
        let item = self.inner.snapshot().item;
        let pending_placements = {
            let observation = self.observation.lock();
            observation
                .placements
                .iter()
                .filter_map(|placement| {
                    (placement.item == item
                        && !observation
                            .placement_applied
                            .iter()
                            .any(|(owner, seq, _)| *owner == item && *seq == placement.seq))
                    .then_some(placement.seq)
                })
                .collect::<Vec<_>>()
        };
        for seq in pending_placements {
            if let Some(applied) = self.inner.speed_applied(seq) {
                self.observation
                    .lock()
                    .placement_applied
                    .push((item, seq, applied));
            }
        }
        let pending: Vec<_> = self
            .observation
            .lock()
            .cues
            .iter()
            .filter_map(|(owner, seq)| (*owner == item).then_some(*seq))
            .collect();
        for seq in pending {
            if self.inner.speed_applied(seq) == Some(true) {
                let mut observation = self.observation.lock();
                if !observation.ready_cues.contains(&(item, seq)) {
                    observation.ready_cues.push((item, seq));
                }
            }
        }
    }

    fn apply(
        &mut self,
        command: Self::Command,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let details = match &command {
            TrackCommand::Play { at } => Some(serde_json::json!({"Play": format!("{at:?}")})),
            TrackCommand::Pause { at } => Some(serde_json::json!({"Pause": format!("{at:?}")})),
            TrackCommand::Seek { to } => {
                Some(serde_json::json!({"Seek": to.as_nanos().to_string()}))
            }
            TrackCommand::Jump { to, at } => Some(
                serde_json::json!({"Jump": {"to_ns": to.as_nanos().to_string(), "at": i64::from(*at)}}),
            ),
            TrackCommand::Align { to, speed, at } => Some(
                serde_json::json!({"Align": {"to_ns": to.as_nanos().to_string(), "speed": format!("{speed:?}"), "at": i64::from(*at)}}),
            ),
            TrackCommand::SetSpeed { speed, at } => Some(
                serde_json::json!({"SetSpeed": {"speed": format!("{speed:?}"), "at": format!("{at:?}")}}),
            ),
            _ => None,
        };
        if let Some(details) = details.as_ref() {
            self.trace("command", details, out);
        }
        let loading = matches!(command, TrackCommand::Load { .. });
        let aligned = matches!(command, TrackCommand::Align { .. });
        let placement = out.pass().and_then(|pass| {
            let before = self.inner.snapshot();
            let consumed = before
                .slot
                .and_then(|slot| pass.deck.slots.get(usize::from(slot.get())).copied())?;
            match &command {
                TrackCommand::Jump { to, at } | TrackCommand::Align { to, at, .. } => {
                    let rate = NonZeroU32::new(pass.deck.sample_rate)?;
                    let planned = self.inner.planned(*at, rate);
                    Some((
                        (*to, planned),
                        *at,
                        pass.now,
                        pass.delivery,
                        before,
                        consumed,
                    ))
                }
                _ => None,
            }
        });
        let result = self.inner.apply(command, out);
        if details.is_some() {
            self.trace("admission", &serde_json::json!(format!("{result:?}")), out);
        }
        let seq = result?;
        if let Some(seq) = seq
            && let Some(((to, planned), at, now, delivery, before, consumed)) = placement
        {
            let item = before.item;
            let placement = LinkPlacement {
                item,
                seq,
                aligned,
                to,
                at,
                now,
                delivery,
                before,
                consumed,
                planned,
            };
            self.observation.lock().placements.push(placement);
        }
        if loading && let Some(seq) = seq {
            self.observation
                .lock()
                .loads
                .push((self.inner.snapshot().item, seq));
        }
        Ok(seq)
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        let origin =
            matches!(receipt, TrackReceipt::Deck { .. }).then_some(LinkReceiptOrigin::Deck);
        if let TrackReceipt::Deck {
            seq,
            outcome: Outcome::Applied { .. },
            batch,
        } = &receipt
        {
            let snapshot = self.inner.snapshot();
            if batch.commands.iter().any(
                |part| matches!(part, DeckPart::Start { slot, .. } if Some(*slot) == snapshot.slot),
            ) {
                let mut observation = self.observation.lock();
                let start = (snapshot.item, *seq);
                if !observation.starts.contains(&start) {
                    observation.starts.push(start);
                }
            }
        }
        if let TrackReceipt::Deck { seq, outcome, .. } = &receipt {
            self.trace(
                "deck receipt",
                &serde_json::json!({"seq": seq.get(), "outcome": format!("{outcome:?}")}),
                out,
            );
        }
        let settled = self.inner.settle(receipt, out);
        self.record(&settled, origin);
        settled
    }
}

impl<S> Track<S> for ObservedTrack<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    delegate::delegate! {
        to self.inner {
            fn admit(&mut self, change: TrackSettingsChange, at: When<SessionFrame>, out: &Outbox<'_, S>) -> Result<(), PlayError>;
            fn projected(&self) -> TrackSettings;
            fn prepared_speed(&self) -> Option<&kithara::warp::SpeedCurve>;
            fn prepare_entry(&mut self, at: SessionFrame, out: &mut Outbox<'_, S>) -> Result<bool, PlayError>;
            fn planned_end(&self, sample_rate: NonZeroU32) -> Result<Option<SessionFrame>, PlayError>;
            fn speed_applied(&mut self, seq: Seq) -> Option<bool>;
            fn finish_group(&mut self, result: Result<Seq, &mut Vec<DeckPart>>);
        }
    }

    fn planned(
        &self,
        at: SessionFrame,
        sample_rate: NonZeroU32,
    ) -> Result<(Position, f32), PlayError> {
        let result = self.inner.planned(at, sample_rate);
        self.observation.lock().timeline.push(serde_json::json!({
            "event": "planned",
            "item": u64::from(self.inner.snapshot().item),
            "at": i64::from(at),
            "sample_rate": sample_rate.get(),
            "result": format!("{result:?}"),
            "track": format!("{:?}", self.inner.snapshot()),
        }));
        result
    }

    fn cue(
        &mut self,
        position: Position,
        speed: kithara::warp::SpeedCurve,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        self.trace(
            "cue",
            &serde_json::json!({"position_ns": position.as_nanos().to_string(), "speed": format!("{speed:?}")}),
            out,
        );
        let seq = self.inner.cue(position, speed, out)?;
        if let Some(seq) = seq {
            let item = self.inner.snapshot().item;
            let mut observation = self.observation.lock();
            observation.cues.push((item, seq));
        }
        Ok(seq)
    }

    fn speed_receipt(&mut self) -> Option<Settled> {
        let receipt = self.inner.speed_receipt();
        if let Some(receipt) = &receipt {
            self.record(receipt, Some(LinkReceiptOrigin::Lane));
        }
        receipt
    }
}

pub struct LinkedQueueHandle<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    id: DeckId,
    control: QueueControl<S>,
    observation: Arc<Mutex<LinkObservation>>,
    commands: Arc<Mutex<LinkProbes<S>>>,
}

impl<S> LinkedQueueHandle<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub fn probe(&self, command: LinkProbeCommand<S>) {
        let wake = {
            let mut probes = self.commands.lock();
            probes.commands.push(command);
            probes.wake.clone()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }

    delegate::delegate! {
        to self {
            #[field(id)]
            pub const fn id(&self) -> DeckId;
            #[field(&control)]
            pub const fn control(&self) -> &QueueControl<S>;
        }
    }

    pub fn observe<Value>(&self, read: impl FnOnce(&LinkObservation) -> Value) -> Value {
        read(&self.observation.lock())
    }
    pub fn track_snapshot(&self, item: TrackId) -> Option<LinkedSnapshot<TrackSnapshot>> {
        self.observe(|observation| {
            observation
                .tracks
                .iter()
                .find(|track| track.track.item == item)
                .cloned()
        })
    }
}

impl<S> Deref for LinkedQueueHandle<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Target = QueueControl<S>;
    fn deref(&self) -> &Self::Target {
        &self.control
    }
}

struct ObservedQueue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    inner: LinkedQueue<S>,
    observation: Arc<Mutex<LinkObservation>>,
    commands: Arc<Mutex<LinkProbes<S>>>,
}

impl<S> ObservedQueue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn publish(&self) {
        self.observation.lock().tracks = self.inner.tracks_active().map(Player::snapshot).collect();
    }

    fn publish_pass(&self, pass: DeckPass<'_>) {
        self.publish();
        let mut observation = self.observation.lock();
        observation.consumed_rate = NonZeroU32::new(pass.deck.sample_rate);
        observation.consumed = observation
            .tracks
            .iter()
            .filter_map(|track| {
                let slot = track.track.slot?;
                pass.deck
                    .slots
                    .get(usize::from(slot.get()))
                    .copied()
                    .map(|slot| (track.track.item, slot))
            })
            .collect();
    }
}

impl<S> HostedDeck<S> for ObservedQueue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    delegate::delegate! {
        to self.inner {
            fn worker(&self) -> Option<&PlayWorker<S>>;
            fn resource_prep(&self) -> Option<&ResourcePrep<S>>;
            fn mixer_config(&self) -> DeckMixerConfig;
            fn close(&mut self, out: &mut Outbox<'_, S>) -> Result<(), PlayError>;
            fn release(&mut self);
        }
    }

    fn hold(&mut self, waker: Waker) {
        self.commands.lock().wake = Some(waker.clone());
        self.inner.hold(waker);
    }

    fn drain(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.inner.drain(pass, out);
        let commands = std::mem::take(&mut self.commands.lock().commands);
        for command in commands {
            match command {
                LinkProbeCommand::Track { item, command } => {
                    let track = self
                        .inner
                        .tracks_mut()
                        .find(|track| track.snapshot().track.item == item)
                        .expect("probe targets an active track");
                    if let Some(seq) = track
                        .apply(command, out)
                        .expect("real track command admitted")
                    {
                        self.observation.lock().commands.push((item, seq));
                    }
                }
                LinkProbeCommand::Incoming { item, source, cue } => {
                    self.inner.factory_mut().set_synced(true);
                    self.inner
                        .apply(QueueCommand::Append { id: item, source }, out)
                        .expect("append incoming track");
                    self.inner
                        .apply(
                            QueueCommand::Select {
                                id: item,
                                transition: Transition::None,
                            },
                            out,
                        )
                        .expect("select incoming track");
                    if let Some(cue) = cue {
                        let track = self
                            .inner
                            .tracks_mut()
                            .find(|track| track.snapshot().track.item == item)
                            .expect("selected incoming track");
                        track
                            .apply(TrackCommand::Seek { to: cue }, out)
                            .expect("incoming cue waits for its grid");
                    }
                }
            }
        }
        self.publish_pass(pass);
    }
    fn settle(
        &mut self,
        receipt: TrackReceipt<'_, S>,
        pass: DeckPass<'_>,
        out: &mut Outbox<'_, S>,
    ) {
        if let TrackReceipt::Loaded(receipt) = &receipt {
            let verdict = match receipt.outcome() {
                Outcome::Applied { .. } => LinkVerdict::Applied,
                Outcome::Rejected(Rejection::Refused(LoadRefusal::Capacity { .. })) => {
                    LinkVerdict::Capacity
                }
                Outcome::Rejected(Rejection::Refused(LoadRefusal::Cancelled)) => {
                    LinkVerdict::Cancelled
                }
                Outcome::Rejected(_) => LinkVerdict::Refused,
            };
            self.observation
                .lock()
                .dispatcher
                .push((receipt.seq(), verdict));
        }
        if let TrackReceipt::Deck { seq, outcome, .. } = &receipt {
            let verdict = match outcome {
                Outcome::Applied { .. } => LinkVerdict::Applied,
                Outcome::Rejected(Rejection::Stale) => LinkVerdict::Stale,
                Outcome::Rejected(_) => LinkVerdict::Refused,
            };
            self.observation.lock().deck.push((*seq, verdict));
        }
        HostedDeck::settle(&mut self.inner, receipt, pass, out);
        self.publish_pass(pass);
    }
    fn tick(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        HostedDeck::tick(&mut self.inner, pass, out);
        self.publish_pass(pass);
    }
}

impl<S> Player<S> for ObservedQueue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    type Command = QueueCommand<S>;
    type Snapshot = kithara::queue::QueueSnapshot<S>;

    fn entry(&self, bound: Bound) -> Option<SessionFrame> {
        Player::entry(&self.inner, bound)
    }
    fn apply(
        &mut self,
        command: Self::Command,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let seq = Player::apply(&mut self.inner, command, out)?;
        self.publish();
        Ok(seq)
    }
    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        let settled = Player::settle(&mut self.inner, receipt, out);
        self.publish();
        settled
    }
    fn tick(&mut self, now: SessionFrame, out: &mut Outbox<'_, S>) {
        Player::tick(&mut self.inner, now, out);
        self.publish();
    }
    fn snapshot(&self) -> Self::Snapshot {
        Player::snapshot(&self.inner)
    }
}

impl<S> LinkedPlayer<S> for ObservedQueue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        let seq = LinkedPlayer::sync(&mut self.inner, on, out)?;
        self.observation.lock().sync_requests.push((on, seq));
        self.publish();
        Ok(seq)
    }
    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        LinkedPlayer::retime(&mut self.inner, trajectory, at, out);
        self.publish();
    }
    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>) {
        LinkedPlayer::grid(&mut self.inner, answer, out);
        self.publish();
    }
    fn synced(&self) -> bool {
        LinkedPlayer::synced(&self.inner)
    }
    fn lead(&self, delivery: FrameCount) -> Option<FrameCount> {
        LinkedPlayer::lead(&self.inner, delivery)
    }
    fn lane_room(&self) -> usize {
        LinkedPlayer::lane_room(&self.inner)
    }
    fn scope_parts(&self) -> usize {
        LinkedPlayer::scope_parts(&self.inner)
    }
    fn retime_applied(&mut self, at: SessionFrame) -> Option<bool> {
        LinkedPlayer::retime_applied(&mut self.inner, at)
    }
    fn realign(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        LinkedPlayer::realign(&mut self.inner, trajectory, at, out);
        self.publish();
    }
}

impl<S> OfflineHostHarness<S, LinkedOwner<S>>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub async fn insert_linked(
        &self,
        queue: LinkedQueue<S>,
        observation: Arc<Mutex<LinkObservation>>,
    ) -> Result<LinkedQueueHandle<S>, PlayError> {
        self.with(move |host| {
            let id = host.deck_id()?;
            let control = queue.control();
            let commands = Arc::new(Mutex::new(LinkProbes {
                commands: Vec::new(),
                wake: None,
            }));
            host.send(
                HostCommand::Register {
                    id,
                    deck: Box::new(ObservedQueue {
                        inner: queue,
                        observation: observation.clone(),
                        commands: commands.clone(),
                    }) as Box<dyn LinkedDeck<S>>,
                }
                .into(),
            )?;
            Ok(LinkedQueueHandle {
                id,
                control,
                observation,
                commands,
            })
        })
        .await
    }
}
