use std::{
    marker::PhantomData,
    mem,
    panic::Location,
    sync::{
        Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{Core, FLASH, FlashInner, SyncHolder, WaiterId};
use crate::{
    backend::thread::current,
    common::thread_id::ACTIVE_NAMED_THREADS,
    flash::{
        ctx::{self, Credit},
        ids::ThreadKey,
        join::{Join, hand_over},
    },
    sync::Arc,
};

/// `ThreadKey` of the calling OS thread — the key under which it appears in the
/// engine's sync-holder dump map (matches `park_timeout`'s keying).
pub(super) fn current_thread_key() -> ThreadKey {
    ThreadKey::of(current().id())
}

/// The calling OS thread's name for the sync-holder dump (`spawn_named` pacers
/// are always named; pool/main threads may be anonymous). Pure introspection —
/// no engine interaction.
fn current_thread_name() -> Option<String> {
    std::thread::current().name().map(str::to_owned)
}

/// True when the calling OS thread is inside an async-task poll (a runtime
/// worker driving a [`Participating`](crate::flash::Participating) future) —
/// non-zero poll depth. A wrapped sync wait taken while this holds is a
/// BRIDGED wait: the worker is not a dedicated pacer (it returns to the
/// runtime when the poll yields, parking on the runtime, never on the
/// engine), and the task it drives is parked for the duration of the block.
/// Such a wait releases the `active_async` slot on enter and re-acquires it
/// on wake (so the clock can advance while the worker blocks on the engine),
/// and NEVER enters the sync `active` count (which would leak — the worker
/// has no matching `enter_wait`/exit to balance it).
fn in_async_poll() -> bool {
    ctx::poll_depth() > 0
}

/// Mark the current thread as a dedicated virtual-time pacer.
/// Dedicated threads hold active credit while working between wrapped waits.
///
/// `origin` is a parameter rather than `#[track_caller]` because a spawned
/// child claims on its OWN thread: the caller here is always the spawn shim,
/// which reads the same for every consumer and so tells two blocking jobs
/// apart in the hang dump. The shim passes the site it was called from.
pub(crate) fn mark_dedicated(origin: &'static Location<'static>) {
    ctx::set_dedicated(true);
    ctx::set_credit(Credit::Running);
    FLASH.sync_holder_running(origin);
}

/// Dedicated-pacer credit reserved on the parent before spawning or queueing,
/// then moved to the child and claimed as `Running`. Reserving only after the
/// child starts lets a sibling observe `active == 0` and advance its watchdog
/// before the pacer's initial decode/ring-fill burst runs.
/// An unclaimed slot returns all reservations on drop, including `active` and,
/// for `spawn_named`, the named-thread count, so failed or discarded work cannot
/// wedge the engine.
#[must_use]
pub(crate) struct DedicatedSlot {
    /// True for the `spawn_named` variant, which owns the
    /// `ACTIVE_NAMED_THREADS` increment alongside the `active` reservation.
    /// The `spawn_blocking` variant owns only the credit.
    named: bool,
    /// The spawn site, captured on the parent and moved into the child with
    /// the reservation, so the claim can name WHO is holding the engine.
    origin: &'static Location<'static>,
}

impl DedicatedSlot {
    /// Claim the reservation on a `spawn_named` child: mark this thread a
    /// dedicated pacer holding the reserved slot as `Running`. The returned
    /// [`Participant`] owes the exit settle (and the named-count decrement).
    pub(crate) fn claim_dedicated(self) -> Participant {
        debug_assert!(self.named, "claim_dedicated on a spawn_blocking slot");
        let named = self.named;
        let origin = self.origin;
        mem::forget(self);
        mark_dedicated(origin);
        Participant {
            named,
            _not_send: PhantomData,
        }
    }

    /// Claim the reservation on a POOLED blocking thread for ONE closure: an
    /// ambient `task::spawn_blocking` closure is real work in flight, so the
    /// clock must not advance while it runs (its engine parks still release
    /// the credit, exactly like a `spawn_named` pacer). The returned
    /// [`PoolParticipant`] settles the exit and restores the pool thread's
    /// previous dedicated flag, so a reused thread does not pace later
    /// unrelated tasks. A closure with a `JoinHandle` passes its `join`, which
    /// the exit hands the credit to.
    pub(crate) fn claim_pooled(self, join: Option<Arc<Join>>) -> PoolParticipant {
        debug_assert!(!self.named, "claim_pooled on a spawn_named slot");
        let origin = self.origin;
        mem::forget(self);
        let prev = ctx::dedicated();
        mark_dedicated(origin);
        PoolParticipant {
            join,
            prev_dedicated: prev,
            _not_send: PhantomData,
        }
    }

    /// Claim an unnamed platform thread's reservation.
    pub(crate) fn claim_thread(self) -> Participant {
        debug_assert!(!self.named, "claim_thread on a named slot");
        let origin = self.origin;
        mem::forget(self);
        mark_dedicated(origin);
        Participant {
            named: false,
            _not_send: PhantomData,
        }
    }

    /// Reserve an unnamed platform thread or ambient blocking closure: the
    /// `active` slot only (the named-thread count is not this path's resource).
    ///
    /// `origin` is the spawn site; the spawning shim passes its own
    /// `Location::caller()` so the dump names the consumer, not the shim.
    pub(crate) fn reserve(origin: &'static Location<'static>) -> Self {
        FLASH.pre_count_dedicated();
        Self {
            named: false,
            origin,
        }
    }

    /// Reserve for a `spawn_named` pacer thread: the `active` slot AND the
    /// named-thread count, both returned by Drop if the slot is never claimed.
    pub(crate) fn reserve_named(origin: &'static Location<'static>) -> Self {
        ACTIVE_NAMED_THREADS.fetch_add(1, Ordering::Release);
        FLASH.pre_count_dedicated();
        Self {
            named: true,
            origin,
        }
    }
}

impl Drop for DedicatedSlot {
    /// An unconsumed reservation returns the raw `active` count directly on drop, since no thread
    /// ever claimed the slot as credit; this release may itself be the quiescent edge.
    fn drop(&mut self) {
        FLASH.release_slot();
        if self.named {
            ACTIVE_NAMED_THREADS.fetch_sub(1, Ordering::Release);
        }
    }
}

/// RAII exit settle of a dedicated `spawn_named` pacer (and, via
/// [`Participant::unreserved`], of a slot-less non-ambient pool closure):
/// Drop (incl. unwind through a panicking body) runs the participant-exit
/// settle — read + clear the credit, release the `active` slot if the thread
/// exits `Running` — and decrements the named-thread count when this
/// participant owns it.
#[must_use]
pub(crate) struct Participant {
    _not_send: PhantomData<*mut ()>,
    named: bool,
}

impl Participant {
    /// Exit settle WITHOUT a reservation: the non-ambient `spawn_blocking`
    /// arm. Such a closure is invisible to the engine (it never becomes
    /// `Running` through the ambient bracket), so the settle is a defensive
    /// no-op on the happy path — but RAII keeps the exit unwind-safe and
    /// consistent with the ambient arm.
    pub(crate) fn unreserved() -> Self {
        Self {
            named: false,
            _not_send: PhantomData,
        }
    }
}

impl Drop for Participant {
    fn drop(&mut self) {
        FLASH.on_participant_exit();
        if self.named {
            ACTIVE_NAMED_THREADS.fetch_sub(1, Ordering::Release);
        }
    }
}

/// RAII making a POOLED blocking thread a dedicated pacer for ONE closure
/// (see [`DedicatedSlot::claim_pooled`]). Drop settles the credit, handing it
/// to the closure's joiner when it has one, and restores the pool thread's
/// previous dedicated flag.
#[must_use]
pub(crate) struct PoolParticipant {
    _not_send: PhantomData<*mut ()>,
    join: Option<Arc<Join>>,
    prev_dedicated: bool,
}

impl Drop for PoolParticipant {
    fn drop(&mut self) {
        let running = FLASH.exit_running();
        ctx::set_dedicated(self.prev_dedicated);
        if running {
            hand_over(self.join.as_deref(), HeldSlot { _priv: () });
        }
    }
}

/// An `active` count no thread holds as credit any more: the slot of work that
/// has finished, kept until whoever joins that work has been woken. The
/// runtime wakes a joiner only after the work has returned, so a slot released
/// as the work returns leaves a moment in which every participant looks parked
/// and the clock jumps past the joiner. Drop returns the count, which may be
/// the quiescent edge.
#[must_use]
pub(crate) struct HeldSlot {
    _priv: (),
}

impl Drop for HeldSlot {
    fn drop(&mut self) {
        FLASH.release_slot();
    }
}

#[derive(Clone, Copy)]
pub(super) enum AsyncKey {
    Timed((u64, WaiterId)),
    Indef(WaiterId),
    Yield(WaiterId),
}

/// Unique receipt retaining one grant credit through wake and poll latency.
/// Drop removes an ungranted entry or settles that credit on the creating
/// engine, whose weak identity does not keep a stopped engine alive.
pub(crate) struct AsyncHandle {
    pub(super) owner: Weak<FlashInner>,
    pub(super) granted: Arc<AtomicBool>,
    pub(super) key: AsyncKey,
    pub(super) task: Option<u64>,
}

impl AsyncHandle {
    /// Observe an engine grant only while its owner is alive. A different
    /// clock advance cannot resolve this waiter; only its firer sets the flag.
    pub(crate) fn granted(&self) -> bool {
        self.granted.load(Ordering::Acquire) && self.owner.strong_count() != 0
    }
}

impl Drop for AsyncHandle {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            owner.release_async_wait(self);
        }
    }
}

