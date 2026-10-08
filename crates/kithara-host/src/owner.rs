//! The owner a session drives and the commands its handle posts.

use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::HasPool;
use kithara_command::{Batch, ChannelConfig, LiveError, Outcome, Port, Rejection, ScopedReceipt, SendError, Sender, Seq, When, channel};
use kithara_platform::maybe_send::MaybeSend;
use kithara_play::{DeckPass, HostedDeck, Outbox, PlayError, ResourceLoad, TrackReceipt};
use kithara_render::{
    DispatcherProtocol,
    bridge::{DeckMixSettingsChange, DeckPart},
};
use kithara_worker::TaskHandle;
use kithara_signal::{FrameCount, SessionFrame};

pub use kithara_render::bridge::DeckEqChange as EqPart;
pub use kithara_play::DeckControl;

use crate::{
    HostSettingsChange, HostSettingsExec,
    session::{
        SessionError,
        decks::{Deck, DeckInbox, DeckWake, Decks},
        dispatch::tick_session,
        graph,
        state::{SessionState, SessionStream},
        transport::TransportState,
    },
};

/// The existing identity used for decks and their beat grids.
pub type DeckId = kithara_warp::BeatGridId;

/// What the session thread drives, including decorators over its base owner.
pub trait HostOwner<S>:
    HostSettingsExec<(), At = When<SessionFrame>, Output = Result<Option<Seq>, PlayError>>
    + 'static
{
    /// Commands posted by the Host handle.
    type Command: From<HostCommand<S, Self::Deck>> + MaybeSend;
    /// The deck objects held by this owner.
    type Deck: ?Sized + HostedDeck<S>;

    /// Runs a command and returns the batch it sent, if any.
    fn apply(&mut self, command: Self::Command) -> Result<Option<Seq>, PlayError>;
    /// Builds and holds a deck's mixer and owner record.
    fn register(&mut self, id: DeckId, deck: Box<Self::Deck>) -> Result<(), PlayError>;
    /// Lends each deck its outbox and current pass.
    fn each_deck(
        &mut self,
        visit: &mut dyn FnMut(DeckId, &mut Self::Deck, &mut Outbox<'_, S>, DeckPass<'_>),
    );
    /// Lends the named deck its outbox and current pass.
    fn with_deck(
        &mut self,
        id: DeckId,
        visit: &mut dyn FnMut(&mut Self::Deck, &mut Outbox<'_, S>, DeckPass<'_>),
    ) -> Result<(), PlayError>;
    /// Current frame and delivery lead, absent before a render graph exists.
    fn clock(&self) -> Option<(SessionFrame, FrameCount)>;
    /// Prepares the offline stream before the block's owner publication.
    fn prepare_offline(&mut self) -> Result<(), PlayError>;
    /// Processes one offline block after the owner has published its commands.
    fn render_offline(
        &mut self,
        position: u64,
        frames: usize,
        output: &mut [f32],
    ) -> Result<(), PlayError>;
    /// Reads the latest applied transport anchor from its processor observation.
    fn transport(&mut self) -> Option<crate::api::SessionTransportSnapshot>;
    /// Available batches in the host ring.
    fn host_room(&self) -> usize;
    /// Reads the iteration clock and settles receipts before handling posts.
    fn begin_pass(&mut self);
    /// A release post waits for this deck's scope retirement.
    fn release_id(command: &Self::Command) -> Option<DeckId>;
    /// Identifies consecutive next-block tempo posts that share one winner.
    fn is_next_tempo(command: &Self::Command) -> bool;
    /// Settles executor answers, drives decks, and publishes the owner snapshot.
    fn pass(&mut self) -> Vec<HostSettled>;
}

