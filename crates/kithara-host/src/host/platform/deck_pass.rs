use std::task::{Wake, Waker};

use kithara_platform::{
    sync::{Arc, mpsc},
    thread::{JoinHandle, spawn_named},
    time::Instant,
};
use kithara_play::PlayError;
use kithara_warp::BeatGridId;
use tracing::warn;

use super::decks::{Deck, Decks};
use crate::{consts, session::native::receive_message};

/// What makes a native Host's deck thread tick its decks.
#[derive(Clone, Copy)]
pub(super) enum Pace {
    /// A realtime session: once per session pump interval, with no caller
    /// driving it.
    Clock,
    /// An offline session: once before each block the Host renders.
    #[cfg(feature = "offline")]
    Blocks,
}

enum DeckMsg {
    Hold(BeatGridId, Deck),
    Release(BeatGridId, mpsc::Sender<Result<Deck, PlayError>>),
    /// The deck has commands posted: run them now, between ticks.
    Drain(BeatGridId),
    #[cfg(feature = "offline")]
    Tick(mpsc::Sender<()>),
    Close,
}

/// The thread that holds a native Host's decks. It runs the commands posted
/// to a deck as soon as the deck wakes it, and ticks every deck at its pace.
pub(super) struct DeckThread {
    tx: mpsc::Sender<DeckMsg>,
    thread: Option<JoinHandle<Decks>>,
}

impl DeckThread {
    pub(super) fn spawn(pace: Pace) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            thread: Some(spawn_named("kithara-host-decks", move || run(&rx, pace))),
        }
    }

    /// Stops the thread and takes back the decks it held, each released
    /// first. A thread that panicked dropped them as it unwound.
    pub(super) fn close(&mut self) -> Decks {
        let Some(thread) = self.thread.take() else {
            return Decks::default();
        };
        if self.tx.send(DeckMsg::Close).is_err() {
            warn!("host deck thread stopped before it was closed");
        }
        thread.join().unwrap_or_else(|_| {
            warn!("host deck thread panicked");
            Decks::default()
        })
    }

    /// Hands `deck` to the thread, which drains it once it holds it: a
    /// command posted before then woke no one. The deck is held before it
    /// leaves, so a command posted as soon as this returns reaches it.
    pub(super) fn hold(&self, id: BeatGridId, mut deck: Deck) -> Result<(), PlayError> {
        deck.hold(Waker::from(Arc::new(DeckWake {
            id,
            tx: self.tx.clone(),
        })));
        self.send(DeckMsg::Hold(id, deck))
    }

    /// Hands the deck `id` back to the caller, released.
    pub(super) fn release(&self, id: BeatGridId) -> Result<Deck, PlayError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.send(DeckMsg::Release(id, reply_tx))?;
        reply_rx.recv().map_err(|_| Self::gone())?
    }

    /// Ticks the decks of an offline session ahead of one rendered block and
    /// returns once they are ticked.
    #[cfg(feature = "offline")]
    pub(super) fn tick_block(&self) {
        let (done_tx, done_rx) = mpsc::channel();
        if self.send(DeckMsg::Tick(done_tx)).is_err() || done_rx.recv().is_err() {
            warn!("host deck thread stopped before ticking a block");
        }
    }

    fn send(&self, message: DeckMsg) -> Result<(), PlayError> {
        self.tx.send(message).map_err(|_| Self::gone())
    }

    const fn gone() -> PlayError {
        PlayError::SessionGone {
            reason: "host deck thread stopped",
        }
    }
}

impl Drop for DeckThread {
    fn drop(&mut self) {
        drop(self.close());
    }
}

/// Wakes the deck thread to drain one deck.
struct DeckWake {
    id: BeatGridId,
    tx: mpsc::Sender<DeckMsg>,
}

impl Wake for DeckWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    /// A thread that already stopped has released the deck, so a wake it
    /// misses had nothing left to run.
    fn wake_by_ref(self: &Arc<Self>) {
        drop(self.tx.send(DeckMsg::Drain(self.id)));
    }
}