impl Core {
    /// Move a finished task's `active_async` slot into a [`HeldSlot`]. Adding
    /// to `active` cannot enable an advance, so this runs no advance rule.
    pub(super) fn hold_async(&mut self, id: u64) -> HeldSlot {
        debug_assert!(
            self.registry.active_async > 0,
            "async hold without a matching acquire"
        );
        self.registry.active_async -= 1;
        self.registry.active_async_holders.remove(&id);
        self.registry.active += 1;
        HeldSlot { _priv: () }
    }
}

/// RAII bracket marking the current OS thread as inside an async-task poll for the
/// guard's lifetime. Held by [`Participating::poll`](crate::flash::Participating) around
/// the inner future's poll, so a wrapped sync wait taken inside that poll is
/// recognised as bridged (see [`in_async_poll`]). Drop-safe across an unwind.
pub(in crate::flash) struct AsyncPollGuard {
    /// The top-of-stack task identity to restore on drop (LIFO), so a nested
    /// poll on the same thread does not lose the outer task's identity.
    prev_cur: Option<(u64, &'static Location<'static>)>,
    _not_send: PhantomData<*mut ()>,
}

impl AsyncPollGuard {
    /// Enter a task poll: bump the poll depth and publish this task's identity
    /// (`id`, spawn `loc`) as the top of the stack, so a bridged sync wait taken
    /// inside the poll can keep the async-holder dump map exact.
    pub(in crate::flash) fn enter(id: u64, loc: &'static Location<'static>) -> Self {
        ctx::set_poll_depth(ctx::poll_depth().saturating_add(1));
        let prev_cur = ctx::swap_cur_async(Some((id, loc)));
        Self {
            prev_cur,
            _not_send: PhantomData,
        }
    }
}

