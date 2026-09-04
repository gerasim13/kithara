#![forbid(unsafe_code)]

use std::fmt;

use dashmap::mapref::entry::Entry;
use kithara_bufpool::HasPool;
use kithara_platform::sync::Arc;
use kithara_storage::StorageResult;

use super::{SessionPhase, WriterIdentity};
use crate::{
    error::{AssetsError, AssetsResult},
    resource::WriteSide,
};

/// Result of an epoch-scoped writer operation.
#[derive(Debug)]
#[doc(hidden)]
#[must_use]
#[non_exhaustive]
pub enum WriterOutcome<T> {
    /// The epoch was current and the operation ran.
    Current(T),
    /// The session has moved to another writer or generation.
    Stale,
}

impl<T> WriterOutcome<T> {
    /// Return the operation result only while its writer epoch was current.
    #[must_use]
    pub fn current(self) -> Option<T> {
        self.into()
    }
}

impl<T> From<WriterOutcome<T>> for Option<T> {
    fn from(outcome: WriterOutcome<T>) -> Self {
        match outcome {
            WriterOutcome::Current(value) => Some(value),
            WriterOutcome::Stale => None,
        }
    }
}

/// Cloneable capability used by one writer epoch's callbacks.
#[doc(hidden)]
pub struct WriterEpoch<S> {
    identity: Arc<WriterIdentity<S>>,
}

impl<S> Clone for WriterEpoch<S> {
    fn clone(&self) -> Self {
        Self {
            identity: Arc::clone(&self.identity),
        }
    }
}

impl<S> WriterEpoch<S>
where
    S: HasPool<u8> + Send + Sync + 'static,
{
    pub(super) const fn new(identity: Arc<WriterIdentity<S>>) -> Self {
        Self { identity }
    }

    /// Commit the canonical writer when this epoch is current.
    ///
    /// # Panics
    ///
    /// Panics if the active session has lost its canonical writer. That state
    /// violates the pending-resource acquisition ownership invariant.
    pub fn commit(&self, final_len: Option<u64>) -> WriterOutcome<AssetsResult<()>> {
        let session = &self.identity.session;
        let Entry::Occupied(occupied) = session.inner.slots.entry(session.key.clone()) else {
            return WriterOutcome::Stale;
        };
        if !Arc::ptr_eq(occupied.get(), &session.slot) {
            return WriterOutcome::Stale;
        }

        let mut state = session.slot.state.lock();
        if !state.is_current_writer(&self.identity.claim) {
            return WriterOutcome::Stale;
        }
        state.phase = SessionPhase::Finishing;
        state.writer_claim = None;
        let Some(writer) = state.writer.take() else {
            panic!("BUG: active pending resource lost its writer");
        };
        drop(state);

        match writer.commit(final_len) {
            Ok(reader) => {
                let (old_reader, wakers) = {
                    let mut state = session.slot.state.lock();
                    let old_reader = state.reader.replace(reader);
                    state.phase = SessionPhase::Committed;
                    (old_reader, state.terminal_wakers())
                };
                drop(old_reader);
                occupied.remove();
                session.slot.writer_cancel.cancel();
                for waker in wakers {
                    waker.wake();
                }
                WriterOutcome::Current(Ok(()))
            }
            Err(error) => {
                let primary = AssetsError::from(error);
                let (old_reader, wakers) = {
                    let mut state = session.slot.state.lock();
                    (state.reader.take(), state.terminal_wakers())
                };
                drop(old_reader);
                match (session.slot.remove)(&session.key) {
                    Ok(()) => {
                        occupied.remove();
                    }
                    Err(source) => {
                        let cleanup = session.slot.record_cleanup_failure(&session.key, source);
                        drop(occupied);
                        tracing::warn!(
                            error = %cleanup,
                            primary = %primary,
                            key = ?session.key,
                            "pending resource cleanup failed after commit error"
                        );
                    }
                }
                session.slot.writer_cancel.cancel();
                for waker in wakers {
                    waker.wake();
                }
                WriterOutcome::Current(Err(primary))
            }
        }
    }

    fn current_entry(&self) -> bool {
        let session = &self.identity.session;
        session
            .inner
            .slots
            .get(&session.key)
            .is_some_and(|slot| Arc::ptr_eq(slot.value(), &session.slot))
    }

    /// Fail and remove the canonical writer when this epoch is current.
    pub fn fail(&self, reason: String) -> WriterOutcome<AssetsResult<()>> {
        self.retire(reason)
    }

    /// Whether this epoch still owns the active writer role.
    #[must_use]
    pub fn is_current(&self) -> bool {
        if !self.current_entry() {
            return false;
        }
        let state = self.identity.session.slot.state.lock();
        state.is_current_writer(&self.identity.claim)
    }

    /// Relinquish writer ownership without abandoning the shared writer.
    pub fn relinquish(&self) -> WriterOutcome<()> {
        let session = &self.identity.session;
        let Entry::Occupied(occupied) = session.inner.slots.entry(session.key.clone()) else {
            return WriterOutcome::Stale;
        };
        if !Arc::ptr_eq(occupied.get(), &session.slot) {
            return WriterOutcome::Stale;
        }
        let mut state = session.slot.state.lock();
        if !state.is_current_writer(&self.identity.claim) {
            return WriterOutcome::Stale;
        }
        state.writer_claim = None;
        let wakers = state.peer_wakers();
        drop(state);
        drop(occupied);
        for waker in wakers {
            waker.wake();
        }
        WriterOutcome::Current(())
    }

    fn retire(&self, reason: String) -> WriterOutcome<AssetsResult<()>> {
        let session = &self.identity.session;
        let Entry::Occupied(occupied) = session.inner.slots.entry(session.key.clone()) else {
            return WriterOutcome::Stale;
        };
        if !Arc::ptr_eq(occupied.get(), &session.slot) {
            return WriterOutcome::Stale;
        }
        let mut state = session.slot.state.lock();
        if !state.is_current_writer(&self.identity.claim) {
            return WriterOutcome::Stale;
        }
        state.phase = SessionPhase::Finishing;
        state.writer_claim = None;
        let Some(writer) = state.writer.take() else {
            panic!("BUG: active pending resource lost its writer");
        };
        let reader = state.reader.take();
        let wakers = state.terminal_wakers();
        drop(state);

        writer.fail(reason);
        drop(reader);
        let cleanup = match (session.slot.remove)(&session.key) {
            Ok(()) => {
                occupied.remove();
                Ok(())
            }
            Err(source) => {
                let error = session.slot.record_cleanup_failure(&session.key, source);
                drop(occupied);
                Err(error)
            }
        };
        session.slot.writer_cancel.cancel();
        for waker in wakers {
            waker.wake();
        }
        WriterOutcome::Current(cleanup)
    }

    /// Write through the canonical writer when this epoch is current.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> WriterOutcome<StorageResult<()>> {
        if !self.current_entry() {
            return WriterOutcome::Stale;
        }
        let state = self.identity.session.slot.state.lock();
        if !state.is_current_writer(&self.identity.claim) {
            return WriterOutcome::Stale;
        }
        let Some(writer) = state.writer.as_ref() else {
            return WriterOutcome::Stale;
        };
        let result = writer.write_at(offset, data);
        let wakers = result
            .as_ref()
            .ok()
            .map(|()| state.reader_wakers())
            .unwrap_or_default();
        drop(state);
        for waker in wakers {
            waker.wake();
        }
        WriterOutcome::Current(result)
    }
}

impl<S> fmt::Debug for WriterEpoch<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriterEpoch")
            .field("key", &self.identity.session.key)
            .finish_non_exhaustive()
    }
}
