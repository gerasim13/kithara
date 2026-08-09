//! The engine lifecycle contract is the same whatever session drives the graph.
//! The caller supplies that session because only a cpal output stream needs
//! hardware, and that decides which suite owns the test.

use kithara::{
    events::EventBus,
    play::{EngineConfig, EngineImpl, PlayError},
};

pub(super) fn start_stop_roundtrip(config: EngineConfig) {
    let engine = EngineImpl::new(config, EventBus::default());
    engine.start().unwrap();
    assert!(engine.is_running());
    engine.stop().unwrap();
    assert!(!engine.is_running());
}

pub(super) fn allocate_and_release_slot(config: EngineConfig) {
    let engine = EngineImpl::new(config, EventBus::default());
    engine.start().unwrap();

    let slot_id = engine.allocate_slot().unwrap();
    assert_eq!(engine.active_slots().len(), 1);
    assert!(engine.active_slots().contains(&slot_id));

    engine.release_slot(slot_id).unwrap();
    assert_eq!(engine.active_slots().len(), 0);

    engine.stop().unwrap();
}

/// The supplied config must set `max_slots` to 1.
pub(super) fn arena_full_error(config: EngineConfig) {
    let engine = EngineImpl::new(config, EventBus::default());
    engine.start().unwrap();

    let _slot = engine.allocate_slot().unwrap();
    let result = engine.allocate_slot();
    assert!(matches!(result, Err(PlayError::ArenaFull)));

    engine.stop().unwrap();
}
