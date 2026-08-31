use std::task::Context;

use kithara_platform::{sync::Arc, time::Instant};

use super::core::{AbrController, AbrPeerId};

impl AbrController {
    /// Earliest interval-gated tick still owned by a live peer.
    #[doc(hidden)]
    #[must_use]
    pub fn next_tick_deadline(&self) -> Option<Instant> {
        self.peers
            .iter()
            .filter(|entry| !entry.cancel.is_cancelled())
            .filter_map(|entry| entry.tick_deadline())
            .min()
    }

    /// Poll queued ABR ticks from the downloader's existing run loop.
    ///
    /// `deadline_elapsed` refers to the deadline returned by
    /// [`Self::next_tick_deadline`] when the current downloader tick
    /// began. The method never spawns work: it ticks every dirty peer on the
    /// caller's task and returns whether any tick ran.
    #[doc(hidden)]
    pub fn poll_ticks(
        self: &Arc<Self>,
        cx: &mut Context<'_>,
        now: Instant,
        deadline_elapsed: bool,
    ) -> bool {
        *self.tick_waker.lock() = Some(cx.waker().clone());
        if deadline_elapsed {
            self.request_due_ticks(now);
        }

        let mut ticked = false;
        while let Some(peer_id) = self.take_tick_request() {
            ticked = true;
            self.run_tick(peer_id, now);
        }
        ticked
    }

    fn request_due_ticks(&self, now: Instant) {
        for entry in &self.peers {
            if !entry.cancel.is_cancelled() && entry.take_due_tick_deadline(now) {
                entry.mark_tick_requested();
            }
        }
    }

    fn take_tick_request(&self) -> Option<AbrPeerId> {
        self.peers.iter().find_map(|entry| {
            (!entry.cancel.is_cancelled() && entry.take_tick_request()).then_some(*entry.key())
        })
    }

    pub(crate) fn tick(&self, peer_id: AbrPeerId) {
        let Some(entry) = self.peer_entry(peer_id) else {
            return;
        };
        if !entry.mark_tick_requested() {
            return;
        }
        if let Some(waker) = self.tick_waker.lock().clone() {
            waker.wake();
        }
    }
}
