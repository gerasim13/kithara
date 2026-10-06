use std::{
    fmt,
    future::Future,
    panic::Location,
    pin::Pin,
    task::{Context, Poll, Wake, Waker},
};

pub use crate::{
    backend::tokio::task::{JoinError, JoinHandle},
    flash::yield_now,
};
use crate::{
    backend::tokio::{backend::task, runtime::Handle, task as native_task},
    flash::{Yield, system::credit::DedicatedSlot},
    maybe_send::MaybeSend,
    sync::Arc,
    system::lock::Mutex,
};

/// Yield a scheduling opportunity while a participating task retains runnable credit.
/// Its immediate wake keeps Flash's task gate runnable across the pending poll.
#[inline]
pub fn yield_runnable() -> Yield {
    Yield::Real { yielded: false }
}

/// Spawn a participating task with the caller's ambient and dynamic clock mode.
/// Reassert both modes per poll and restore the worker's previous context afterward.
#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let on = crate::flash::ambient_snapshot();
    let active = crate::flash::flash_enabled();
    let loc = Location::caller();
    task::spawn(crate::flash::with_ambient(
        on,
        crate::flash::participate(
            crate::flash::dynamic(
                active,
                crate::no_block::watch_blanket_at("spawn", loc, future),
            ),
            loc,
        ),
    ))
}

/// Spawn a task on the supplied handle with the same per-poll context and
/// participation contract as [`spawn`].
#[track_caller]
pub fn spawn_on<F>(handle: &Handle, future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let on = crate::flash::ambient_snapshot();
    let active = crate::flash::flash_enabled();
    let loc = Location::caller();
    handle.spawn(crate::flash::with_ambient(
        on,
        crate::flash::participate(
            crate::flash::dynamic(
                active,
                crate::no_block::watch_blanket_at("spawn", loc, future),
            ),
            loc,
        ),
    ))
}

/// Spawn a blocking computation on the runtime's blocking pool.
///
/// Off the sim path: a thin pass-through to [`tokio::task::spawn_blocking`].
/// Under `flash` (native), an AMBIENT closure is real work in flight, so
/// it paces the virtual clock exactly like a `spawn_named` thread: the caller
/// reserves the `active` slot BEFORE the pool queues the closure (covering the
/// queue wait), the closure claims it `Running` for its lifetime, and its
/// engine parks release it as usual — the clock advances while the closure
/// WAITS, never while it runs or sits queued. Without this the clock outruns
/// the closure's real execution and virtual deadlines fire against time the
/// work never had. A non-ambient closure stays invisible to the engine.
///
/// The parent's ambient snapshot is also re-established on the blocking thread
/// for the closure's lifetime (thread-locals do not cross the pool), so a
/// blocking computation spawned from a flash test stays flash-eligible.
#[track_caller]
pub fn spawn_blocking<F, R>(f: F) -> BlockingJoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (work, completion) = BlockingJoinHandle::prepare(f);
    BlockingJoinHandle::new(native_task::spawn_blocking(work), completion)
}

/// Spawn synchronous work without blocking an async runtime worker.
#[track_caller]
pub fn spawn_sync<F, R>(f: F) -> BlockingJoinHandle<R>
where
    F: FnOnce() -> R + MaybeSend + 'static,
    R: MaybeSend + 'static,
{
    spawn_blocking(f)
}

/// Spawn a blocking computation on a specific runtime [`Handle`].
///
/// Same ambient propagation and quiescence accounting as [`spawn_blocking`],
/// but queued onto the captured runtime handle.
///
/// Reserves the `active` slot before the pool queues the closure, covering the queue wait; the
/// slot's `Drop` returns the reservation if the pool never runs it.
#[track_caller]
pub fn spawn_blocking_on<F, R>(handle: &Handle, f: F) -> BlockingJoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (work, completion) = BlockingJoinHandle::prepare(f);
    BlockingJoinHandle::new(handle.spawn_blocking(work), completion)
}

/// A blocking task's native result handle with its virtual-clock completion handoff.
/// Dropping the handle detaches the work, matching Tokio's join contract.
#[must_use = "dropping a blocking join handle detaches its task"]
#[derive(derive_more::Debug)]
#[debug(bound(R: fmt::Debug))]
#[debug("{:?}", inner)]
pub struct BlockingJoinHandle<R> {
    inner: JoinHandle<R>,
    completion: Option<Arc<BlockingCompletion>>,
}