/// One operation on the canonical owner, answered through its receipt.
pub enum HostCommand<S, D: ?Sized> {
    Register {
        id: DeckId,
        deck: Box<D>,
    },
    Configure(HostSettingsChange, When<SessionFrame>),
    Mix {
        deck: DeckId,
        change: DeckMixSettingsChange,
        at: When<SessionFrame>,
    },
    Eq {
        deck: DeckId,
        part: EqPart,
        at: When<SessionFrame>,
    },
    Close(DeckId),
    Release(DeckId),
    AttachOutputs { tap: crate::api::Tap, outputs: kithara_output::OutputGroup },
    DetachOutputs { tap: crate::api::Tap },
    Restart,
    Idle,
    #[doc(hidden)]
    _Marker(PhantomData<fn() -> S>),
}

/// The final outcome of a host-settings batch.
pub enum HostSettled {
    Settings {
        seq: Seq,
        change: HostSettingsChange,
        outcome: Result<SessionFrame, Rejection<PlayError>>,
    },
    Batch {
        seq: Seq,
        outcome: Result<SessionFrame, Rejection<PlayError>>,
    },
    Replanned { from: Seq, to: Seq },
    Closed { deck: DeckId },
}

/// The base owner of a session, its deck records and its single load dispatcher.
pub struct HostCore<S, D: ?Sized + HostedDeck<S> = dyn HostedDeck<S>> {
    pub(crate) session: SessionState<SessionStream, S>,
    decks: Decks<S, D>,
    dispatcher: Sender<DispatcherProtocol<ResourceLoad<S>>>,
    dispatcher_inbox: Option<kithara_command::Inbox<DispatcherProtocol<ResourceLoad<S>>>>,
    dispatcher_task: Option<TaskHandle>,
    retired_dispatches: Vec<Seq>,
    retired_lanes: Vec<kithara_render::LaneId>,
    inbox: kithara_platform::sync::Arc<dyn DeckInbox>,
}

impl<S, D: ?Sized + HostedDeck<S>> HostCore<S, D>
where S: HasPool<f32> + Send + Sync + 'static,
{
    pub(crate) fn new(
        session: SessionState<SessionStream, S>,
        inbox: kithara_platform::sync::Arc<dyn DeckInbox>,
    ) -> Self {
        let (dispatcher, dispatcher_inbox) = channel(ChannelConfig::builder().build());
        Self {
            session,
            decks: Decks::default(),
            dispatcher,
            dispatcher_inbox: Some(dispatcher_inbox),
            dispatcher_task: None,
            retired_dispatches: Vec::new(),
            retired_lanes: Vec::new(),
            inbox,
        }
    }

    fn close(&mut self, id: DeckId) -> Result<(), PlayError> {
        let index = self.decks.index(id)?;
        if self.decks.0[index].1.releasing { return Ok(()); }
        let mut result = Ok(());
        self.with_deck(id, &mut |deck, out, _pass| { result = deck.close(out); })?;
        result?;
        let record = &mut self.decks.0[index].1;
        self.session.channel.as_mut().ok_or(PlayError::Closed)?
            .close(record.scope).map_err(|error| PlayError::Internal(error.to_string()))?;
        record.releasing = true;
        Ok(())
    }

    fn release(&mut self, id: DeckId) -> Result<(), PlayError> {
        self.close(id)
    }

    fn idle(&mut self) -> Result<(), PlayError> {
        if self.decks.0.is_empty() { graph::idle(&mut self.session).map_err(Into::into) }
        else { crate::session::transport::prepare_route_restart(&mut self.session).map(|_| ()).map_err(Into::into) }
    }

    fn restart(&mut self) -> Result<(), PlayError> {
        crate::session::dispatch::invalidate_audio_route(&mut self.session, "host restart").map_err(Into::into)
    }

    fn publish_root(&self) {
        self.session
            .root_view
            .publish_decks(self.decks.0.iter().map(|(id, _)| *id).collect());
        self.session.publish_root();
    }
}

