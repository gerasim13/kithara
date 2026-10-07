//! The engine lifecycle contract is the same whatever session drives the graph.
//! The caller supplies an `EngineImpl`; each fixture decides which session and
//! backend it uses, and therefore which suite owns the test.
use kithara_play::EngineImpl;
use kithara_test_utils::bufpool::TestPools;

pub(super) fn start_stop_roundtrip(engine: &EngineImpl<TestPools>) {
    engine.start().unwrap();
    assert!(engine.is_running());
    engine.stop().unwrap();
    assert!(!engine.is_running());
}

/// A deck holds its one slot from start to stop.
pub(super) fn holds_its_slot_while_running(engine: &EngineImpl<TestPools>) {
    assert_eq!(engine.slot(), None);

    engine.start().unwrap();
    assert!(engine.slot().is_some());

    engine.stop().unwrap();
    assert_eq!(engine.slot(), None);
}