impl Drop for AsyncPollGuard {
    fn drop(&mut self) {
        ctx::swap_cur_async(self.prev_cur);
        ctx::set_poll_depth(ctx::poll_depth().saturating_sub(1));
    }
}

/// Obligation minted with one engine wait under the same `core` lock.
/// After `token.wait()`, consume it exactly once: production waits use
/// [`WaitGuard::resume`] to settle the firer's active bump by credit class;
/// only harness `park_for` uses [`WaitGuard::mark_running`] for its asymmetric
/// bracket. The guard retains the entering engine so settlement reaches the
/// same `Core`. Dropping it unconsumed leaks the bump and triggers a debug assert.
#[must_use]
pub(crate) struct WaitGuard<'a> {
    flash: &'a FlashInner,
    _not_send: PhantomData<*mut ()>,
}

impl WaitGuard<'_> {
    /// Test-only bare settle: mark this thread `Running`, keeping the firer's
    /// bump, WITHOUT the credit-class dispatch of [`WaitGuard::resume`]. The
    /// harness `park_for` site is un-bracketed (no spawn bracket balances its
    /// credit), so the non-dedicated `resume` arm would wrongly
    /// `active -= 1` + advance there. Deliberately asymmetric — do not
    /// convert `park_for` to `resume()`.
    #[cfg(test)]
    pub(super) fn mark_running(self) {
        mem::forget(self);
        ctx::set_credit(Credit::Running);
    }

    /// Settle the firer's `active` bump per this thread's credit class —
    /// see [`FlashInner::resume_after_wait`].
    #[track_caller]
    pub(crate) fn resume(self) {
        let flash = self.flash;
        mem::forget(self);
        flash.resume_after_wait();
    }
}

