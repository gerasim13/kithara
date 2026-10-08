#![forbid(unsafe_code)]

use tracing::warn;

use super::{sealed, state::ResourceCore};
use crate::backend::traits::DriverIo;

/// Lifecycle phase of a [`Resource`](super::handle::Resource). Sealed: only the in-crate markers
/// [`Active`], [`Committed`] and [`Reader`] implement it.
pub trait ResourcePhase: sealed::Sealed {
    /// The per-phase payload stored inside `Resource<Self, D>`.
    type Data<D: DriverIo>;
}

/// Writable, in-flight phase: single-owner, not `Clone`.
#[derive(kithara_derive::Phase)]
#[phase(
    trait = ResourcePhase,
    sealed = sealed::Sealed,
    data = WriteGuard<D>,
    generic = D,
    bound = D: DriverIo,
    gat
)]
pub struct Active;
/// Sealed, fully-written phase: read-final, reactivatable.
#[derive(kithara_derive::Phase)]
#[phase(
    trait = ResourcePhase,
    sealed = sealed::Sealed,
    data = ReadCore<D>,
    generic = D,
    bound = D: DriverIo,
    gat
)]
pub struct Committed;
/// Cloneable read-only view minted from an `Active` or `Committed` handle.
#[derive(kithara_derive::Phase)]
#[phase(
    trait = ResourcePhase,
    sealed = sealed::Sealed,
    data = ReadCore<D>,
    generic = D,
    bound = D: DriverIo,
    gat
)]
pub struct Reader;

/// Drop-guard payload for the `Active` writer phase. Owns the shared
/// `ResourceCore` and, if dropped without a successful `commit`/`fail`, marks
/// the core failed and wakes blocked readers. The `Drop` lives on this concrete
/// payload (full-generic match) so the phantom `Resource<S, D>` needs no
/// (illegal) specialized `Drop` impl.
pub struct WriteGuard<D: DriverIo> {
    pub(super) core: ResourceCore<D>,
}

impl<D: DriverIo> Drop for WriteGuard<D> {
    fn drop(&mut self) {
        if self.core.should_fail_on_drop() {
            if let Some(path) = self.core.path_inner() {
                warn!(resource = ?path, "active resource writer dropped without commit");
            }
            self.core
                .fail_inner("active resource writer dropped without commit".to_owned());
        }
    }
}

/// Read-only payload shared by the `Committed` and `Reader` phases.
#[derive_where::derive_where(Clone; D: DriverIo)]
pub struct ReadCore<D: DriverIo> {
    pub(super) core: ResourceCore<D>,
}
