use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Wake, Waker},
};

use super::system::credit::HeldSlot;
use crate::{
    backend::tokio::{
        backend::task::{AbortHandle, Id, coop},
        task::{JoinError, JoinHandle as TaskHandle},
    },
    sync::Arc,
    system::lock::Mutex,
};

/// The scheduling handoff shared by spawned work and its native result handle.
/// Work transfers its existing slot here on exit; Tokio owns result publication.
pub(crate) struct Join {
    state: Mutex<JoinState>,
    counted: bool,
}

#[derive(Default)]
struct JoinState {
    receiver: Option<Arc<Waker>>,
    registration: u64,
    native: Option<AbortHandle>,
    native_wait: bool,
    finished: bool,
    held: Option<HeldSlot>,
}

impl JoinState {
    fn register(&mut self, receiver: Arc<Waker>, counted: bool) -> (u64, Option<Arc<Waker>>) {
        self.registration += 1;
        let previous = self.receiver.replace(receiver);
        if self.finished && counted && self.held.is_none() {
            self.held = Some(HeldSlot::reserve());
        }
        (self.registration, previous)
    }
}

impl Join {
    pub(crate) fn new(counted: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::default(),
            counted,
        })
    }

    /// Retain an existing wait or work-exit handoff before cooperative admission.
    /// A fresh unfinished poll that is refused has no native waiter to release it.
    fn admit(&self, receiver: &Waker) -> (Option<u64>, Option<Arc<Waker>>) {
        let receiver = Arc::new(receiver.clone());
        let (registration, previous) = {
            let mut state = self.state.lock();
            if !state.finished && !state.native_wait {
                return (None, None);
            }
            state.register(receiver, self.counted)
        };
        (Some(registration), previous)
    }

    fn wait(&self, receiver: &Waker) -> (u64, Option<Arc<Waker>>) {
        let receiver = Arc::new(receiver.clone());
        self.state.lock().register(receiver, self.counted)
    }

    /// Arm before native polling: completion can notify while that poll returns Pending.
    fn arm_native(&self, registration: u64) {
        let mut state = self.state.lock();
        if state.registration == registration {
            state.native_wait = true;
        }
    }

    /// Ready and Drop settle unconditionally; a notification owns only its registration.
    fn stop(&self, registration: Option<u64>) {
        let (receiver, wakes) = {
            let mut state = self.state.lock();
            if registration.is_some_and(|registration| registration != state.registration) {
                return;
            }
            state.native_wait = false;
            (
                state.receiver.take(),
                state.held.take().map(HeldSlot::release),
            )
        };
        if let Some(wakes) = wakes {
            wakes.fire();
        }
        drop(receiver);
    }

    /// A discarded cooperative delivery abandons only a wait without a native callback promise.
    fn abandon(&self, registration: u64) {
        let (receiver, wakes) = {
            let mut state = self.state.lock();
            if state.registration != registration || state.native_wait {
                return;
            }
            (
                state.receiver.take(),
                state.held.take().map(HeldSlot::release),
            )
        };
        if let Some(wakes) = wakes {
            wakes.fire();
        }
        drop(receiver);
    }

    fn notify(&self, registration: Option<u64>) {
        let (receiver, terminal) = {
            let mut state = self.state.lock();
            if registration.is_some_and(|registration| registration != state.registration) {
                return;
            }
            if state.native.as_ref().is_some_and(AbortHandle::is_finished) {
                state.native_wait = false;
                (state.receiver.take(), Some(state.registration))
            } else {
                (state.receiver.clone(), None)
            }
        };
        if let Some(receiver) = receiver {
            let _notification = terminal.map(|registration| Notification {
                join: self,
                registration,
            });
            receiver.wake_by_ref();
        }
    }
}

struct Notification<'a> {
    join: &'a Join,
    registration: u64,
}

impl Drop for Notification<'_> {
    fn drop(&mut self) {
        self.join.stop(Some(self.registration));
    }
}

