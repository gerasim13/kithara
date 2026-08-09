//! An injected offline session dispatcher replaces the cpal output stream, so
//! this half asks nothing of the machine and belongs in the ordinary gate
//! rather than a lane.

use kithara::{bufpool::PcmPool, play::EngineConfig};
use kithara_integration_tests::offline::OfflineSession;

use super::engine_session_contract as contract;

fn engine_config(max_slots: usize) -> EngineConfig {
    EngineConfig::builder()
        .max_slots(max_slots)
        .pcm_pool(PcmPool::default())
        .session(OfflineSession::arc_auto())
        .build()
}

#[kithara::test]
fn engine_start_stop_roundtrip() {
    contract::start_stop_roundtrip(engine_config(4));
}

#[kithara::test]
fn engine_allocate_and_release_slot() {
    contract::allocate_and_release_slot(engine_config(4));
}

#[kithara::test]
fn engine_arena_full_error() {
    contract::arena_full_error(engine_config(1));
}
