use std::{
    collections::VecDeque,
    future::Future,
    panic::Location,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, Waker},
};

use parking_lot::Mutex;

use crate::{
    flash::{
        diag::PrimKind,
        flash_ambient,
        ids::{Backend, trace_native_from_ambient},
        system,
    },
    sync::Arc,
};

/// Real-wake state for the off-flash path: FIFO queue of parked waiters (each a
/// grant flag + waker, mirroring the engine's `granted`) plus a pending
/// `notify_one` permit (consumed by the next `notified()`).
#[derive(Default)]
struct RealNotify {
    waiters: VecDeque<(Arc<AtomicBool>, Waker)>,
    permit: bool,
}

/// `Notify` under `flash`. Each op matches on the construction-latched
/// [`Backend`] (see its contract): engine cvid group when the test is
/// flash-eligible, else a real waker list + permit.
pub struct Notify {
    backend: Backend,
    real: Mutex<RealNotify>,
}

impl Default for Notify {
    #[track_caller]
    fn default() -> Self {
        Self {
            backend: if flash_ambient() {
                let cvid = system::next_condvar_id();
                system::describe_cvid(cvid, PrimKind::Notify, Location::caller());
                Backend::Engine(cvid)
            } else {
                Backend::Native
            },
            real: Mutex::new(RealNotify::default()),
        }
    }
}

impl Notify {
    pub fn notified(&self) -> Notified<'_> {
        Notified {
            state: NotifiedState::Init,
            notify: self,
        }
    }

    /// Grants and wakes one parked waiter; with none parked, stores a permit instead (tokio
    /// semantics).
    pub fn notify_one(&self) {
        match self.backend {
            Backend::Engine(cvid) => system::signal_notify(cvid),
            Backend::Native => {
                trace_native_from_ambient("notify", "notify_one");
                let mut real = self.real.lock();
                let waiter = real.waiters.pop_front();
                match &waiter {
                    Some((granted, _)) => granted.store(true, Ordering::Release),
                    None => real.permit = true,
                }
                drop(real);
                if let Some((_, waker)) = waiter {
                    waker.wake();
                }
            }
        }
    }
}

/// Park state of a [`Notified`] future, so re-poll and `Drop` use the matching
/// teardown. `Real` carries this future's grant flag so re-poll resolves on a
/// grant (never re-parks past a `notify_one`) and `Drop` removes EXACTLY its own
/// entry from the shared waiter queue (leaving a stale one would steal a
/// `notify_one` from a still-parked waiter — a lost wakeup).
enum NotifiedState {
    Init,
    Engine(system::AsyncHandle),
    Real(Arc<AtomicBool>),
    Done,
}

/// Future returned by [`Notify::notified`] under `flash`.
pub struct Notified<'a> {
    notify: &'a Notify,
    state: NotifiedState,
}

impl Future for Notified<'_> {
    type Output = ();

    /// Resolution is grant-driven: a waiter resolves only when `notify_one`/`notify_all` sets its
    /// flag, never from a bare re-check, so a spurious re-poll before that grant stays parked
    /// instead of resolving early or racing a lost wakeup.
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match &self.state {
            NotifiedState::Done => return Poll::Ready(()),
            NotifiedState::Engine(handle) => {
                if handle.granted() {
                    self.state = NotifiedState::Done;
                    return Poll::Ready(());
                }
                return Poll::Pending;
            }
            NotifiedState::Real(granted) => {
                if granted.load(Ordering::Acquire) {
                    self.state = NotifiedState::Done;
                    return Poll::Ready(());
                }
                return Poll::Pending;
            }
            NotifiedState::Init => {}
        }
        match self.notify.backend {
            Backend::Engine(cvid) => {
                let (handle, adv) = system::register_notify_async(cvid, cx.waker().clone());
                match handle {
                    None => {
                        self.state = NotifiedState::Done;
                        Poll::Ready(())
                    }
                    Some(handle) => {
                        self.state = NotifiedState::Engine(handle);
                        adv.fire();
                        Poll::Pending
                    }
                }
            }
            Backend::Native => {
                trace_native_from_ambient("notify", "notified");
                let mut real = self.notify.real.lock();
                if real.permit {
                    real.permit = false;
                    drop(real);
                    self.state = NotifiedState::Done;
                    return Poll::Ready(());
                }
                let granted = Arc::new(AtomicBool::new(false));
                real.waiters
                    .push_back((Arc::clone(&granted), cx.waker().clone()));
                drop(real);
                self.state = NotifiedState::Real(granted);
                Poll::Pending
            }
        }
    }
}

impl Drop for Notified<'_> {
    /// Removes only this future's own entry by grant-flag identity, so `notify_one` cannot steal a
    /// wake from a still-parked peer via an already-dropped future; a grant observed only after
    /// drop is handed to the next waiter or stored as a permit.
    fn drop(&mut self) {
        match std::mem::replace(&mut self.state, NotifiedState::Done) {
            NotifiedState::Engine(handle) => system::cancel_async_wait(&handle),
            NotifiedState::Real(granted) => {
                let mut real = self.notify.real.lock();
                let before = real.waiters.len();
                real.waiters.retain(|(g, _)| !Arc::ptr_eq(g, &granted));
                let still_queued = real.waiters.len() != before;
                if !still_queued && granted.load(Ordering::Acquire) {
                    if let Some((g, w)) = real.waiters.pop_front() {
                        g.store(true, Ordering::Release);
                        drop(real);
                        w.wake();
                    } else {
                        real.permit = true;
                    }
                }
            }
            NotifiedState::Init | NotifiedState::Done => {}
        }
    }
}