impl Drop for WaitGuard<'_> {
    /// Asserting here during a mid-unwind drop (a panic between mint and settle) would double-panic
    /// into an abort and mask the root panic, so the guard only debug-asserts that unwinding is in
    /// progress.
    fn drop(&mut self) {
        debug_assert!(
            std::thread::panicking(),
            "WaitGuard dropped without resume()/mark_running() — wrapped wait left unsettled"
        );
    }
}

/// Reset this thread's credit to `None`. Called at the start of a pooled
/// thread's body (spawn bracket) so a reused OS thread does not inherit a stale
/// credit from a previous task.
pub(crate) fn reset_credit() {
    ctx::set_credit(Credit::None);
}

impl FlashInner {
    /// Settle one unique async receipt after consume or cancellation. The
    /// grant and entry removal share this lock with the firer, so either the
    /// entry is removed before grant or its retained credit is returned once.
    fn release_async_wait(&self, handle: &AsyncHandle) {
        let mut s = self.core.lock();
        match handle.key {
            AsyncKey::Timed(key) => {
                s.sched.timed.remove(&key);
            }
            AsyncKey::Indef(id) => {
                s.sched.indef.remove(&id);
            }
            AsyncKey::Yield(id) => {
                s.sched.yielders.remove(&id);
            }
        }
        if handle.granted.load(Ordering::Acquire) {
            match handle.task.and_then(|id| s.registry.task_diag.get_mut(&id)) {
                Some(task) => {
                    debug_assert!(task.grants > 0, "async grant without task credit");
                    task.grants -= 1;
                }
                None => {
                    debug_assert!(s.registry.active > 0, "async grant without retained credit");
                    s.registry.active -= 1;
                }
            }
            if let Some(id) = handle.task {
                s.registry.remove_settled_task(id);
            }
        }
        let adv = s.try_advance(&self.clock);
        drop(s);
        adv.fire();
    }

