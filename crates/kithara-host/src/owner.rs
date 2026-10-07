//! The owner a session drives and the commands its handle posts.

use std::{marker::PhantomData, num::NonZeroU32};

use kithara_bufpool::HasPool;
use kithara_command::{LiveError, Receipt, Rejection, SendError, Sender, Seq, When};
use kithara_config::ConfigOwner;
use kithara_platform::maybe_send::MaybeSend;
use kithara_play::{DeckPass, HostedDeck, Outbox, PlayError, ResourceLoad, TrackReceipt};
use kithara_render::{
    DispatcherProtocol,
    bridge::{DeckMixSettingsChange, DeckPart},
};
use kithara_signal::{FrameCount, SessionFrame};

pub use kithara_render::bridge::DeckEqChange as EqPart;

use crate::{
    HostSettingsChange, HostSettingsExec,
    session::{
        SessionError,
        decks::{Deck, DeckInbox, DeckWake, Decks},
        dispatch::tick_session,
        graph,
        state::{SessionState, SessionStream},
    },
};

/// The existing identity used for decks and their beat grids.
pub type DeckId = kithara_warp::BeatGridId;

/// What the session thread drives, including decorators over its base owner.
pub trait HostOwner<S>:
    HostSettingsExec<(), At = When<SessionFrame>, Output = Result<Option<Seq>, PlayError>>
    + MaybeSend
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
    /// Available batches in the host ring.
    fn host_room(&self) -> usize;
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
    },
    Close(DeckId),
    Release(DeckId),
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
}

/// A deck's public control endpoint, independent of session attachment.
pub trait DeckControl {
    /// The handle applications retain while the owner holds the deck.
    type Control;
    /// The identity already assigned to this deck's beat grid.
    fn id(&self) -> DeckId;
    /// Hands out a control handle without binding or seating the deck.
    fn control(&self) -> Self::Control;
}

/// The base owner of a session, its deck records and its single load dispatcher.
pub struct HostCore<S, D: ?Sized + HostedDeck<S> = dyn HostedDeck<S>> {
    pub(crate) session: SessionState<SessionStream, S>,
    decks: Decks<S, D>,
    dispatcher: Sender<DispatcherProtocol<ResourceLoad<S>>>,
    inbox: kithara_platform::sync::Arc<dyn DeckInbox>,
}

impl<S, D: ?Sized + HostedDeck<S>> HostCore<S, D> {
    pub(crate) fn new(
        session: SessionState<SessionStream, S>,
        inbox: kithara_platform::sync::Arc<dyn DeckInbox>,
    ) -> Self {
        Self {
            session,
            decks: Decks::default(),
            dispatcher: Self::open_dispatcher(),
            inbox,
        }
    }

    fn open_dispatcher() -> Sender<DispatcherProtocol<ResourceLoad<S>>> {
        todo!(
            "Connect the owner's single load channel to kithara_render::dispatch and the resources' PlayWorker; no sender is exposed by PlayWorker today (spec §5.4)"
        )
    }

    fn loaded_deck(&self, _receipt: &Receipt<DispatcherProtocol<ResourceLoad<S>>>) -> DeckId {
        todo!(
            "Route a dispatcher Seq to the deck whose Outbox sent the load; Outbox does not expose load ownership yet (spec §5.3)"
        )
    }

    fn close(&mut self, id: DeckId) -> Result<(), PlayError> {
        let mut result = Ok(());
        self.with_deck(id, &mut |deck, out, _pass| {
            result = deck.close(out);
        })?;
        result
    }

    fn release(&mut self, id: DeckId) -> Result<(), PlayError> {
        let index = self.decks.index(id)?;
        graph::remove_deck(&mut self.session, id)?;
        let (_, mut record) = self.decks.0.remove(index);
        record.deck.release();
        self.publish_root();
        if self.decks.0.is_empty() {
            self.idle()?;
        }
        Ok(())
    }

    fn idle(&mut self) -> Result<(), PlayError> {
        self.session.settings.abandon();
        for (_, record) in &mut self.decks.0 {
            record.mix.abandon();
        }
        graph::idle(&mut self.session).map_err(Into::into)
    }