/// A cooperative queue owns this delivery identity, independently of the native waker.
struct DeferredJoin {
    join: Arc<Join>,
    registration: u64,
}

impl Wake for DeferredJoin {
    fn wake(self: Arc<Self>) {
        self.join.notify(Some(self.registration));
    }
}

impl Drop for DeferredJoin {
    fn drop(&mut self) {
        self.join.abandon(self.registration);
    }
}

impl Wake for Join {
    fn wake(self: Arc<Self>) {
        self.notify(None);
    }
}

/// Transfer the finished work's existing slot, or record a slot-less exit.
pub(crate) fn hand_over(join: Option<&Join>, held: Option<HeldSlot>) {
    if let Some(join) = join {
        let (released, duplicate) = {
            let mut state = join.state.lock();
            state.finished = true;
            if state.receiver.is_some() {
                let previous = held.and_then(|held| state.held.replace(held));
                let duplicate = previous.is_some();
                (previous, duplicate)
            } else {
                (held, false)
            }
        };
        drop(released);
        debug_assert!(!duplicate, "work handed over two slots");
    } else {
        drop(held);
    }
}

/// The runtime's result handle, joined through the work's scheduling handoff.
pub struct JoinHandle<T> {
    task: TaskHandle<T>,
    join: Arc<Join>,
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(task: TaskHandle<T>, join: Arc<Join>) -> Self {
        join.state.lock().native = Some(task.abort_handle());
        Self { task, join }
    }

    delegate::delegate! {
        to self.task {
            /// Cancel the task at its next yield point; a finished task keeps its value.
            pub fn abort(&self);
            /// Obtain an independent native cancellation handle.
            pub fn abort_handle(&self) -> AbortHandle;
            /// Whether native work has finished, whatever its outcome.
            pub fn is_finished(&self) -> bool;
            /// The native task's identity.
            pub fn id(&self) -> Id;
        }
    }
}

/// Unwinding an admitted registration or native poll settles only that registration.
struct NativePoll<'a> {
    join: &'a Join,
    registration: u64,
    returned: bool,
}

impl Drop for NativePoll<'_> {
    fn drop(&mut self) {
        if !self.returned {
            self.join.stop(Some(self.registration));
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let join = Arc::clone(&self.join);
        let (admitted, previous) = join.admit(cx.waker());
        let mut native_poll = admitted.map(|registration| NativePoll {
            join: &join,
            registration,
            returned: false,
        });
        drop(previous);
        // The budget is thread-local; no callback runs between this check and admission.
        // Only a refused poll gives the cooperative queue ownership of this identity.
        let deferred = admitted
            .filter(|_| !coop::has_budget_remaining())
            .map(|registration| {
                Waker::from(Arc::new(DeferredJoin {
                    join: Arc::clone(&join),
                    registration,
                }))
            });
        let budget = match deferred.as_ref() {
            Some(waker) => coop::poll_proceed(&mut Context::from_waker(waker)),
            None => coop::poll_proceed(cx),
        };
        let budget = match budget {
            Poll::Ready(budget) => budget,
            Poll::Pending => {
                if let Some(native_poll) = native_poll.as_mut() {
                    native_poll.returned = true;
                }
                return Poll::Pending;
            }
        };
        let registration = match admitted {
            Some(registration) => registration,
            None => {
                let (registration, previous) = join.wait(cx.waker());
                native_poll = Some(NativePoll {
                    join: &join,
                    registration,
                    returned: false,
                });
                drop(previous);
                registration
            }
        };
        join.arm_native(registration);
        let waker = Waker::from(Arc::clone(&join));
        let out = Pin::new(&mut coop::unconstrained(&mut self.task))
            .poll(&mut Context::from_waker(&waker));
        if let Some(native_poll) = native_poll.as_mut() {
            native_poll.returned = true;
        }
        if out.is_ready() {
            budget.made_progress();
            join.stop(None);
        }
        out
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        self.join.stop(None);
    }
}
