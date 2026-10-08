use std::{
    future::{Future, pending, poll_fn},
    sync::atomic::Ordering,
    task::Poll,
};

use kithara_platform::{
    time::{Instant, sleep},
    tokio,
    tokio::sync::mpsc,
};
use kithara_test_utils::kithara;

use super::{
    peers::{Peers, PollStats},
    slots::Slots,
};
use crate::{
    batch::BatchGroup,
    downloader::{DownloaderInner, RegisteredPeerEntry},
};

/// Observable forward motion of the fetch pipeline across one
/// [`Registry::tick`]. Consumed by the hang watchdog in
/// [`Downloader::run`](crate::downloader::Downloader::run) to distinguish
/// legitimate quiet periods from genuine deadlocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchProgress {
    /// No in-flight fetches and no queued work. Legitimate quiet; the
    /// watchdog must stay silent.
    Idle,
    /// At least one of: a command was drained from a peer's channel, a
    /// peer yielded a batch, an ABR tick ran, an in-flight fetch
    /// completed (inflight decremented), or a new fetch was dispatched.
    Advanced,
    /// Pending work exists (inflight > 0 or slots non-empty) but this
    /// tick observed no forward motion. Consecutive stalls across the
    /// watchdog window trigger a deadlock panic.
    Stalled,
}

/// Peer registry: owns peers, routes commands to priority slots,
/// and drives batch execution.
#[derive(Default)]
pub(crate) struct Registry {
    peers: Peers,
    slots: Slots,
}

impl Registry {
    pub(crate) fn reschedule(&mut self) {
        self.slots.reschedule(|peer| self.peers.priority(peer));
    }

    async fn process_batch(&mut self, batch: BatchGroup, inner: &DownloaderInner) -> usize {
        let result = batch.process(inner);
        let capacity_blocked = !result.pending.is_empty()
            && inner.inflight.load(Ordering::Relaxed) >= inner.config.max_concurrent;
        self.slots
            .requeue_pending(result.pending, |peer| self.peers.priority(peer));
        if capacity_blocked {
            inner.capacity_notify.notified().await;
        }
        result.dispatched
    }

