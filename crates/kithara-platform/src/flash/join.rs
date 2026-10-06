use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Wake, Waker, ready},
};

use super::system::credit::HeldSlot;
use crate::{
    backend::tokio::{
        backend::task::coop,
        task::{JoinError, JoinHandle as TaskHandle},
    },
    sync::Arc,
    system::lock::Mutex,
};

/// What a spawned task or blocking closure shares with its [`JoinHandle`].
///
/// The runtime wakes a joiner only after the joined work has returned, and
/// the work's engine slot ends as it returns. Released there, it would leave a
/// moment in which every participant looks parked, and the clock would jump to
/// the joiner's own deadline before the joiner learned the work was done. So
/// while a poll of the handle is waiting, the finished work's slot is held
/// here, and the runtime's join wake releases it only after waking the joiner.
/// Work nobody is waiting on releases its slot as it finishes.
#[derive(Default)]
pub(crate) struct Join {
    state: Mutex<JoinState>,
}

#[derive(Default)]
struct JoinState {
    /// The waker of the poll waiting on the handle.
    joiner: Option<Waker>,
    /// The finished work's slot, held for that poll.
    held: Option<HeldSlot>,
}

impl Join {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The handle is waiting, from the task `waker` wakes.
    fn wait(&self, waker: &Waker) {
        let mut state = self.state.lock();
        match state.joiner.as_ref() {
            Some(joiner) if joiner.will_wake(waker) => {}
            _ => state.joiner = Some(waker.clone()),
        }
    }

    /// The handle stopped waiting (its poll finished, or it dropped): forget
    /// the joiner and release a slot held for it.
    fn stop(&self) {
        let (_joiner, held) = self.take();
        drop(held);
    }

    fn take(&self) -> (Option<Waker>, Option<HeldSlot>) {
        let mut state = self.state.lock();
        (state.joiner.take(), state.held.take())
    }
}

impl Wake for Join {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    /// The runtime's join wake: wake the joiner first, so it holds a slot of
    /// its own, then release the finished work's slot.
    fn wake_by_ref(self: &Arc<Self>) {
        let (joiner, held) = self.take();
        if let Some(joiner) = joiner {
            joiner.wake();
        }
        drop(held);
    }
}

/// Hand finished work's slot to `join`: held there while a poll of the handle
/// is waiting, released at once otherwise or when the work has no handle.
pub(crate) fn hand_over(join: Option<&Join>, held: HeldSlot) {
    if let Some(join) = join {
        let mut state = join.state.lock();
        if state.joiner.is_some() {
            debug_assert!(state.held.is_none(), "work finished twice");
            state.held = Some(held);
            return;
        }
    }
    drop(held);
}

/// Handle to a spawned task or blocking closure: the runtime's handle, waited
/// on through the [`Join`] it shares with the work.
pub struct JoinHandle<T> {
    task: TaskHandle<T>,
    join: Arc<Join>,
}

impl<T> JoinHandle<T> {
    pub(crate) fn new(task: TaskHandle<T>, join: Arc<Join>) -> Self {
        Self { task, join }
    }

    delegate::delegate! {
        to self.task {
            /// Cancel the task at its next yield point; the join then reports it
            /// cancelled. A finished task keeps its value.
            pub fn abort(&self);
            /// Whether the work has finished, whatever its outcome.
            pub fn is_finished(&self) -> bool;
        }
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    /// The runtime is handed the [`Join`] as the waker, so its join wake goes
    /// through the handoff. A poll the task budget turns away registers no
    /// join wake, so nothing would release a slot held for it: it yields with
    /// the caller's own waker and leaves the [`Join`] waiting on nothing. The
    /// budget is charged here, once, and the runtime's handle is polled
    /// unconstrained, so a poll that waits always registers the join wake.
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let budget = ready!(coop::poll_proceed(cx));
        self.join.wait(cx.waker());
        let waker = Waker::from(Arc::clone(&self.join));
        let out = Pin::new(&mut coop::unconstrained(&mut self.task))
            .poll(&mut Context::from_waker(&waker));
        if out.is_ready() {
            budget.made_progress();
            self.join.stop();
        }
        out
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        self.join.stop();
    }
}