impl<R> BlockingJoinHandle<R> {
    #[track_caller]
    pub(in crate::flash) fn prepare<F>(
        f: F,
    ) -> (
        impl FnOnce() -> R + Send + 'static,
        Option<Arc<BlockingCompletion>>,
    )
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let origin = Location::caller();
        let completion = crate::flash::ambient_snapshot().then(|| {
            Arc::new(BlockingCompletion {
                origin,
                state: Mutex::default(),
            })
        });
        let work = crate::flash::thread::wrap_pool_task_with_completion(f, completion.clone());
        (work, completion)
    }

    pub(in crate::flash) fn new(
        inner: JoinHandle<R>,
        completion: Option<Arc<BlockingCompletion>>,
    ) -> Self {
        if let Some(completion) = &completion {
            completion.state.lock().native = Some(inner.abort_handle());
        }
        Self { inner, completion }
    }

    delegate::delegate! {
        to self.inner {
            /// Abort the task if it has not started running.
            pub fn abort(&self);
            /// Obtain a handle that can abort queued work independently of its result owner.
            pub fn abort_handle(&self) -> task::AbortHandle;
            /// Return whether the native task has completed.
            pub fn is_finished(&self) -> bool;
            /// Return the native task's identity.
            pub fn id(&self) -> task::Id;
        }
    }
}

impl<R> Future for BlockingJoinHandle<R> {
    type Output = Result<R, JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(completion) = &this.completion else {
            return Pin::new(&mut this.inner).poll(cx);
        };
        completion.register(cx.waker());
        let waker = Waker::from(Arc::clone(completion));
        let outcome = Pin::new(&mut this.inner).poll(&mut Context::from_waker(&waker));
        if outcome.is_ready() {
            completion.settle(None);
        }
        outcome
    }
}

impl<R> Drop for BlockingJoinHandle<R> {
    fn drop(&mut self) {
        if let Some(completion) = &self.completion {
            completion.settle(None);
        }
    }
}

/// One job owns the handoff between pooled execution and native result publication.
/// Tokio remains the sole result owner; this state only tracks its registered receiver.
pub(in crate::flash) struct BlockingCompletion {
    origin: &'static Location<'static>,
    state: Mutex<CompletionState>,
}

#[derive(Default)]
struct CompletionState {
    receiver: Option<Waker>,
    /// Equal wakers can belong to distinct polls that defer native readiness.
    registration: u64,
    native: Option<task::AbortHandle>,
    finishing: bool,
    credit: Option<DedicatedSlot>,
}

impl BlockingCompletion {
    /// A native poll can defer its wake even after publication; every such
    /// scheduling handoff holds credit until the receiver wakes or the poll is Ready.
    fn register(&self, receiver: &Waker) {
        let mut state = self.state.lock();
        state.registration += 1;
        state.receiver = Some(receiver.clone());
        if state.finishing && state.credit.is_none() {
            state.credit = Some(DedicatedSlot::reserve(self.origin));
        }
    }

    /// Runs before the pooled participant or never-claimed reservation is released,
    /// including panic and queued cancellation. Unobserved results hold no credit.
    pub(in crate::flash) fn finish(&self) {
        let mut state = self.state.lock();
        state.finishing = true;
        if state.receiver.is_some() && state.credit.is_none() {
            state.credit = Some(DedicatedSlot::reserve(self.origin));
        }
    }

    /// A completed join settles unconditionally; a wake settles only the poll it forwarded.
    fn settle(&self, registration: Option<u64>) {
        let wakes = {
            let mut state = self.state.lock();
            if registration.is_some_and(|registration| registration != state.registration) {
                return;
            }
            state.receiver = None;
            state.credit.take().map(DedicatedSlot::release)
        };
        if let Some(wakes) = wakes {
            wakes.fire();
        }
    }
}

/// Return the forwarded poll's credit after its terminal wake, including unwind.
struct CompletionNotification<'a> {
    completion: &'a BlockingCompletion,
    registration: u64,
}

impl Drop for CompletionNotification<'_> {
    fn drop(&mut self) {
        self.completion.settle(Some(self.registration));
    }
}

impl Wake for BlockingCompletion {
    /// Forward readiness and cooperative wakes. Published results return completion
    /// credit only after the receiver's participating waker acquires runnable credit.
    fn wake(self: Arc<Self>) {
        let (receiver, registration) = {
            let mut state = self.state.lock();
            if state
                .native
                .as_ref()
                .is_some_and(task::AbortHandle::is_finished)
            {
                (state.receiver.take(), Some(state.registration))
            } else {
                (state.receiver.clone(), None)
            }
        };
        if let Some(receiver) = receiver {
            let _notification = registration.map(|registration| CompletionNotification {
                completion: &self,
                registration,
            });
            receiver.wake();
        }
    }
}