impl<S, D> HostSettingsExec<()> for HostCore<S, D>
where
    S: HasPool<f32> + Send + Sync + 'static,
    D: ?Sized + HostedDeck<S>,
{
    type At = When<SessionFrame>;
    type Output = Result<Option<Seq>, PlayError>;

    fn exec_sample_rate(&mut self, value: NonZeroU32, at: Self::At, cx: &mut ()) -> Self::Output {
        self.session.exec_sample_rate(value, at, cx)
    }
    fn exec_tempo(&mut self, value: crate::api::Tempo, at: Self::At, cx: &mut ()) -> Self::Output {
        self.session.exec_tempo(value, at, cx)
    }
    fn exec_live(&mut self, change: HostSettingsChange, at: Self::At, cx: &mut ()) -> Self::Output {
        self.session.exec_live(change, at, cx)
    }
}

impl<S, D> HostOwner<S> for HostCore<S, D>
where
    S: HasPool<f32> + Send + Sync + 'static,
    D: ?Sized + HostedDeck<S>,
{
    type Command = HostCommand<S, D>;
    type Deck = D;

    fn apply(&mut self, command: Self::Command) -> Result<Option<Seq>, PlayError> {
        match command {
            HostCommand::Register { id, deck } => self.register(id, deck).map(|()| None),
            HostCommand::Configure(change, at) => self.exec(change, at, &mut ()),
            HostCommand::Mix { deck, change, at } => {
                self.session.check_when(at)?;
                let index = self.decks.index(deck)?;
                let record = &mut self.decks.0[index].1;
                let mut port = self.session.channel.as_mut().ok_or(PlayError::Closed)?
                    .scope(record.scope).ok_or(PlayError::Closed)?;
                record
                    .mix
                    .send(&mut port, at, change, DeckPart::Mix)
                    .map(Some)
                    .map_err(|error| match error {
                        LiveError::Invalid(error) => PlayError::Internal(error.to_string()),
                        LiveError::Send(SendError::Full(_)) => PlayError::Full("deck"),
                        LiveError::Send(SendError::Closed(_)) => PlayError::Closed,
                        LiveError::Send(SendError::Target(_)) => {
                            PlayError::Internal("a deck mix batch names a slot".to_owned())
                        }
                    })
            }
            HostCommand::Eq { deck, part, at } => {
                self.session.check_when(at)?;
                let index = self.decks.index(deck)?;
                let record = &self.decks.0[index].1;
                let mut port = self.session.channel.as_mut().ok_or(PlayError::Closed)?
                    .scope(record.scope).ok_or(PlayError::Closed)?;
                port.send(at, Batch { basis: Vec::new(), commands: vec![DeckPart::Eq(part)] })
                    .map(Some).map_err(|error| match error {
                        SendError::Full(_) => PlayError::Full("deck"),
                        SendError::Closed(_) => PlayError::Closed,
                        SendError::Target(_) => PlayError::Internal("EQ names a slot".into()),
                    })
            }
            HostCommand::Close(id) => self.close(id).map(|()| None),
            HostCommand::Release(id) => self.release(id).map(|()| None),
            HostCommand::AttachOutputs { tap, outputs } => {
                graph::tap::attach(&mut self.session, tap, outputs).map(|()| None).map_err(Into::into)
            }
            HostCommand::DetachOutputs { tap } => {
                graph::tap::detach(&mut self.session, tap);
                Ok(None)
            }
            HostCommand::Restart => self.restart().map(|()| None),
            HostCommand::Idle => self.idle().map(|()| None),
            HostCommand::_Marker(_) => Err(PlayError::Internal(
                "a marker is not an owner command".to_owned(),
            )),
        }
    }

    fn register(&mut self, id: DeckId, deck: Box<D>) -> Result<(), PlayError> {
        if self.decks.0.iter().any(|(held, _)| *held == id) {
            return Err(SessionError::DeckAttached(id).into());
        }
        crate::session::state::ensure_ctx(&mut self.session)?;
        let worker = deck.worker().ok_or_else(|| PlayError::Internal("a hosted deck requires its resource worker".into()))?;
        #[cfg(not(target_arch = "wasm32"))]
        if self.session.worker_wake_allowance.is_zero() {
            self.session.worker_wake_allowance = worker_wake_allowance(worker);
        }
        let pools = worker.pools().clone();
        if let Some(inbox) = self.dispatcher_inbox.take() {
            self.dispatcher_task = Some(worker.start_dispatcher(inbox)
                .map_err(|error| PlayError::Internal(error.to_string()))?);
        }
        let scope = self.session.channel.as_mut().ok_or(PlayError::Closed)?
            .open(deck.mixer_config().slots().get()).map_err(|error| PlayError::Internal(error.to_string()))?;
        let (mut record, inputs) = Deck::new(deck, scope)?;
        if let Err(error) = graph::install_deck(&mut self.session, id, inputs, pools) {
            if let Some(channel) = &mut self.session.channel { let _ = channel.close(scope); }
            record.releasing = true;
            self.decks.0.push((id, record));
            return Err(error.into());
        }
        let waker = DeckWake::waker(&self.inbox, id);
        record.deck.hold(waker.clone());
        self.dispatcher.hold(waker.clone());
        if let Some(channel) = &mut self.session.channel { channel.hold(waker); }
        self.decks.0.push((id, record));
        self.with_deck(id, &mut |deck, out, pass| deck.drain(pass, out))?;
        Ok(())
    }

    fn each_deck(
        &mut self,
        visit: &mut dyn FnMut(DeckId, &mut D, &mut Outbox<'_, S>, DeckPass<'_>),
    ) {
        let clock = self.clock();
        let (now, delivery) = clock.unwrap_or((SessionFrame::new(0), FrameCount::new(0)));
        let Some(channel) = &mut self.session.channel else { return; };
        for (id, record) in &mut self.decks.0 {
            let pass = DeckPass {
                now,
                delivery,
                deck: record.snapshot.read(),
            };
            let Some(mut port) = channel.scope(record.scope) else { continue; };
            let mut out = Outbox::new(&mut port, &mut self.dispatcher)
                .track_dispatches(&mut record.dispatches);
            if clock.is_some() { out = out.in_pass(pass); }
            visit(*id, &mut record.deck, &mut out, pass);
        }
    }

    fn with_deck(
        &mut self,
        id: DeckId,
        visit: &mut dyn FnMut(&mut D, &mut Outbox<'_, S>, DeckPass<'_>),
    ) -> Result<(), PlayError> {
        let index = self.decks.index(id)?;
        let clock = self.clock();
        let (now, delivery) = clock.unwrap_or((SessionFrame::new(0), FrameCount::new(0)));
        let record = &mut self.decks.0[index].1;
        let pass = DeckPass {
            now,
            delivery,
            deck: record.snapshot.read(),
        };
        let mut port = self.session.channel.as_mut().ok_or(PlayError::Closed)?
            .scope(record.scope).ok_or(PlayError::Closed)?;
        let mut out = Outbox::new(&mut port, &mut self.dispatcher)
            .track_dispatches(&mut record.dispatches);
        if clock.is_some() { out = out.in_pass(pass); }
        visit(&mut record.deck, &mut out, pass);
        Ok(())
    }

    fn clock(&self) -> Option<(SessionFrame, FrameCount)> {
        self.session.iteration_clock
    }

    #[cfg(feature = "offline")]
    fn prepare_offline(&mut self) -> Result<(), PlayError> {
        crate::session::state::ensure_ctx(&mut self.session)?;
        if matches!(self.session.stream, Some(SessionStream::Offline(_))) {
            Ok(())
        } else {
            Err(PlayError::Internal("host is not configured for offline rendering".to_owned()))
        }
    }

    #[cfg(not(feature = "offline"))]
    fn prepare_offline(&mut self) -> Result<(), PlayError> {
        Err(PlayError::Internal("offline rendering requires the offline feature".to_owned()))
    }

    #[cfg(feature = "offline")]
    fn render_offline(
        &mut self,
        position: u64,
        frames: usize,
        output: &mut [f32],
    ) -> Result<(), PlayError> {
        let Some(SessionStream::Offline(stream)) = &mut self.session.stream else {
            return Err(PlayError::Internal("offline stream is not prepared".to_owned()));
        };
        stream.render(position, frames, output)
            .map_err(|error| PlayError::Internal(error.to_string()))
    }

    #[cfg(not(feature = "offline"))]
    fn render_offline(
        &mut self,
        _position: u64,
        _frames: usize,
        _output: &mut [f32],
    ) -> Result<(), PlayError> {
        Err(PlayError::Internal("offline rendering requires the offline feature".to_owned()))
    }

    fn transport(&mut self) -> Option<crate::api::SessionTransportSnapshot> {
        self.session.transport_observation.as_mut()?.read().snapshot()
    }

    fn host_room(&self) -> usize {
        self.session
            .channel
            .as_ref()
            .map_or(0, Port::available)
    }

    fn release_id(command: &Self::Command) -> Option<DeckId> {
        if let HostCommand::Release(id) = command { Some(*id) } else { None }
    }

    fn is_next_tempo(command: &Self::Command) -> bool {
        matches!(command, HostCommand::Configure(HostSettingsChange::Tempo(_), When::Next))
    }

    fn begin_pass(&mut self) {
        self.session.iteration_clock = self.session.ctx.as_ref().and_then(|ctx| {
            let _ = ctx.stream_info()?;
            Some((SessionFrame::new(ctx.audio_clock().samples.0), self.session.delivery()))
        });
        let (now, delivery) = self.clock().unwrap_or((SessionFrame::new(0), FrameCount::new(0)));
        loop {
            let receipt = {
                let mut receipts = self.dispatcher.receipts();
                receipts.next()
            };
            let Some(receipt) = receipt else { break; };
            let owner = self.decks.0.iter().position(|(_, record)| record.dispatches.contains(&receipt.seq()));
            let Some(index) = owner else {
                if let Some(index) = self.retired_dispatches.iter().position(|seq| *seq == receipt.seq()) {
                    self.retired_dispatches.remove(index);
                    if let Outcome::Applied { data: kithara_render::Dispatched::Loaded(loaded), .. } = receipt.outcome() {
                        self.retired_lanes.push(loaded.lane);
                    }
                    continue;
                }
                tracing::error!(seq = ?receipt.seq(), "dispatcher receipt has no owning deck");
                continue;
            };
            let record = &mut self.decks.0[index].1;
            record.dispatches.retain(|seq| *seq != receipt.seq());
            let pass = DeckPass { now, delivery, deck: record.snapshot.read() };
            if let Some(mut port) = self.session.channel.as_mut().and_then(|channel| channel.scope(record.scope)) {
                let mut out = Outbox::new(&mut port, &mut self.dispatcher).track_dispatches(&mut record.dispatches);
                if self.session.iteration_clock.is_some() { out = out.in_pass(pass); }
                record.deck.settle(TrackReceipt::Loaded(receipt), pass, &mut out);
            }
        }
        while self.dispatcher.available() != 0 {
            let Some(lane) = self.retired_lanes.last().copied() else { break; };
            match self.dispatcher.send(When::Next, Batch {
                basis: Vec::new(),
                commands: vec![kithara_render::DispatcherCommand::Release(lane)],
            }) {
                Ok(seq) => {
                    self.retired_lanes.pop();
                    self.retired_dispatches.push(seq);
                }
                Err(error) => {
                    tracing::error!(?error, "a retired load's lane could not be released");
                    break;
                }
            }
        }
        self.retire_stopped_scopes();
        self.route_receipts(now, delivery);
    }

    fn pass(&mut self) -> Vec<HostSettled> {
        if let Err(error) = tick_session(&mut self.session) { tracing::warn!(%error, "host graph pass failed"); }
        if self.clock().is_some() {
            self.poll_events();
            self.each_deck(&mut |_, deck, out, pass| {
                deck.drain(pass, out);
                deck.tick(pass, out);
            });
        }
        self.publish_root();
        if let Some(channel) = &mut self.session.channel
            && let Err(error) = channel.publish()
        { tracing::warn!(%error, "host publication gate closed"); }
        if self.session.stream.is_none() {
            self.retire_stopped_scopes();
            if self.decks.0.is_empty()
                && let Err(error) = graph::drop_idle_context(&mut self.session)
            {
                tracing::warn!(%error, "empty stopped host context could not retire");
            }
        }
        std::mem::take(&mut self.session.settled)
    }
}

