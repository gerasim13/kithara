use super::{engine::Backend, state};

/// The phase machine bound to this build's packager: the one in `live.rs` when
/// the `broadcast` feature is on, the one in `off.rs` when it is not. The
/// second has no stream values, so the on-air phase is unconstructable there.
pub(crate) type Broadcaster = state::Broadcaster<Backend>;
pub(crate) type BroadcastStop = state::BroadcastStop<Backend>;
