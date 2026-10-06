use kithara_platform::{
    sync::mpsc,
    thread::{JoinHandle, spawn_named},
    time::Instant,
};
use kithara_play::PlayError;
use kithara_warp::BeatGridId;
use tracing::warn;

use super::decks::{Deck, Decks};
use crate::{consts, session::native::receive_message};

/// Where a native Host keeps its decks, and what ticks them.
pub(super) enum DeckPass {
    /// A realtime session: the deck thread ticks them on the session clock.
    Clock(DeckThread),
    /// An offline session: they tick before each block the Host renders.
    #[cfg(feature = "offline")]
    Blocks(Decks),
}

impl DeckPass {
    /// Drops every deck before the session they talk to shuts down.
    pub(super) fn close(&mut self) {
        match self {
            Self::Clock(thread) => thread.close(),
            #[cfg(feature = "offline")]
            Self::Blocks(decks) => drop(std::mem::take(decks)),
        }
    }

    pub(super) fn hold(&mut self, id: BeatGridId, deck: Deck) -> Result<(), PlayError> {
        match self {
            Self::Clock(thread) => thread.send(DeckMsg::Hold(id, deck)),
            #[cfg(feature = "offline")]
            Self::Blocks(decks) => {
                decks.hold(id, deck);
                Ok(())
            }
        }
    }

    /// Hands the deck `id` back to the caller.
    pub(super) fn release(&mut self, id: BeatGridId) -> Result<Deck, PlayError> {
        match self {
            Self::Clock(thread) => thread.release(id),
            #[cfg(feature = "offline")]
            Self::Blocks(decks) => decks.release(id),
        }
    }

    /// Ticks the decks of an offline session ahead of one rendered block.
    #[cfg(feature = "offline")]
    pub(super) fn tick_block(&self) {
        if let Self::Blocks(decks) = self {
            decks.tick();
        }
    }
}

enum DeckMsg {
    Hold(BeatGridId, Deck),
    Release(BeatGridId, mpsc::Sender<Result<Deck, PlayError>>),
}

/// The thread that holds a realtime Host's decks and ticks each of them once
/// per session pump interval, with no caller driving it.
pub(super) struct DeckThread {
    tx: Option<mpsc::Sender<DeckMsg>>,
    thread: Option<JoinHandle<()>>,
}

impl DeckThread {
    pub(super) fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx: Some(tx),
            thread: Some(spawn_named("kithara-host-decks", move || run(&rx))),
        }
    }

    /// Stops the thread once it drops its decks.
    fn close(&mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            warn!("host deck thread panicked");
        }
    }

    fn release(&self, id: BeatGridId) -> Result<Deck, PlayError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(DeckMsg::Release(id, reply_tx))?;
        reply_rx.recv().map_err(|_| Self::gone())?
    }

    fn send(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.tx
            .as_ref()
            .ok_or_else(Self::gone)?
            .send(message)
            .map_err(|_| Self::gone())
    }

    const fn gone() -> PlayError {
        PlayError::SessionGone {
            reason: "host deck thread stopped",
        }
    }
}

fn run(rx: &mpsc::Receiver<DeckMsg>) {
    let mut decks = Decks::default();
    let mut deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    while let Ok(message) = receive_message(rx, !decks.is_empty(), deadline) {
        match message {
            Some(DeckMsg::Hold(id, deck)) => decks.hold(id, deck),
            Some(DeckMsg::Release(id, reply_tx)) => hand_back(&mut decks, id, &reply_tx),
            None => {}
        }
        if Instant::now() >= deadline {
            decks.tick();
            deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
        }
    }
}

fn hand_back(decks: &mut Decks, id: BeatGridId, reply_tx: &mpsc::Sender<Result<Deck, PlayError>>) {
    if reply_tx.send(decks.release(id)).is_err() {
        warn!(?id, "host deck release receiver dropped");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use kithara_audio::ConsumerWakeMode;
    use kithara_platform::{sync::Arc, thread::sleep};
    use kithara_play::{
        Cmd, PlayWorker, PlayWorkerConfig, PlayerConfig, PlayerImpl, Reply, SessionBinding,
        SessionDispatcher, player::PlayerControlSource,
    };
    use kithara_test_utils::{bufpool::pools, kithara};

    use super::*;

    /// A session that answers every command and counts the ticks it gets.
    struct TickCounter(Arc<AtomicUsize>);

    impl<S> SessionDispatcher<S> for TickCounter {
        fn consumer_wake_mode(&self) -> ConsumerWakeMode {
            ConsumerWakeMode::RealtimeDeferred
        }

        fn exec(&self, cmd: Cmd<S>) -> Result<Reply, PlayError> {
            if matches!(cmd, Cmd::Tick) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            Ok(Reply::Ok)
        }
    }

    fn deck(ticks: &Arc<AtomicUsize>) -> (BeatGridId, Deck) {
        let mut player = PlayerImpl::new(
            PlayerConfig::builder()
                .sample_rate(consts::DEFAULT_SAMPLE_RATE)
                .worker(PlayWorker::new(PlayWorkerConfig::builder(pools()).build()))
                .build(),
        );
        let id = player
            .attach_session(SessionBinding::new(
                Arc::new(TickCounter(Arc::clone(ticks))),
                consts::DEFAULT_SAMPLE_RATE,
            ))
            .expect("the deck binds its session");
        (id, Box::new(player))
    }

    #[kithara::test]
    fn the_deck_thread_ticks_a_held_deck_until_it_is_released() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let (id, held) = deck(&ticks);
        let mut pass = DeckPass::Clock(DeckThread::spawn());

        pass.hold(id, held).expect("the thread takes the deck");
        while ticks.load(Ordering::Relaxed) < 2 {
            sleep(consts::SESSION_PUMP_INTERVAL);
        }
        let released = pass.release(id).expect("the thread hands the deck back");
        let at_release = ticks.load(Ordering::Relaxed);
        sleep(consts::SESSION_PUMP_INTERVAL * 3);

        assert_eq!(
            ticks.load(Ordering::Relaxed),
            at_release,
            "a released deck is no longer ticked"
        );
        drop(released);
        pass.close();
    }

    #[cfg(feature = "offline")]
    #[kithara::test]
    fn an_offline_pass_ticks_its_decks_only_ahead_of_a_block() {
        let ticks = Arc::new(AtomicUsize::new(0));
        let (id, held) = deck(&ticks);
        let mut pass = DeckPass::Blocks(Decks::default());

        pass.hold(id, held).expect("the pass takes the deck");
        sleep(consts::SESSION_PUMP_INTERVAL * 3);
        assert_eq!(
            ticks.load(Ordering::Relaxed),
            0,
            "no clock ticks an offline deck"
        );

        pass.tick_block();
        assert_eq!(ticks.load(Ordering::Relaxed), 1);
        pass.close();
    }
}
