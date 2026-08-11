#![forbid(unsafe_code)]

mod cleanup;
mod epoch;
mod handle;
mod lease;
mod session;
mod state;

pub use cleanup::PendingResourceCleanupError;
pub(crate) use cleanup::RemoveResource;
pub use epoch::{WriterEpoch, WriterOutcome};
pub use handle::WriterHandle;
pub use lease::{ResourceAttachment, ResourceLease};
pub(in crate::index) use session::PendingResourceSession;
use session::WriterIdentity;
pub(in crate::index) use state::WriterClaim;
pub(crate) use state::{PendingResource, SessionPhase};
