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
use kithara_render::{LoadRefusal, bridge::DeckPart, rt::DeckMixerConfig};

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
        cue: Position,
    },
}

#[derive(Default)]
pub struct LinkObservation {
    pub tracks: Vec<LinkedSnapshot<TrackSnapshot>>,
    pub sync_requests: Vec<(bool, Option<Seq>)>,
    pub settled: Vec<(TrackId, Settled)>,
    pub loads: Vec<(TrackId, Seq)>,
    pub cues: Vec<(TrackId, Seq)>,
    pub starts: Vec<(TrackId, Seq)>,
    pub commands: Vec<(TrackId, Seq)>,
    pub dispatcher: Vec<(Seq, LinkVerdict)>,
    pub deck: Vec<(Seq, LinkVerdict)>,
    pub ready_cues: Vec<(TrackId, Seq)>,
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
    fn record(&self, settled: &Settled) {
        if !matches!(settled, Settled::Pending) {
            self.observation
                .lock()
                .settled
                .push((self.inner.snapshot().item, settled.clone()));
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
        let loading = matches!(command, TrackCommand::Load { .. });
        let starting = matches!(command, TrackCommand::Play { .. });
        let seq = self.inner.apply(command, out)?;
        if loading && let Some(seq) = seq {
            self.observation
                .lock()
                .loads
                .push((self.inner.snapshot().item, seq));
        }
        if starting && let Some(seq) = seq {
            self.observation
                .lock()
                .starts
                .push((self.inner.snapshot().item, seq));
        }
        Ok(seq)
    }

    fn settle(&mut self, receipt: TrackReceipt<'_, S>, out: &mut Outbox<'_, S>) -> Settled {
        let settled = self.inner.settle(receipt, out);
        self.record(&settled);
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
            fn planned(&self, at: SessionFrame, sample_rate: NonZeroU32) -> Result<(Position, f32), PlayError>;
            fn planned_end(&self, sample_rate: NonZeroU32) -> Result<Option<SessionFrame>, PlayError>;
            fn speed_applied(&mut self, seq: Seq) -> Option<bool>;
            fn finish_group(&mut self, result: Result<Seq, &mut Vec<DeckPart>>);
        }
    }

    fn cue(
        &mut self,
        position: Position,
        speed: f32,
        out: &mut Outbox<'_, S>,
    ) -> Result<Option<Seq>, PlayError> {
        let seq = self.inner.cue(position, speed, out)?;
        if let Some(seq) = seq {
            self.observation
                .lock()
                .cues
                .push((self.inner.snapshot().item, seq));
        }
        Ok(seq)
    }

    fn speed_receipt(&mut self) -> Option<Settled> {
        let receipt = self.inner.speed_receipt();
        if let Some(receipt) = &receipt {
            self.record(receipt);
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
    commands: Arc<Mutex<Vec<LinkProbeCommand<S>>>>,
}

impl<S> LinkedQueueHandle<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    pub fn probe(&self, command: LinkProbeCommand<S>) {
        self.commands.lock().push(command);
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
    commands: Arc<Mutex<Vec<LinkProbeCommand<S>>>>,
}

impl<S> ObservedQueue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn publish(&self) {
        self.observation.lock().tracks = self.inner.tracks_active().map(Player::snapshot).collect();
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
            fn hold(&mut self, waker: Waker);
            fn release(&mut self);
        }
    }

    fn drain(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        self.inner.drain(pass, out);
        let commands = std::mem::take(&mut *self.commands.lock());
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
        self.publish();
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
        self.publish();
    }
    fn tick(&mut self, pass: DeckPass<'_>, out: &mut Outbox<'_, S>) {
        HostedDeck::tick(&mut self.inner, pass, out);
        self.publish();
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
            let commands = Arc::new(Mutex::new(Vec::new()));
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