impl<S, D> HostCore<S, D>
where D: ?Sized + HostedDeck<S>,
{
    fn retire_stopped_scopes(&mut self) {
        if self.session.stream.is_some() { return; }
        if let Some(store) = self.session.ctx.as_mut().and_then(|ctx| ctx.proc_store_mut())
            && let Some(transport) = store.try_get_mut::<TransportState>()
        { transport.inbox.retire_closing(); }
    }

    fn route_receipts(&mut self, now: SessionFrame, delivery: FrameCount) {
        loop {
            let Some(receipt) = self.session.channel.as_mut().and_then(|channel| channel.receipt()) else { break; };
            match receipt {
                ScopedReceipt::Root(receipt) => crate::session::queue::settle_receipt(&mut self.session, &receipt),
                ScopedReceipt::Scope(scope, receipt) => {
                    let Some(index) = self.decks.0.iter().position(|(_, record)| record.scope == scope) else {
                        tracing::error!(?scope, "scope receipt has no owning deck");
                        continue;
                    };
                    let record = &mut self.decks.0[index].1;
                    let mix = record.mix.settle(&receipt).is_some();
                    let eq = receipt.batch().commands.iter().any(|part| matches!(part, DeckPart::Eq(_) | DeckPart::Returned(kithara_render::bridge::Returned::Eq(_))));
                    if mix || eq {
                        let outcome = match receipt.outcome() {
                            Outcome::Applied { at, .. } => Ok(*at),
                            Outcome::Rejected(reason) => Err(map_deck_rejection(reason)),
                        };
                        self.session.settled.push(HostSettled::Batch { seq: receipt.seq(), outcome });
                    } else if let Some(mut port) = self.session.channel.as_mut().and_then(|channel| channel.scope(scope)) {
                        let seq = receipt.seq();
                        let (outcome, mut batch) = receipt.into();
                        let pass = DeckPass { now, delivery, deck: record.snapshot.read() };
                        let mut out = Outbox::new(&mut port, &mut self.dispatcher).track_dispatches(&mut record.dispatches);
                        if self.session.iteration_clock.is_some() { out = out.in_pass(pass); }
                        record.deck.settle(TrackReceipt::Deck { seq, outcome: &outcome, batch: &mut batch }, pass, &mut out);
                    }
                }
                ScopedReceipt::Closed(scope) => {
                    let Some(index) = self.decks.0.iter().position(|(_, record)| record.scope == scope) else { continue; };
                    let id = self.decks.0[index].0;
                    if self.session.deck_nodes.iter().any(|(deck, _)| *deck == id)
                        && let Err(error) = graph::remove_deck(&mut self.session, id)
                    { tracing::error!(%error, ?id, "closed deck node could not be reclaimed"); continue; }
                    let (_, mut record) = self.decks.0.remove(index);
                    self.retired_dispatches.append(&mut record.dispatches);
                    record.deck.release();
                    self.session.settled.push(HostSettled::Closed { deck: id });
                }
            }
        }
    }

    fn poll_events(&mut self) {
        if let Some((now, delivery)) = self.session.iteration_clock
            && let Some(channel) = &mut self.session.channel
        {
            for (_, record) in &mut self.decks.0 {
                let pass = DeckPass { now, delivery, deck: record.snapshot.read() };
                let Some(mut port) = channel.scope(record.scope) else { continue; };
                let mut out = Outbox::new(&mut port, &mut self.dispatcher).in_pass(pass).track_dispatches(&mut record.dispatches);
                for event in record.receipts.drain() { record.deck.settle(TrackReceipt::Event(event), pass, &mut out); }
            }
        }
    }
}

