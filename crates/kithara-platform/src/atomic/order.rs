use core::sync::atomic::Ordering;

mod sealed {
    pub trait Sealed {}

    impl Sealed for super::Relaxed {}
    impl Sealed for super::Acquire {}
    impl Sealed for super::Release {}
    impl Sealed for super::SeqCst {}
}

/// Permitted ordering for an atomic load.
pub trait ReadOrder: sealed::Sealed {
    /// Ordering used by `AtomicValue::load`.
    const ORDERING: Ordering;
}

/// Permitted ordering for an atomic store.
pub trait WriteOrder: sealed::Sealed {
    /// Ordering used by `AtomicValue::store`.
    const ORDERING: Ordering;
}

/// Relaxed load or store ordering.
pub struct Relaxed;
/// Acquire load ordering.
pub struct Acquire;
/// Release store ordering.
pub struct Release;
/// Sequentially consistent load or store ordering.
pub struct SeqCst;

impl ReadOrder for Relaxed {
    const ORDERING: Ordering = Ordering::Relaxed;
}

impl WriteOrder for Relaxed {
    const ORDERING: Ordering = Ordering::Relaxed;
}

impl ReadOrder for Acquire {
    const ORDERING: Ordering = Ordering::Acquire;
}

impl WriteOrder for Release {
    const ORDERING: Ordering = Ordering::Release;
}

impl ReadOrder for SeqCst {
    const ORDERING: Ordering = Ordering::SeqCst;
}

impl WriteOrder for SeqCst {
    const ORDERING: Ordering = Ordering::SeqCst;
}
