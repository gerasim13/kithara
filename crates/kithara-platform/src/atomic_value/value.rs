use core::{fmt, marker::PhantomData, sync::atomic::Ordering};

#[cfg(not(all(feature = "loom", not(target_arch = "wasm32"))))]
use portable_atomic::{AtomicBool, AtomicU32};

use super::order::{ReadOrder, Relaxed, WriteOrder};
#[cfg(all(feature = "loom", not(target_arch = "wasm32")))]
use crate::sync::atomic::{AtomicBool, AtomicU32};

mod sealed {
    pub trait Sealed {}

    impl Sealed for bool {}
    impl Sealed for f32 {}
}

/// A primitive with an atomic storage representation.
pub trait AtomicPrimitive: Copy + sealed::Sealed {
    /// Backend-selected atomic storage.
    type Atomic;

    /// Loads a primitive value from the backing atomic.
    fn atomic_load(atomic: &Self::Atomic, order: Ordering) -> Self;

    /// Creates the backing atomic.
    fn atomic_new(value: Self) -> Self::Atomic;

    /// Stores a primitive value in the backing atomic.
    fn atomic_store(atomic: &Self::Atomic, value: Self, order: Ordering);
}

impl AtomicPrimitive for bool {
    type Atomic = AtomicBool;

    fn atomic_load(atomic: &Self::Atomic, order: Ordering) -> Self {
        atomic.load(order)
    }

    fn atomic_new(value: Self) -> Self::Atomic {
        AtomicBool::new(value)
    }

    fn atomic_store(atomic: &Self::Atomic, value: Self, order: Ordering) {
        atomic.store(value, order);
    }
}

impl AtomicPrimitive for f32 {
    type Atomic = AtomicU32;

    fn atomic_load(atomic: &Self::Atomic, order: Ordering) -> Self {
        Self::from_bits(atomic.load(order))
    }

    fn atomic_new(value: Self) -> Self::Atomic {
        AtomicU32::new(value.to_bits())
    }

    fn atomic_store(atomic: &Self::Atomic, value: Self, order: Ordering) {
        atomic.store(value.to_bits(), order);
    }
}

/// An atomic scalar with load and store orderings fixed by its type.
/// Cloning creates an independent snapshot, not another handle to the same atomic.
pub struct AtomicValue<T: AtomicPrimitive, R: ReadOrder, W: WriteOrder> {
    atomic: T::Atomic,
    _orders: PhantomData<(R, W)>,
}

impl<T: AtomicPrimitive, R: ReadOrder, W: WriteOrder> AtomicValue<T, R, W> {
    fn construct(value: T) -> Self {
        Self {
            atomic: T::atomic_new(value),
            _orders: PhantomData,
        }
    }

    /// Loads the current value with the type's read ordering.
    #[must_use]
    pub fn load(&self) -> T {
        T::atomic_load(&self.atomic, R::ORDERING)
    }

    /// Stores a value with the type's write ordering.
    pub fn store(&self, value: T) {
        T::atomic_store(&self.atomic, value, W::ORDERING);
    }
}

#[cfg(not(all(feature = "loom", not(target_arch = "wasm32"))))]
impl<R: ReadOrder, W: WriteOrder> AtomicValue<bool, R, W> {
    /// Creates a boolean atomic value.
    #[must_use]
    pub const fn new(value: bool) -> Self {
        Self {
            atomic: AtomicBool::new(value),
            _orders: PhantomData,
        }
    }
}

#[cfg(all(feature = "loom", not(target_arch = "wasm32")))]
impl<R: ReadOrder, W: WriteOrder> AtomicValue<bool, R, W> {
    /// Creates a boolean atomic value inside a Loom model.
    #[must_use]
    pub fn new(value: bool) -> Self {
        Self::construct(value)
    }
}

#[cfg(not(all(feature = "loom", not(target_arch = "wasm32"))))]
impl<R: ReadOrder, W: WriteOrder> AtomicValue<f32, R, W> {
    /// Creates a floating-point atomic value.
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self {
            atomic: AtomicU32::new(value.to_bits()),
            _orders: PhantomData,
        }
    }
}

#[cfg(all(feature = "loom", not(target_arch = "wasm32")))]
impl<R: ReadOrder, W: WriteOrder> AtomicValue<f32, R, W> {
    /// Creates a floating-point atomic value inside a Loom model.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self::construct(value)
    }
}

impl<T: AtomicPrimitive, R: ReadOrder, W: WriteOrder> Clone for AtomicValue<T, R, W> {
    fn clone(&self) -> Self {
        Self::construct(self.load())
    }
}

impl<T: AtomicPrimitive + fmt::Debug, R: ReadOrder, W: WriteOrder> fmt::Debug
    for AtomicValue<T, R, W>
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("AtomicValue")
            .field(&self.load())
            .finish()
    }
}

/// Boolean atomic value using relaxed loads and stores.
pub type RelaxedAtomicBool = AtomicValue<bool, Relaxed, Relaxed>;
/// Floating-point atomic value using relaxed loads and stores.
pub type RelaxedAtomicF32 = AtomicValue<f32, Relaxed, Relaxed>;

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    #[cfg(all(feature = "loom", not(target_arch = "wasm32")))]
    use super::super::{Acquire, Release};
    use super::*;
    #[cfg(all(feature = "loom", not(target_arch = "wasm32")))]
    use crate::{sync::Arc, thread};

    #[cfg(not(feature = "loom"))]
    #[kithara::test(native, flash(false))]
    fn float_bits_and_clone_snapshot_are_preserved() {
        let value = RelaxedAtomicF32::new(f32::from_bits(0x7fc0_1234));
        let snapshot = value.clone();
        value.store(-0.0);
        assert_eq!(value.load().to_bits(), (-0.0_f32).to_bits());
        assert_eq!(snapshot.load().to_bits(), 0x7fc0_1234);
    }

    #[cfg(not(feature = "loom"))]
    #[kithara::test(native, flash(false))]
    fn bool_clone_is_independent() {
        let value = RelaxedAtomicBool::new(false);
        let snapshot = value.clone();
        value.store(true);
        assert!(value.load());
        assert!(!snapshot.load());
    }

    #[cfg(all(feature = "loom", not(target_arch = "wasm32")))]
    #[kithara::test(native, loom, flash(false))]
    fn loom_models_bool_publication_and_float_value() {
        let value = Arc::new(RelaxedAtomicF32::new(0.0));
        let ready = Arc::new(AtomicValue::<bool, Acquire, Release>::new(false));
        let writer_value = Arc::clone(&value);
        let writer_ready = Arc::clone(&ready);
        let writer = thread::spawn(move || {
            writer_value.store(f32::from_bits(0x7fc0_1234));
            writer_ready.store(true);
        });

        if ready.load() {
            assert_eq!(value.load().to_bits(), 0x7fc0_1234);
        }
        writer.join().expect("model writer did not panic");
        assert!(ready.load());
        assert_eq!(value.load().to_bits(), 0x7fc0_1234);
        let snapshot = value.as_ref().clone();
        value.store(-0.0);
        assert_eq!(snapshot.load().to_bits(), 0x7fc0_1234);
    }
}