    fn restart(&mut self) -> Result<(), PlayError> {
        self.session.settings.abandon();
        for (_, record) in &mut self.decks.0 {
            record.mix.abandon();
        }
        todo!(
            "Rebuild each mixer from its Live::projected configuration after abandoning the destroyed rings, retaining tracks and publishing the route boundary (spec §5.7)"
        )
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
                let index = self.decks.index(deck)?;
                let record = &mut self.decks.0[index].1;
                record
                    .mix
                    .send(&mut record.ring, at, change, DeckPart::Mix)
                    .map(Some)
                    .map_err(|error| match error {
                        LiveError::Invalid(error) => PlayError::Internal(error.to_string()),
                        LiveError::Send(SendError::Full(_)) => PlayError::Full,
                        LiveError::Send(SendError::Closed(_)) => PlayError::Closed,
                        LiveError::Send(SendError::Target(_)) => {
                            PlayError::Internal("a deck mix batch names a slot".to_owned())
                        }
                    })
            }
            HostCommand::Eq { deck, part } => {
                self.decks.index(deck)?;
                todo!(
                    "Send EqPart as one DeckPart::Eq on Next when the render protocol provides that part (spec §4.2)"
                )
            }
            HostCommand::Close(id) => self.close(id).map(|()| None),
            HostCommand::Release(id) => self.release(id).map(|()| None),
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
        let (mut record, inputs) = Deck::new(deck)?;
        graph::install_deck(&mut self.session, id, inputs)?;
        record.deck.hold(DeckWake::waker(&self.inbox, id));
        self.decks.0.push((id, record));
        self.with_deck(id, &mut |deck, out, pass| deck.drain(pass, out))?;
        self.publish_root();
        Ok(())
    }

    fn each_deck(
        &mut self,
        visit: &mut dyn FnMut(DeckId, &mut D, &mut Outbox<'_, S>, DeckPass<'_>),
    ) {
        let Some((now, delivery)) = self.clock() else {
            return;
        };
        for (id, record) in &mut self.decks.0 {
            let pass = DeckPass {
                now,
                delivery,
                deck: record.snapshot.read(),
            };
            let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
            visit(*id, &mut record.deck, &mut out, pass);
        }
    }

    fn with_deck(
        &mut self,
        id: DeckId,
        visit: &mut dyn FnMut(&mut D, &mut Outbox<'_, S>, DeckPass<'_>),
    ) -> Result<(), PlayError> {
        let index = self.decks.index(id)?;
        let (now, delivery) = self.clock().ok_or(PlayError::Untimed)?;
        let record = &mut self.decks.0[index].1;
        let pass = DeckPass {
            now,
            delivery,
            deck: record.snapshot.read(),
        };
        let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
        visit(&mut record.deck, &mut out, pass);
        Ok(())
    }

    fn clock(&self) -> Option<(SessionFrame, FrameCount)> {
        let ctx = self.session.ctx.as_ref()?;
        let block = ctx.stream_info()?.max_block_frames.get();
        Some((
            SessionFrame::new(ctx.audio_clock().samples.0),
            FrameCount::new(u64::from(block)),
        ))
    }

    fn host_room(&self) -> usize {
        self.session
            .transport_queue
            .as_ref()
            .map_or(0, Sender::available)
    }

    fn pass(&mut self) -> Vec<HostSettled> {
        if let Err(error) = tick_session(&mut self.session) {
            tracing::warn!(%error, "host graph pass failed");
        }
        if let Some((now, delivery)) = self.clock() {
            let mut loaded = Vec::new();
            while let Some(receipt) = self.dispatcher.receipts().next() {
                loaded.push((self.loaded_deck(&receipt), receipt));
            }
            for (id, record) in &mut self.decks.0 {
                let pass = DeckPass {
                    now,
                    delivery,
                    deck: record.snapshot.read(),
                };
                while let Some(receipt) = record.ring.receipts().next() {
                    if receipt.batch().basis.is_empty() {
                        record.mix.settle(&receipt);
                    } else {
                        let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
                        record
                            .deck
                            .settle(TrackReceipt::Deck(&receipt), pass, &mut out);
                    }
                }
                let mut index = 0;
                while index < loaded.len() {
                    if loaded[index].0 == *id {
                        let (_, receipt) = loaded.remove(index);
                        let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
                        record
                            .deck
                            .settle(TrackReceipt::Loaded(receipt), pass, &mut out);
                    } else {
                        index += 1;
                    }
                }
                for event in record.receipts.drain() {
                    let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
                    record
                        .deck
                        .settle(TrackReceipt::Event(event), pass, &mut out);
                }
                let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
                record.deck.drain(pass, &mut out);
                let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
                record.deck.tick(pass, &mut out);
            }
        }
        self.publish_root();
        std::mem::take(&mut self.session.settled)
    }
}

impl<S, D: ?Sized + HostedDeck<S>> Drop for HostCore<S, D> {
    fn drop(&mut self) {
        for (_, record) in &mut self.decks.0 {
            let mut out = Outbox::new(&mut record.ring, &mut self.dispatcher);
            if let Err(error) = record.deck.close(&mut out) {
                tracing::warn!(%error, "host deck close failed during shutdown");
            }
            record.deck.release();
        }
    }
}
