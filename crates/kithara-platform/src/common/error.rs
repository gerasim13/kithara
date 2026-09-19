/// `try_lock()` failed because the mutex is already held.
#[derive(Debug, Clone, Copy, derive_more::Display, PartialEq, Eq)]
#[display("mutex is already locked")]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct NotAvailable;