    /// Enters one wrapped wait under `core` and returns the [`WaitGuard`] to consume
    /// once `token.wait()` returns. First-wait uncounted threads become `Parked`
    /// without decrementing `active`; their first wake adds the credit and marks
    /// them running. A counted `Running` thread decrements `active` before parking.
    /// Already `Parked` is unreachable: a thread waits on only one thing at a time.
    pub(super) fn enter_wait_locked(&self, s: &mut Core) -> WaitGuard<'_> {
        if in_async_poll() {
            crate::no_block::forbid_bridged(ctx::cur_async().map(|(_, loc)| loc));
            debug_assert!(
                s.registry.active_async > 0,
                "bridged wait must be inside a counted async poll"
            );
            s.registry.active_async -= 1;
            if let Some((id, _)) = ctx::cur_async() {
                s.registry.active_async_holders.remove(&id);
            }
            s.registry.bridged.insert(current_thread_key());
        } else if !ctx::dedicated() {
        } else {
            match ctx::credit() {
                Credit::None => ctx::set_credit(Credit::Parked),
                Credit::Running => {
                    debug_assert!(
                        s.registry.active > 0,
                        "running participant must be counted in active"
                    );
                    s.registry.active -= 1;
                    ctx::set_credit(Credit::Parked);
                    s.registry.active_sync_holders.remove(&current_thread_key());
                }
                Credit::Parked => {
                    debug_assert!(false, "a thread cannot enter two wrapped waits at once");
                }
            }
        }
        WaitGuard {
            flash: self,
            _not_send: PhantomData,
        }
    }

    /// Decrement `active` for a thread that EXITS while `Running` — the balancing
    /// half of the bootstrap (`None -> Parked` left `active` untouched; the first
    /// wake then `active += 1`'d it). Called from the spawn bracket after the
    /// thread's body returns. If the credit was `Running`, drops it from `active`
    /// and fires any advance the drop unblocks.
    pub(in crate::flash) fn on_participant_exit(&self) {
        if self.exit_running() {
            self.release_slot();
        }
    }

    /// Read and clear the exiting thread's credit. True when it exits
    /// `Running`: the thread is no longer a holder, but its count is still in
    /// `active`, and the caller owes its release.
    fn exit_running(&self) -> bool {
        let was = ctx::credit();
        ctx::set_credit(Credit::None);
        if was != Credit::Running {
            return false;
        }
        let mut s = self.core.lock();
        debug_assert!(
            s.registry.active > 0,
            "exiting running participant must be counted"
        );
        s.registry.active_sync_holders.remove(&current_thread_key());
        true
    }

    /// Reserve a dedicated pacer's slot: raw `active += 1` on the parent,
    /// before the child is scheduled (see [`DedicatedSlot`]).
    pub(in crate::flash) fn pre_count_dedicated(&self) {
        self.core.lock().registry.active += 1;
    }

    /// Return an `active` count no thread holds as credit — a reservation no
    /// thread ever claimed ([`DedicatedSlot`]), a finished participant's
    /// [`HeldSlot`], or an exiting thread's cleared credit — and fire any
    /// advance the release unblocks.
    fn release_slot(&self) {
        let mut s = self.core.lock();
        debug_assert!(
            s.registry.active > 0,
            "slot release without a matching count"
        );
        s.registry.active -= 1;
        let adv = s.try_advance(&self.clock);
        drop(s);
        adv.fire();
    }

    /// Resume accounting after a wrapped sync wait's `token.wait()` returned. The firer
    /// always `active += 1`'d the woken Sync entry to cover wake latency; how that is
    /// settled depends on the thread:
    /// - BRIDGED (runtime worker mid async-poll): undo the `active` bump and re-acquire
    ///   the `active_async` slot released on enter.
    /// - NON-DEDICATED, non-async: undo the `active` bump (the thread is not a pacer and
    ///   never entered `active` on the wait side).
    /// - DEDICATED pacer: keep the bump and mark the thread `Running`.
    #[track_caller]
    pub(in crate::flash) fn resume_after_wait(&self) {
        if in_async_poll() {
            let mut s = self.core.lock();
            debug_assert!(
                s.registry.active > 0,
                "bridged resume without a firer active bump"
            );
            s.registry.active -= 1;
            s.registry.active_async += 1;
            if let Some((id, loc)) = ctx::cur_async() {
                s.registry.active_async_holders.insert(id, loc);
            }
            s.registry.bridged.remove(&current_thread_key());
            drop(s);
            return;
        }
        if !ctx::dedicated() {
            let mut s = self.core.lock();
            debug_assert!(
                s.registry.active > 0,
                "non-pacer resume without a firer active bump"
            );
            s.registry.active -= 1;
            let adv = s.try_advance(&self.clock);
            drop(s);
            adv.fire();
            return;
        }
        ctx::set_credit(Credit::Running);
        self.sync_holder_running(Location::caller());
    }

    /// Record (or refresh) the calling dedicated pacer thread as a `Running`
    /// sync `active` holder in the diagnostic dump map. Called when it claims its
    /// slot ([`mark_dedicated`], which passes the spawn site) and each time it
    /// resumes `Running` from a wait (dedicated
    /// [`resume_after_wait`](FlashInner::resume_after_wait), which passes the
    /// wait site). Keyed by the thread; named by it. Diagnostic only — it does
    /// not touch `active`.
    pub(in crate::flash) fn sync_holder_running(&self, resumed_from: &'static Location<'static>) {
        let key = current_thread_key();
        let name = current_thread_name();
        let resumed_at_real_ns = self.clock.real_now_nanos();
        self.core.lock().registry.active_sync_holders.insert(
            key,
            SyncHolder {
                resumed_from,
                name,
                resumed_at_real_ns,
            },
        );
    }
}
