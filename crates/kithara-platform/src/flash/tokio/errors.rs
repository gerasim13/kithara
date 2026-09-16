/// All senders dropped; the watched value will never change again.
#[derive(Debug, Clone, Copy, derive_more::Display, PartialEq, Eq)]
#[display("watch channel closed")]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct RecvError;

/// Returned by `Sender::send` when no receivers remain; carries the value back.
/// Distinct from `tokio`'s (its inner field is private); callers discard it.
#[derive(derive_more::Debug, derive_more::Display)]
#[debug("SendError(..)")]
#[display("sending on a watch channel with no receivers")]
#[derive(derive_more::Error)]
#[error(ignore)]
pub struct SendError<T>(pub T);
