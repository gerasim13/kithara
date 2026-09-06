#![forbid(unsafe_code)]

use std::{fmt, task::Waker};

use dashmap::mapref::entry::Entry;
use kithara_bufpool::HasPool;
use kithara_platform::{CancelToken, sync::Arc};

use super::{PendingResourceSession, SessionPhase, WriterClaim, WriterHandle};
use crate::{
    index::pending::DemandEntry,
    store::{AssetReader, AssetStore},
};

/// RAII attachment for one consumer.
#[non_exhaustive]
pub struct ResourceLease<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(in crate::index) entry: Arc<DemandEntry>,
    pub(in crate::index) session: Arc<PendingResourceSession<S>>,
    pub(in crate::index) _store: AssetStore<S>,
}

impl<S> ResourceLease<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(in crate::index) const fn new(
        entry: Arc<DemandEntry>,
        session: Arc<PendingResourceSession<S>>,
        store: AssetStore<S>,
    ) -> Self {
        Self {
            entry,
            session,
            _store: store,
        }
    }

    /// Wake the writer so it re-reads aggregate demand.
    pub fn note_progress(&self) {
        let state = self.session.slot.state.lock();
        let waker = state.current_peer_waker();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// Extend this consumer's immediate byte demand and wake the writer.
    #[doc(hidden)]
    pub fn request_until(&self, end: u64) {
        if self.entry.request_until(end) {
            self.note_progress();
        }
    }

    /// Cancellation scope for this exact pending resource.
    #[doc(hidden)]
    #[must_use]
    pub fn session_cancel(&self) -> CancelToken {
        self.session.slot.writer_cancel.clone()
    }

    /// Take the writer role when no current writer exists.
    #[must_use]
    pub fn try_take_writer(&self) -> Option<WriterHandle<S>> {
        let Entry::Occupied(occupied) = self.session.inner.slots.entry(self.session.key.clone())
        else {
            return None;
        };
        if !Arc::ptr_eq(occupied.get(), &self.session.slot) {
            return None;
        }
        let mut state = self.session.slot.state.lock();
        if !state
            .entries
            .iter()
            .any(|entry| Arc::ptr_eq(entry, &self.entry))
        {
            return None;
        }
        let claim = self.session.slot.elect_writer(&mut state, &self.entry)?;
        drop(state);
        Some(self.writer(claim))
    }

    /// Consume and wake this consumer's registered peer.
    pub fn wake_peer(&self) {
        if let Some(waker) = self.entry.take_peer_waker() {
            waker.wake();
        }
    }

    pub(in crate::index) fn writer(&self, claim: Arc<WriterClaim>) -> WriterHandle<S> {
        WriterHandle::new(claim, &self.session)
    }

    delegate::delegate! {
        to self.entry {
            /// Register a one-shot synchronous reader wake capability.
            ///
            /// Register before checking byte readiness. A write or terminal transition
            /// consumes the registration, so the reader must rearm after every wake.
            pub fn register_reader_waker(&self, waker: &Waker);
            /// Register a one-shot peer-poll wake capability.
            ///
            /// Arm on every poll before the readiness or election check that may
            /// return `Pending`. Demand or writer handoff consumes the registration,
            /// so the next poll must rearm it.
            pub fn register_peer_waker(&self, waker: &Waker);
            /// Clear this exact peer registration after a ready check.
            ///
            /// After an armed recheck confirms readiness, clear the same waker
            /// immediately before returning `Ready`. Leave it armed when the recheck
            /// returns `Pending`. Clearing an older waker never removes a newer
            /// registration.
            #[doc(hidden)]
            pub fn clear_peer_waker(&self, waker: &Waker);
        }
    }
}

impl<S> Drop for ResourceLease<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn drop(&mut self) {
        let entry = self.session.inner.slots.entry(self.session.key.clone());
        let exact = matches!(
            &entry,
            Entry::Occupied(occupied) if Arc::ptr_eq(occupied.get(), &self.session.slot)
        );
        let mut state = self.session.slot.state.lock();
        state
            .entries
            .retain(|entry| !Arc::ptr_eq(entry, &self.entry));
        let was_writer = state
            .writer_claim
            .as_ref()
            .is_some_and(|claim| claim.belongs_to(&self.entry));
        if was_writer {
            state.writer_claim = None;
        }
        if matches!(&state.phase, SessionPhase::CleanupFailed(_)) {
            return;
        }
        if !exact || !state.entries.is_empty() {
            let peer_wakers = if was_writer {
                state.peer_wakers()
            } else {
                state.current_peer_waker().into_iter().collect()
            };
            drop(state);
            drop(entry);
            for waker in peer_wakers {
                waker.wake();
            }
            return;
        }

        state.phase = SessionPhase::Finishing;
        let writer = state.writer.take();
        let reader = state.reader.take();
        let wakers = state.terminal_wakers();
        drop(state);

        drop(writer);
        drop(reader);
        match (self.session.slot.remove)(&self.session.key) {
            Ok(()) => {
                if let Entry::Occupied(occupied) = entry {
                    occupied.remove();
                }
            }
            Err(source) => {
                let error = self
                    .session
                    .slot
                    .record_cleanup_failure(&self.session.key, source);
                drop(entry);
                tracing::warn!(
                    %error,
                    key = ?self.session.key,
                    "pending resource cleanup failed"
                );
            }
        }
        self.session.slot.writer_cancel.cancel();
        for waker in wakers {
            waker.wake();
        }
    }
}

impl<S> fmt::Debug for ResourceLease<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResourceLease")
            .field("key", &self.session.key)
            .finish_non_exhaustive()
    }
}

/// One consumer's attachment to an active pending resource acquisition.
#[derive(Debug)]
#[doc(hidden)]
pub struct ResourceAttachment<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(in crate::index) reader: AssetReader<S>,
    pub(in crate::index) writer: Option<WriterHandle<S>>,
    pub(in crate::index) lease: ResourceLease<S>,
}

impl<S> From<ResourceAttachment<S>> for (AssetReader<S>, ResourceLease<S>, Option<WriterHandle<S>>)
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    fn from(attachment: ResourceAttachment<S>) -> Self {
        (attachment.reader, attachment.lease, attachment.writer)
    }
}
