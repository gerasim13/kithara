use std::task::Waker;

use kithara_platform::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use kithara_play::{PlayError, player::Player};
use kithara_warp::BeatGridId;

use crate::session::decks::Deck;

/// What a holder did to a [`Probe`].
pub(crate) enum Seen {
    Held(Waker),
    /// Drained, on the thread named here.
    Drained(Option<String>),
    Released,
    /// Ticked, on the thread named here.
    Ticked(Option<String>),
    Dropped,
}

/// A deck that reports every call its holder makes, and its drop. A test may
/// stop watching before the holder lets the deck go.
struct Probe(mpsc::Sender<Seen>);

impl Probe {
    fn report(&self, seen: Seen) {
        drop(self.0.send(seen));
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.report(Seen::Dropped);
    }
}

impl Player for Probe {
    fn close(&mut self) -> Result<(), PlayError> {
        Ok(())
    }

    fn drain(&mut self) {
        self.report(Seen::Drained(here()));
    }

    fn hold(&mut self, waker: Waker) {
        self.report(Seen::Held(waker));
    }

    fn release(&mut self) {
        self.report(Seen::Released);
    }

    fn tick(&mut self) -> Result<(), PlayError> {
        self.report(Seen::Ticked(here()));
        Ok(())
    }
}

fn here() -> Option<String> {
    thread::current().name().map(str::to_owned)
}

/// A probe deck under a fresh id, and what it sees.
pub(crate) fn probe() -> (BeatGridId, Deck, mpsc::Receiver<Seen>) {
    let (seen_tx, seen_rx) = mpsc::channel();
    let id = BeatGridId::allocate().expect("fixture grid id");
    (id, Box::new(Probe(seen_tx)), seen_rx)
}

/// The next call the probe sees.
pub(crate) fn next(seen: &mpsc::Receiver<Seen>) -> Seen {
    seen.recv_timeout(Instant::now() + Duration::from_secs(5))
        .expect("the holder reaches the probe")
}

/// Everything the probe has seen so far, and nothing it sees later.
pub(crate) fn so_far(seen: &mpsc::Receiver<Seen>) -> Vec<Seen> {
    seen.try_iter().collect()
}

pub(crate) fn ticks(seen: &[Seen]) -> usize {
    seen.iter()
        .filter(|seen| matches!(seen, Seen::Ticked(_)))
        .count()
}