    /// Single tick: poll peers, process urgent, then demand with throttle.
    ///
    /// New peer registrations, queued ABR ticks, and the nearest ABR deadline
    /// are handled inside `poll_fn` rather than competing `select!` arms. This
    /// guarantees that `process()` runs to completion: no readiness source can
    /// drop `tick()` mid-batch and lose unspawned `FetchCmd`s.
    ///
    /// The deadline future completes once while the closure runs on every
    /// wakeup of this tick, so the elapsed flag latches rather than re-polling
    /// it. A deadline the controller then declines to tick on — its peer was
    /// cancelled while the loop slept — leaves the loop parked with that future
    /// finished, and resuming it panics the downloader out from under every
    /// peer it serves.
    ///
    /// Returns a [`FetchProgress`] describing whether fetch work moved
    /// forward this tick. Idle returns are possible when `poll_fn` is
    /// woken by `fetch_waker` (an in-flight fetch completed elsewhere)
    /// but no new peer/command activity occurred; the downloader
    /// watchdog uses this signal to avoid false panics during quiet
    /// periods.
    ///
    /// The tick reads the clock the ABR controller decides on: its deadlines,
    /// the `now` it ticks with, and the enqueue stamps
    /// [`BatchGroup::process`] measures queue waits from.
    #[kithara::flash(true)]
    pub(crate) async fn tick(
        &mut self,
        inner: &DownloaderInner,
        register_rx: &mut mpsc::UnboundedReceiver<RegisteredPeerEntry>,
    ) -> FetchProgress {
        let inflight_enter = inner.inflight.load(Ordering::Relaxed);
        let mut aggregate = PollStats::default();
        let abr_deadline = inner.abr.next_tick_deadline();
        let abr_deadline_wait = async move {
            let Some(deadline) = abr_deadline else {
                return pending::<()>().await;
            };
            sleep(deadline.saturating_duration_since(Instant::now())).await;
        };
        tokio::pin!(abr_deadline_wait);
        let mut deadline_elapsed = false;

        poll_fn(|cx| {
            if !deadline_elapsed {
                deadline_elapsed = abr_deadline_wait.as_mut().poll(cx).is_ready();
            }
            aggregate.abr_ticked |= inner.abr.poll_ticks(cx, Instant::now(), deadline_elapsed);
            while let Poll::Ready(Some(entry)) = register_rx.poll_recv(cx) {
                self.peers.add(entry);
                self.slots.notify_urgent();
            }
            let stats = self.peers.poll(cx, inner, &mut self.slots);
            aggregate.drained_cmds += stats.drained_cmds;
            aggregate.peer_batches += stats.peer_batches;
            if deadline_elapsed || aggregate.abr_ticked || self.slots.has_work() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;

        if aggregate.abr_ticked && !self.slots.has_work() {
            return FetchProgress::Advanced;
        }

        let mut dispatched: usize = 0;

        let urgent_batch = self.slots.take(0..2);
        if !urgent_batch.is_empty() {
            dispatched += self.process_batch(urgent_batch, inner).await;
            return classify_progress(
                inflight_enter,
                inner.inflight.load(Ordering::Relaxed),
                aggregate,
                dispatched,
            );
        }

        if !inner.config.demand_throttle.is_zero() {
            let preempted_by_urgent = tokio::select! {
                () = sleep(inner.config.demand_throttle) => false,
                () = self.slots.urgent_notified() => true,
            };
            if preempted_by_urgent {
                return classify_progress(
                    inflight_enter,
                    inner.inflight.load(Ordering::Relaxed),
                    aggregate,
                    dispatched,
                );
            }
        }

        let demand_batch = self.slots.take(2..4);
        if !demand_batch.is_empty() {
            dispatched += self.process_batch(demand_batch, inner).await;
        }

        classify_progress(
            inflight_enter,
            inner.inflight.load(Ordering::Relaxed),
            aggregate,
            dispatched,
        )
    }
}

/// A queued command still owns its peer's claim — the HLS segment slot and the
/// single non-`Clone` `AssetWriter` both ride in its `on_complete`. Dropping the
/// registry (downloader cancel leaves `Downloader::run`) would drop those
/// closures uncalled and strand the claim, so teardown delivers the
/// cancellation every queued command was still waiting for.
///
/// Peer channels need no such drain: they carry only imperative
/// `ResponseTarget::Channel` commands, whose caller already learns of the
/// cancellation from the dropped oneshot.
impl Drop for Registry {
    fn drop(&mut self) {
        self.slots.cancel_all();
    }
}

/// Classify a completed [`Registry::tick`] as one of the [`FetchProgress`]
/// variants. `inflight_enter` is the snapshot taken at the start of the
/// tick; `inflight_exit` is the value after processing. An exit below
/// enter means at least one fetch completed while this tick was running
/// — which counts as forward motion regardless of whether any new cmds
/// were drained or batches dispatched.
///
/// Pure function of its inputs so the invariant can be exhaustively
/// tested without constructing a live Downloader.
const fn classify_progress(
    inflight_enter: usize,
    inflight_exit: usize,
    poll_stats: PollStats,
    dispatched: usize,
) -> FetchProgress {
    let advanced = poll_stats.drained_cmds > 0
        || poll_stats.abr_ticked
        || poll_stats.peer_batches > 0
        || dispatched > 0
        || inflight_exit < inflight_enter;
    if advanced {
        return FetchProgress::Advanced;
    }
    if inflight_exit > 0 {
        return FetchProgress::Stalled;
    }
    FetchProgress::Idle
}

#[cfg(test)]
mod classify_progress_tests {
    use kithara_test_utils::kithara;

    use super::{FetchProgress, PollStats, classify_progress};

    fn stats(drained: usize, batches: usize) -> PollStats {
        PollStats {
            abr_ticked: false,
            drained_cmds: drained,
            peer_batches: batches,
        }
    }

    #[kithara::test]
    fn advanced_when_abr_tick_ran() {
        let out = classify_progress(
            0,
            0,
            PollStats {
                abr_ticked: true,
                drained_cmds: 0,
                peer_batches: 0,
            },
            0,
        );
        assert_eq!(out, FetchProgress::Advanced);
    }

    #[kithara::test]
    #[case(0, 0)]
    fn idle_when_no_work_anywhere(#[case] inflight_enter: usize, #[case] inflight_exit: usize) {
        let out = classify_progress(inflight_enter, inflight_exit, stats(0, 0), 0);
        assert_eq!(out, FetchProgress::Idle);
    }

    #[kithara::test]
    #[case(1, 0, 0)]
    #[case(0, 1, 0)]
    #[case(0, 0, 1)]
    #[case(3, 0, 0)]
    #[case(0, 2, 0)]
    #[case(0, 0, 5)]
    #[case(2, 3, 4)]
    fn advanced_when_any_activity_counter_positive(
        #[case] drained: usize,
        #[case] batches: usize,
        #[case] dispatched: usize,
    ) {
        let out = classify_progress(0, 0, stats(drained, batches), dispatched);
        assert_eq!(out, FetchProgress::Advanced);
    }

    #[kithara::test]
    #[case(1, 0)]
    #[case(5, 4)]
    #[case(10, 1)]
    #[case(usize::MAX, usize::MAX - 1)]
    fn advanced_when_inflight_decreases(
        #[case] inflight_enter: usize,
        #[case] inflight_exit: usize,
    ) {
        let out = classify_progress(inflight_enter, inflight_exit, stats(0, 0), 0);
        assert_eq!(out, FetchProgress::Advanced);
    }

    #[kithara::test]
    #[case(1, 1)]
    #[case(5, 5)]
    #[case(0, 3)]
    fn stalled_when_inflight_stuck_and_no_counters(
        #[case] inflight_enter: usize,
        #[case] inflight_exit: usize,
    ) {
        let out = classify_progress(inflight_enter, inflight_exit, stats(0, 0), 0);
        assert_eq!(out, FetchProgress::Stalled);
    }

    #[kithara::test]
    #[case((5, 5), (1, 0, 0))]
    #[case((5, 5), (0, 1, 0))]
    #[case((5, 5), (0, 0, 1))]
    #[case((5, 6), (0, 0, 1))]
    fn advanced_dominates_stalled_when_both_signals_present(
        #[case] inflight: (usize, usize),
        #[case] activity: (usize, usize, usize),
    ) {
        let (inflight_enter, inflight_exit) = inflight;
        let (drained, batches, dispatched) = activity;
        let out = classify_progress(
            inflight_enter,
            inflight_exit,
            stats(drained, batches),
            dispatched,
        );
        assert_eq!(out, FetchProgress::Advanced);
    }

    #[kithara::test]
    fn inflight_decrement_alone_yields_advanced() {
        assert_eq!(
            classify_progress(3, 2, stats(0, 0), 0),
            FetchProgress::Advanced
        );
    }
}