fn map_deck_rejection(reason: &Rejection<kithara_render::bridge::DeckRefusal>) -> Rejection<PlayError> {
    match reason {
        Rejection::Late => Rejection::Late,
        Rejection::Stale => Rejection::Stale,
        Rejection::Unanswered => Rejection::Unanswered,
        Rejection::Refused(reason) => Rejection::Refused(PlayError::Deck(reason.clone())),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker_wake_allowance<S>(_worker: &kithara_play::PlayWorker<S>) -> kithara_platform::time::Duration {
    todo!("missing below (kithara-render, PlayWorker::wake_allowance(&self) -> Duration)")
}

impl<S, D: ?Sized + HostedDeck<S>> Drop for HostCore<S, D> {
    fn drop(&mut self) {
        for (_, record) in &mut self.decks.0 {
            if let Some(channel) = &mut self.session.channel {
                if let Some(mut port) = channel.scope(record.scope) {
                    let mut out = Outbox::new(&mut port, &mut self.dispatcher);
                    if let Err(error) = record.deck.close(&mut out) { tracing::warn!(%error, "host deck close failed during shutdown"); }
                }
                if !record.releasing { let _ = channel.close(record.scope); }
            }
        }
        if let Some(channel) = &mut self.session.channel { let _ = channel.publish(); }
        self.session.stream = None;
        if let Some(ctx) = &mut self.session.ctx {
            #[cfg(not(target_arch = "wasm32"))]
            if let Err(error) = ctx.deactivate_blocking(std::time::Duration::from_secs(3)) {
                tracing::error!(?error, "host processor did not return during shutdown");
                return;
            }
            #[cfg(target_arch = "wasm32")]
            {
                ctx.request_deactivate();
                if let Some(mut context) = self.session.ctx.take() {
                    let mut channel = self.session.channel.take();
                    let mut decks: Vec<_> = self
                        .decks
                        .0
                        .drain(..)
                        .map(|(_, record)| (record.scope, record.deck))
                        .collect();
                    let dispatcher_task = self.dispatcher_task.take();
                    let root_view = self.session.root_view.clone();
                    drop(kithara_platform::tokio::task::spawn(async move {
                        let mut retired = false;
                        loop {
                            if let Err(error) = context.update() {
                                tracing::warn!(?error, "browser shutdown graph update failed");
                            }
                            if !retired && let Some(store) = context.proc_store_mut() {
                                if let Some(transport) = store.try_get_mut::<TransportState>() {
                                    transport.inbox.retire_closing();
                                }
                                if let std::collections::hash_map::Entry::Occupied(entry) =
                                    store.entry::<TransportState>().boxed_entry
                                {
                                    drop(entry.remove());
                                }
                                retired = true;
                            }
                            if let Some(channel) = &mut channel {
                                while let Some(receipt) = channel.receipt() {
                                    if let ScopedReceipt::Closed(scope) = receipt
                                        && let Some(index) =
                                            decks.iter().position(|(held, _)| *held == scope)
                                    {
                                        let (_, mut deck) = decks.remove(index);
                                        deck.release();
                                    }
                                }
                            }
                            if retired && decks.is_empty() {
                                break;
                            }
                            kithara_platform::time::sleep(crate::consts::SESSION_PUMP_INTERVAL).await;
                        }
                        root_view.publish_decks(Box::default());
                        drop(dispatcher_task);
                        drop(context);
                        drop(channel);
                    }));
                }
                return;
            }
        }
        self.retire_stopped_scopes();
        self.route_receipts(SessionFrame::new(0), FrameCount::new(0));
    }
}
