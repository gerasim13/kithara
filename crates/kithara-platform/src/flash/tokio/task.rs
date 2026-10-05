use std::{future::Future, panic::Location};

pub use crate::{
    backend::tokio::task::JoinError,
    flash::{join::JoinHandle, yield_now},
};
use crate::{
    backend::tokio::{backend::task, runtime::Handle, task as native_task},
    flash::join::Join,
    maybe_send::MaybeSend,
    sync::Arc,
};

/// Spawns async work through the platform's accounting boundary. Under flash,
/// [`crate::flash::participate`] counts each active poll so virtual time cannot
/// race its execution; consumers cannot depend directly on raw Tokio spawn.
/// [`crate::flash::with_ambient`] restores the parent's ambient snapshot per poll
/// across worker threads. That outer wrap covers both accounting and task code.
/// Outside simulation, delegates to native Tokio.
/// Completed work keeps its engine credit until its waiting joiner wakes.
#[track_caller]
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let on = crate::flash::ambient_snapshot();
    let loc = Location::caller();
    let join = Join::new();
    let task = task::spawn(crate::flash::with_ambient(
        on,
        crate::flash::participate(crate::no_block::watch_blanket_at("spawn", loc, future), loc)
            .joined(Arc::clone(&join)),
    ));
    JoinHandle::new(task, join)
}

/// Spawn a future on a SPECIFIC runtime [`Handle`] through the chokepoint.
/// Same quiescence + ambient wrapping as [`spawn`], but
/// onto a stored runtime handle rather than the implicit current runtime — for
/// orchestrators (e.g. the downloader run loop) that own their runtime. A raw
/// `handle.spawn(fut)` here would run UNCOUNTED and let the virtual clock race
/// past the orchestrator's event waits, freezing the clock.
#[track_caller]
pub fn spawn_on<F>(handle: &Handle, future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let on = crate::flash::ambient_snapshot();
    let loc = Location::caller();
    let join = Join::new();
    let task = handle.spawn(crate::flash::with_ambient(
        on,
        crate::flash::participate(crate::no_block::watch_blanket_at("spawn", loc, future), loc)
            .joined(Arc::clone(&join)),
    ));
    JoinHandle::new(task, join)
}

/// Spawns blocking work, delegating directly to Tokio outside simulation.
/// Under flash, ambient work reserves `active` before queueing, then claims it
/// `Running` for the closure lifetime. Engine parks release credit; time advances
/// while the closure waits, never while queued or running, so virtual deadlines
/// cannot outrun real work. Non-ambient work remains uncounted.
/// The parent's ambient snapshot is restored for the closure's lifetime because
/// thread-locals do not cross the blocking pool.
/// Completed work keeps its engine credit until its waiting joiner wakes.
#[track_caller]
pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let join = Join::new();
    let task = native_task::spawn_blocking(crate::flash::thread::joined_pool_task(
        f,
        Some(Arc::clone(&join)),
    ));
    JoinHandle::new(task, join)
}

/// Spawn synchronous work without blocking an async runtime worker.
#[track_caller]
pub fn spawn_sync<F, R>(f: F) -> JoinHandle<R>
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
pub fn spawn_blocking_on<F, R>(handle: &Handle, f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let join = Join::new();
    let task = handle.spawn_blocking(crate::flash::thread::joined_pool_task(
        f,
        Some(Arc::clone(&join)),
    ));
    JoinHandle::new(task, join)
}
