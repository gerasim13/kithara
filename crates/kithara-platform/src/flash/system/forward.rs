use std::{panic::Location, sync::atomic::AtomicBool, task::Waker};

use super::{
    CvId, FLASH, WaiterId,
    credit::WaitGuard,
    gate::TaskGate,
    sched::{AsyncHandle, WakeBatch},
    wake::Token,
};
use crate::{
    flash::{diag, ids::ThreadKey},
    sync::Arc,
};

/// Process-engine forward of [`FlashInner::next_condvar_id`](super::FlashInner::next_condvar_id).
pub(crate) fn next_condvar_id() -> CvId {
    FLASH.next_condvar_id()
}

/// Process-engine forward of [`FlashInner::describe_cvid`](super::FlashInner::describe_cvid).
pub(crate) fn describe_cvid(cvid: CvId, kind: diag::PrimKind, loc: &'static Location<'static>) {
    FLASH.describe_cvid(cvid, kind, loc);
}

/// Process-engine forward of [`FlashInner::park_for`](super::FlashInner::park_for).
#[cfg(test)]
pub(crate) fn park_for(d: crate::flash::Duration) {
    FLASH.park_for(d);
}

/// Process-engine forward of
/// [`FlashInner::park_timed_unparkable`](super::FlashInner::park_timed_unparkable).
pub(crate) fn park_timed_unparkable(d: crate::flash::Duration, thread_id: ThreadKey) {
    FLASH.park_timed_unparkable(d, thread_id);
}

/// Process-engine forward of [`FlashInner::sleep_timed`](super::FlashInner::sleep_timed).
pub(crate) fn sleep_timed(d: crate::flash::Duration) {
    FLASH.sleep_timed(d);
}

/// Process-engine forward of [`FlashInner::unpark`](super::FlashInner::unpark).
pub(crate) fn unpark(thread_id: ThreadKey) {
    FLASH.unpark(thread_id);
}

/// Process-engine forward of
/// [`FlashInner::yield_until_advance`](super::FlashInner::yield_until_advance).
pub(crate) fn yield_until_advance() {
    FLASH.yield_until_advance();
}

/// Process-engine forward of
/// [`FlashInner::register_yield_async`](super::FlashInner::register_yield_async).
pub(crate) fn register_yield_async(waker: Waker) -> (WaiterId, Arc<AtomicBool>, WakeBatch) {
    FLASH.register_yield_async(waker)
}

/// Process-engine forward of [`FlashInner::cancel_yield`](super::FlashInner::cancel_yield).
pub(crate) fn cancel_yield(id: WaiterId) {
    FLASH.cancel_yield(id);
}

/// Process-engine forward of
/// [`FlashInner::register_condvar_timed`](super::FlashInner::register_condvar_timed).
pub(crate) fn register_condvar_timed(
    deadline_nanos: u64,
    cvid: CvId,
) -> (Arc<Token>, WakeBatch, WaitGuard<'static>) {
    FLASH.register_condvar_timed(deadline_nanos, cvid)
}

/// Process-engine forward of
/// [`FlashInner::register_condvar_untimed`](super::FlashInner::register_condvar_untimed).
pub(crate) fn register_condvar_untimed(cvid: CvId) -> (Arc<Token>, WakeBatch, WaitGuard<'static>) {
    FLASH.register_condvar_untimed(cvid)
}

/// Process-engine forward of [`FlashInner::signal_condvar`](super::FlashInner::signal_condvar).
pub(crate) fn signal_condvar(cvid: CvId, all: bool) {
    FLASH.signal_condvar(cvid, all);
}

/// Process-engine forward of
/// [`FlashInner::register_sleep_async`](super::FlashInner::register_sleep_async).
pub(crate) fn register_sleep_async(delta_nanos: u64, waker: Waker) -> (AsyncHandle, WakeBatch) {
    FLASH.register_sleep_async(delta_nanos, waker)
}

/// Process-engine forward of
/// [`FlashInner::register_notify_async`](super::FlashInner::register_notify_async).
pub(crate) fn register_notify_async(cvid: CvId, waker: Waker) -> (Option<AsyncHandle>, WakeBatch) {
    FLASH.register_notify_async(cvid, waker)
}

/// Process-engine forward of [`FlashInner::async_acquire`](super::FlashInner::async_acquire).
pub(crate) fn async_acquire(loc: &'static Location<'static>) -> Arc<TaskGate> {
    FLASH.async_acquire(loc)
}

/// Process-engine forward of
/// [`FlashInner::cancel_async_wait`](super::FlashInner::cancel_async_wait).
pub(crate) fn cancel_async_wait(handle: &AsyncHandle) {
    FLASH.cancel_async_wait(handle);
}

/// Process-engine forward of [`FlashInner::signal_notify`](super::FlashInner::signal_notify).
pub(crate) fn signal_notify(cvid: CvId) {
    FLASH.signal_notify(cvid);
}

/// Process-engine forward of
/// [`FlashInner::register_channel_async`](super::FlashInner::register_channel_async).
pub(crate) fn register_channel_async(cvid: CvId, waker: Waker) -> (AsyncHandle, WakeBatch) {
    FLASH.register_channel_async(cvid, waker)
}

/// Process-engine forward of [`FlashInner::signal_channel`](super::FlashInner::signal_channel).
pub(crate) fn signal_channel(cvid: CvId, all: bool) {
    FLASH.signal_channel(cvid, all);
}

/// Process-engine diagnostic dump of [`FlashInner`](super::FlashInner) via its
/// `Display` impl.
pub(crate) fn dump() -> String {
    FLASH.to_string()
}

/// Process-engine forward of
/// [`FlashInner::async_active_count`](super::FlashInner::async_active_count).
#[cfg(test)]
pub(crate) fn async_active_count() -> usize {
    FLASH.async_active_count()
}

/// Process-engine forward of
/// [`FlashInner::diag_yield_count`](super::FlashInner::diag_yield_count).
#[cfg(test)]
pub(crate) fn diag_yield_count() -> usize {
    FLASH.diag_yield_count()
}
