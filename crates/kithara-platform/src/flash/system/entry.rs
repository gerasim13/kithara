//! Parked-waiter records: what a waiter is ([`WaitKind`]), what its deadline
//! means to the engine ([`ParkRole`]), and where a deadline-less waiter parked
//! ([`Parked`]).

use std::backtrace::Backtrace;

use super::{CvId, credit, wake::Wake};
use crate::flash::{diag, ids::ThreadKey};

/// What kind of waiter an [`Entry`] is, so a signal targets the right group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WaitKind {
    /// A timed waiter with no early-wake channel: woken solely by the engine
    /// crossing its deadline. Used by the test harness `park_for` and by the
    /// async `sleep` future (`register_sleep_async`).
    Timed,
    /// An unparkable thread park (`park_timeout`): woken by the deadline OR by
    /// [`FlashInner::unpark`] targeting this thread id.
    Thread(ThreadKey),
    /// A condvar waiter: woken by the deadline (when timed) OR by
    /// [`FlashInner::signal_condvar`] for this condvar id.
    Condvar(CvId),
}

/// Where a deadline-less waiter parked, for the hang dump. Only `indef` entries
/// carry it: a timed waiter is released by the engine crossing its deadline, so
/// its parking site names nothing the deadline does not already name.
pub(super) struct Parked {
    /// The stack that parked, only under `KITHARA_FLASH_SYNC_BT`. Unlike the
    /// dump's own backtrace this belongs to the waiter, not to the watchdog that
    /// took the snapshot, so it names the frame that owes the wait.
    pub(super) stack: Option<Backtrace>,
    /// The OS thread that parked. Read for SYNC waiters — an async waiter's
    /// parking thread is merely whichever worker polled it. Against
    /// [`Registry::bridged`] it says whether the wait is held from INSIDE an
    /// async poll, i.e. whether it strands the tasks that thread owes a poll.
    pub(super) thread: ThreadKey,
}

impl Parked {
    /// Capture the current parking site. Callers MUST call this BEFORE taking
    /// the `core` lock: a stack walk under `core` would hold the whole engine
    /// for its duration.
    pub(super) fn here() -> Self {
        Self {
            thread: credit::current_thread_key(),
            stack: diag::parked_backtrace(),
        }
    }
}

/// What a timed park's deadline MEANS to the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::flash) enum ParkRole {
    /// The deadline is the event: nothing else releases this waiter on time,
    /// so the clock owes it a stop.
    Deadline,
    /// The deadline is a BACKSTOP under a [`WaitGate`](crate::sync::WaitGate)
    /// edge: the waiter expects a signal and carries a timeout only so a
    /// missed edge cannot wedge it. Re-checking the edge when nothing has
    /// happened observes nothing, so the clock bounds such a park instead of
    /// stopping at it.
    Backstop,
    /// The deadline is a POLL for state that changes without a wake to this
    /// waiter: holding the [`Clock::posts`] count read before the poller last
    /// checked. That count moves on every deferred post and on every wake of
    /// any participant except a poll's own deadline, since each such wake may
    /// queue work the poller only finds by looking. Until it moves, re-checking
    /// observes nothing and the park is a [`ParkRole::Backstop`]; after it
    /// moves, the poll is how the waiter observes it, so the clock owes it a
    /// stop.
    Poll(u64),
}

impl ParkRole {
    /// Whether the clock owes this park a stop at its deadline, given the
    /// current [`Clock::posts`] count.
    pub(super) fn owes_stop(self, posts: u64) -> bool {
        match self {
            Self::Deadline => true,
            Self::Backstop => false,
            Self::Poll(seen) => seen != posts,
        }
    }
}

/// A parked waiter's wake handle plus the group it belongs to.
pub(super) struct Entry {
    /// `Some` for deadline-less waiters only. See [`Parked`].
    pub(super) parked: Option<Parked>,
    /// What this entry's deadline means. An entry that owes no stop
    /// ([`ParkRole::owes_stop`]) never becomes the advance target while some
    /// other deadline exists; a jump that passes it still wakes it.
    pub(super) role: ParkRole,
    pub(super) kind: WaitKind,
    pub(super) wake: Wake,
}

impl Entry {
    /// A timed waiter whose deadline means `role`.
    pub(super) fn with_role(kind: WaitKind, wake: Wake, role: ParkRole) -> Self {
        Self {
            kind,
            wake,
            role,
            parked: None,
        }
    }

    /// A waiter with NO deadline: only a matching signal frees it, so when one
    /// of these pins quiescence its parking site IS the diagnosis.
    pub(super) fn indef(kind: WaitKind, wake: Wake, parked: Parked) -> Self {
        Self {
            kind,
            wake,
            role: ParkRole::Deadline,
            parked: Some(parked),
        }
    }

    /// A waiter the engine itself will release by crossing its deadline, so no
    /// parking site is recorded.
    pub(super) fn timed(kind: WaitKind, wake: Wake) -> Self {
        Self {
            kind,
            wake,
            role: ParkRole::Deadline,
            parked: None,
        }
    }
}
