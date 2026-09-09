//! Lower player-to-host session protocol.

pub mod protocol;
pub use protocol::{
    AllocatedSlot, Cmd, PlayerId, PlayerLevel, Reply, SessionBinding, SessionDispatcher,
    SessionError, SessionHandle, SessionSampleRate,
};
