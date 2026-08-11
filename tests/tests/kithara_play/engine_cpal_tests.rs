//! Leaving `EngineConfig::session` unset makes this the hardware half. The
//! engine falls through to `default_session_handle`, which opens a cpal output
//! stream, so these tests run only where a real output device exists.

use kithara::{bufpool::PcmPool, play::EngineConfig};

use super::engine_session_contract as contract;

fn engine_config(max_slots: usize) -> EngineConfig {
    EngineConfig::builder()
        .max_slots(max_slots)
        .pcm_pool(PcmPool::default())
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
