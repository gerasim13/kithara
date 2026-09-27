//! Fixed-order atomic values for independent scalar state.

mod order;
pub use order::{Acquire, ReadOrder, Relaxed, Release, SeqCst, WriteOrder};

mod value;
pub use value::{AtomicPrimitive, AtomicValue, RelaxedAtomicBool, RelaxedAtomicF32};