fn run(rx: &mpsc::Receiver<DeckMsg>, pace: Pace) -> Decks {
    let clocked = matches!(pace, Pace::Clock);
    let mut decks = Decks::default();
    let mut deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
    while let Ok(message) = receive_message(rx, clocked && !decks.is_empty(), deadline) {
        match message {
            Some(DeckMsg::Hold(id, deck)) => decks.hold(id, deck),
            Some(DeckMsg::Release(id, reply_tx)) => hand_back(&mut decks, id, &reply_tx),
            Some(DeckMsg::Drain(id)) => decks.drain(id),
            #[cfg(feature = "offline")]
            Some(DeckMsg::Tick(done_tx)) => {
                decks.tick();
                if done_tx.send(()).is_err() {
                    warn!("offline render stopped waiting for its block tick");
                }
            }
            Some(DeckMsg::Close) => break,
            None => {}
        }
        if clocked && Instant::now() >= deadline {
            decks.tick();
            deadline = Instant::now() + consts::SESSION_PUMP_INTERVAL;
        }
    }
    decks.release_all();
    decks
}

fn hand_back(decks: &mut Decks, id: BeatGridId, reply_tx: &mpsc::Sender<Result<Deck, PlayError>>) {
    if reply_tx.send(decks.release(id)).is_err() {
        warn!(?id, "host deck release receiver dropped");
    }
}

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use kithara_platform::{
        thread::sleep,
        time::{Duration, Instant},
    };
    use kithara_play::player::Player;
    use kithara_test_utils::kithara;

    use super::*;

    /// What the deck thread did to a [`Probe`].
    enum Seen {
        Held(Waker),
        Drained,
        Released,
        Ticked,
        Dropped,
    }

    /// A deck that reports every call its executor makes.
    struct Probe(mpsc::Sender<Seen>);

    impl Probe {
        fn report(&self, seen: Seen) {
            self.0.send(seen).expect("the test watches its probe");
        }
    }

    /// A test may stop watching before its holder drops the deck.
    impl Drop for Probe {
        fn drop(&mut self) {
            drop(self.0.send(Seen::Dropped));
        }
    }

    impl Player for Probe {
        fn close(&mut self) -> Result<(), PlayError> {
            Ok(())
        }

        fn drain(&mut self) {
            self.report(Seen::Drained);
        }

        fn hold(&mut self, waker: Waker) {
            self.report(Seen::Held(waker));
        }

        fn release(&mut self) {
            self.report(Seen::Released);
        }

        fn tick(&mut self) -> Result<(), PlayError> {
            self.report(Seen::Ticked);
            Ok(())
        }
    }

    fn probe() -> (BeatGridId, Deck, mpsc::Receiver<Seen>) {
        let (seen_tx, seen_rx) = mpsc::channel();
        let id = BeatGridId::allocate().expect("fixture grid id");
        (id, Box::new(Probe(seen_tx)), seen_rx)
    }

    fn next(seen: &mpsc::Receiver<Seen>) -> Seen {
        seen.recv_timeout(Instant::now() + Duration::from_secs(5))
            .expect("the deck thread reaches the probe")
    }

    /// Everything the probe has seen so far, and nothing it sees later.
    fn so_far(seen: &mpsc::Receiver<Seen>) -> Vec<Seen> {
        seen.try_iter().collect()
    }

    fn ticks(seen: &[Seen]) -> usize {
        seen.iter()
            .filter(|seen| matches!(seen, Seen::Ticked))
            .count()
    }

    #[kithara::test]
    fn the_deck_thread_ticks_a_held_deck_until_it_is_released() {
        let (id, held, seen) = probe();
        let mut thread = DeckThread::spawn(Pace::Clock);

        thread.hold(id, held).expect("the thread takes the deck");
        let mut ticked = 0;
        while ticked < 2 {
            if matches!(next(&seen), Seen::Ticked) {
                ticked += 1;
            }
        }
        let released = thread.release(id).expect("the thread hands the deck back");
        drop(so_far(&seen));
        sleep(consts::SESSION_PUMP_INTERVAL * 3);

        assert_eq!(
            ticks(&so_far(&seen)),
            0,
            "a released deck is no longer ticked"
        );
        drop(released);
        drop(thread.close());
    }

    /// Closing a realtime thread stops its clock but keeps its decks alive:
    /// the Host drops them only once the session that renders them has shut
    /// down.
    #[kithara::test]
    fn closing_the_deck_thread_stops_its_clock_and_keeps_its_decks() {
        let (id, held, seen) = probe();
        let mut thread = DeckThread::spawn(Pace::Clock);

        thread.hold(id, held).expect("the thread takes the deck");
        let decks = thread.close();
        drop(so_far(&seen));
        sleep(consts::SESSION_PUMP_INTERVAL * 3);

        let after_close = so_far(&seen);
        assert_eq!(ticks(&after_close), 0, "a closed thread ticks no deck");
        assert!(
            !after_close.iter().any(|seen| matches!(seen, Seen::Dropped)),
            "the deck outlives the thread that held it"
        );
        drop(decks);
        assert!(
            so_far(&seen)
                .iter()
                .any(|seen| matches!(seen, Seen::Dropped)),
            "the deck drops with its holder"
        );
    }

    /// Commands posted before the deck was held woke no one, so the thread
    /// runs them as soon as it holds the deck.
    #[kithara::test]
    fn the_deck_thread_drains_a_deck_as_it_takes_it() {
        let (id, held, seen) = probe();
        let thread = DeckThread::spawn(Pace::Clock);

        thread.hold(id, held).expect("the thread takes the deck");

        assert!(matches!(next(&seen), Seen::Held(_)));
        assert!(
            matches!(next(&seen), Seen::Drained),
            "the thread drains a deck it takes before ticking it"
        );
        drop(thread);
    }

    #[kithara::test]
    fn a_deck_the_thread_lets_go_is_released_before_it_is_handed_back() {
        let (id, held, seen) = probe();
        let thread = DeckThread::spawn(Pace::Clock);
        thread.hold(id, held).expect("the thread takes the deck");
        assert!(matches!(next(&seen), Seen::Held(_)));

        let released = thread.release(id).expect("the thread hands the deck back");

        assert!(
            seen.try_iter().any(|seen| matches!(seen, Seen::Released)),
            "a deck comes back released, so nothing waits on what it had queued"
        );
        drop(released);
    }

    #[kithara::test]
    fn a_closed_thread_hands_back_every_deck_released() {
        let (id, held, seen) = probe();
        let mut thread = DeckThread::spawn(Pace::Clock);
        thread.hold(id, held).expect("the thread takes the deck");

        let decks = thread.close();

        assert!(
            seen.try_iter().any(|seen| matches!(seen, Seen::Released)),
            "a deck the Host retires takes no more commands"
        );
        drop(decks);
    }

    #[cfg(feature = "offline")]
    #[kithara::test]
    fn an_offline_thread_ticks_its_decks_only_ahead_of_a_block() {
        let (id, held, seen) = probe();
        let mut thread = DeckThread::spawn(Pace::Blocks);

        thread.hold(id, held).expect("the thread takes the deck");
        sleep(consts::SESSION_PUMP_INTERVAL * 3);
        assert_eq!(ticks(&so_far(&seen)), 0, "no clock ticks an offline deck");

        thread.tick_block();
        assert_eq!(
            ticks(&so_far(&seen)),
            1,
            "the block's tick is done before tick_block returns"
        );
        drop(thread.close());
    }

    #[cfg(feature = "offline")]
    #[kithara::test]
    fn an_offline_deck_drains_on_its_wake_with_no_block_rendered() {
        let (id, held, seen) = probe();
        let mut thread = DeckThread::spawn(Pace::Blocks);
        thread.hold(id, held).expect("the thread takes the deck");
        let Seen::Held(waker) = next(&seen) else {
            panic!("the thread holds the deck before anything else");
        };

        assert!(
            matches!(next(&seen), Seen::Drained),
            "drained as it is held"
        );

        waker.wake_by_ref();

        assert!(
            matches!(next(&seen), Seen::Drained),
            "a woken deck runs its commands before any tick"
        );
        drop(thread.close());
    }
}
