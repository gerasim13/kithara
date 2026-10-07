use std::num::NonZeroU32;

use kithara_audio::ConsumerWakeMode;
use kithara_bufpool::HasPool;
use kithara_command::{ChannelConfig, Inbox, Receipt, Sender, channel};
use kithara_render::{
    DispatcherProtocol, LoadRefusal,
    bridge::{DeckEvents, DeckProtocol, mixer_channels},
    mock::MockDeck,
    rt::{DeckMixerConfig, StreamShape},
};
use kithara_signal::SessionFrame;

pub use crate::api::equalizer::EqualizerMock;
use crate::{OpenedTrack, ResourceLoad, player::Outbox, session::SessionOutputView};

/// Sample rate every mock session runs at.
pub const SAMPLE_RATE: NonZeroU32 = match NonZeroU32::new(44_100) {
    Some(sample_rate) => sample_rate,
    None => unreachable!(),
};

/// The output of a session at [`SAMPLE_RATE`] that measured `shape`.
#[must_use]
pub fn output(shape: Option<StreamShape>) -> SessionOutputView {
    let output = SessionOutputView::new(SAMPLE_RATE, ConsumerWakeMode::RealtimeDeferred);
    output.publish(output.get().sample_rate, shape);
    output
}

/// A deck no audio thread runs and a dispatcher no worker drains: what a
/// player sends lands here for the test to answer as they would.
pub struct DeckRig<S> {
    pub ring: Sender<DeckProtocol>,
    pub mixer: MockDeck,
    pub events: DeckEvents,
    pub dispatcher: Sender<DispatcherProtocol<ResourceLoad<S>>>,
    pub opens: Inbox<DispatcherProtocol<ResourceLoad<S>>>,
}

impl<S> DeckRig<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(config: DeckMixerConfig) -> Self {
        let (ends, inputs) = mixer_channels(config);
        let (dispatcher, opens) = channel(ChannelConfig::builder().build());
        Self {
            ring: ends.ring,
            mixer: MockDeck::new(inputs),
            events: ends.events,
            dispatcher,
            opens,
        }
    }

    /// The queues a player sends to, lent for one pass.
    pub fn outbox(&mut self) -> Outbox<'_, S> {
        Outbox::new(&mut self.ring, &mut self.dispatcher)
    }

    /// Answers the oldest open the dispatcher holds with `opened` and returns
    /// its receipt; `None` when no open is waiting.
    pub fn open(
        &mut self,
        opened: Result<OpenedTrack, LoadRefusal>,
    ) -> Option<Receipt<DispatcherProtocol<ResourceLoad<S>>>> {
        self.opens.drain();
        let due = self.opens.next_due((), 1)?;
        match opened {
            Ok(opened) => due.apply(opened),
            Err(refusal) => due.refuse(refusal),
        }
        self.dispatcher.receipts().next()
    }

    /// Plays the deck's block at `at` and returns the receipts of the batches
    /// it applied; a slot stopped there stood at `stopped_at` seconds.
    pub fn block(&mut self, at: SessionFrame, stopped_at: f64) -> Vec<Receipt<DeckProtocol>> {
        self.mixer.block(at, stopped_at);
        self.ring.receipts().collect()
    }
}
