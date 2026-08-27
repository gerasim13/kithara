//! Lower player-to-host session protocol.

pub mod protocol;
#[cfg(test)]
pub(crate) mod testing;
pub use protocol::{
    AllocatedSlot, Cmd, PlayerId, PlayerLevel, Reply, SessionBinding, SessionDispatcher,
    SessionError, SessionHandle, SessionSampleRate,
};
