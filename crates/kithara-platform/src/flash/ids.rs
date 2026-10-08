use std::hash::Hash;

pub(in crate::flash) use super::system::CvId;
use crate::common::thread_id::thread_id_hash;

/// Park/wake backend latched from `flash_ambient()` at primitive construction.
/// All waits and signals retain this choice, including notifications from raw
/// threads without inherited ambient. Per-call selection would silently lose
/// wakes between an engine waiter and a native notifier, or vice versa.
/// Engine primitives carry their [`CvId`]; native ones mint none. Bounded mpsc
/// carries the corresponding pair-form backend locally.
#[derive(Clone, Copy, Debug)]
pub(in crate::flash) enum Backend {
    /// Engine-backed: the creating context was flash-eligible (ambient).
    Engine(CvId),
    /// Real OS mechanism (the default-real path).
    Native,
}

/// Diagnostic for the [`Backend::Native`] arms: a wait/signal reaching a
/// NATIVE-latched primitive from a flash-ambient thread means the primitive
/// was created BEFORE `ambient_scope` took effect, so it is invisible to the
/// quiescence engine — a class of stalls that is otherwise undiagnosable
/// (the test wedges on a real wait the virtual clock knows nothing about).
/// The reverse direction (Engine-latched primitive signaled from a
/// non-ambient thread) is by design (see [`Backend`]) and stays silent.
/// `debug`, not `warn`: legitimate real carve-outs in `flash(false)` tests
/// land here too and must not scream.
#[inline]
pub(in crate::flash) fn trace_native_from_ambient(primitive: &'static str, op: &'static str) {
    if crate::flash::flash_ambient() {
        tracing::debug!(
            primitive,
            op,
            "native-latched primitive used from a flash-ambient thread \
             (created before ambient_scope; invisible to the engine)"
        );
    }
}

/// Hashed [`ThreadId`] key targeting a parked thread.
/// Park and wake paths must derive it through the same `thread_id_hash`.
#[derive(Clone, Copy, Debug, derive_more::From, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::flash) struct ThreadKey(u64);

impl ThreadKey {
    pub(in crate::flash) fn of<I: Hash>(id: I) -> Self {
        Self(thread_id_hash(id))
    }

    /// The raw hash, for keeping a key in an atomic cell (a task's driver
    /// thread, written lock-free at poll entry). Round-trips through `From`.
    pub(in crate::flash) fn raw(self) -> u64 {
        self.0
    }
}
