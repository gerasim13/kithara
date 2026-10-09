use std::num::NonZeroU32;

use kithara_command::{
    ChannelConfig, Inbox, OpenError, Receipt, ScopeId, ScopedConfig, ScopedInbox, ScopedReceipt,
    ScopedSender, Sender, channel, scoped_channel,
};
use kithara_render::{
    Dispatched, DispatcherProtocol, LoadRefusal, Loaded,
    bridge::{DeckEvents, DeckProtocol, scope_channels},
    mock::MockDeck,
    rt::{DeckMixerConfig, StreamShape},
};
use kithara_signal::SessionFrame;

pub use crate::api::equalizer::EqualizerMock;
pub use crate::resource::source_mock::{resource_tracks, track_load};
use crate::{OpenedTrack, PlayError, ResourceLoad, player::Outbox, session::SessionOutputView};

/// Sample rate every mock session runs at.
pub const SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// The output of a session at [`SAMPLE_RATE`] that measured `shape`.
#[must_use]
pub fn output(shape: Option<StreamShape>) -> SessionOutputView {
    let output = SessionOutputView::new(SAMPLE_RATE);
    output.publish(output.get().sample_rate, shape);
    output
}

/// A deck no audio thread runs and a dispatcher no worker drains: what a
/// player sends lands here for the test to answer as they would.
pub struct DeckRig<S> {
    pub ring: ScopedSender<DeckProtocol, DeckProtocol>,
    pub scope: ScopeId,
    pub inbox: ScopedInbox<DeckProtocol, DeckProtocol>,
    pub mixer: MockDeck,
    pub events: DeckEvents,
    pub dispatcher: Sender<DispatcherProtocol<ResourceLoad<S>>>,
    pub opens: Inbox<DispatcherProtocol<ResourceLoad<S>>>,
}

impl<S> DeckRig<S> {
    #[must_use]
    pub fn new(config: DeckMixerConfig) -> Result<Self, OpenError> {
        let targets = config.slots().get();
        let (mut ring, inbox) = scoped_channel(
            ScopedConfig::builder()
                .scope(ChannelConfig::builder().targets(targets).build())
                .build(),
        );
        let scope = ring.open(targets)?;
        let (ends, inputs) = scope_channels(scope, config);
        let (dispatcher, opens) = channel(ChannelConfig::builder().build());
        Ok(Self {
            ring,
            scope,
            inbox,
            mixer: MockDeck::new(inputs),
            events: ends.events,
            dispatcher,
            opens,
        })
    }

    /// The queues a player sends to, lent for one pass.
    pub fn with_outbox<R>(
        &mut self,
        run: impl FnOnce(&mut Outbox<'_, S>) -> R,
    ) -> Result<R, PlayError> {
        let mut scope = self.ring.scope(self.scope).ok_or(PlayError::Closed)?;
        Ok(run(&mut Outbox::new(&mut scope, &mut self.dispatcher)))
    }

    /// Answers the oldest open the dispatcher holds with `opened` and returns
    /// its receipt; `None` when no open is waiting.
    pub fn open(
        &mut self,
        opened: Result<Loaded<OpenedTrack>, LoadRefusal>,
    ) -> Option<Receipt<DispatcherProtocol<ResourceLoad<S>>>> {
        self.opens.drain();
        let due = self.opens.next_due((), 1)?;
        match opened {
            Ok(opened) => due.apply(Dispatched::Loaded(opened)),
            Err(refusal) => due.refuse(refusal),
        }
        self.dispatcher.receipts().next()
    }

    /// Plays the deck's block at `at` and returns the receipts of the batches
    /// it applied; a slot stopped there stood at `stopped_at` seconds.
    pub fn block(&mut self, at: SessionFrame, stopped_at: f64) -> Vec<Receipt<DeckProtocol>> {
        if let Err(error) = self.ring.publish() {
            tracing::warn!(%error, "mock deck channel publication failed");
        }
        self.inbox.drain();
        self.run_mixer(at, stopped_at);
        let mut receipts = Vec::new();
        while let Some(receipt) = self.ring.receipt() {
            if let ScopedReceipt::Scope(scope, receipt) = receipt
                && scope == self.scope
            {
                receipts.push(receipt);
            }
        }
        receipts
    }

    fn run_mixer(&mut self, _at: SessionFrame, _stopped_at: f64) {
        todo!(
            "kithara-render::mock::MockDeck::block with a borrowed scoped level and SlotMark Stop receipts (contract §§5-6)"
        )
    }
}
