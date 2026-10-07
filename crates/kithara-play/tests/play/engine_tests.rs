use std::num::{NonZeroU32, NonZeroUsize};

use kithara_events::EventBus;
use kithara_play::{EngineConfig, EngineImpl, mock};
use kithara_test_utils::{
    bufpool::{TestPools, pools},
    kithara,
};
use kithara_warp::BeatGridId;

use crate::support::SAMPLE_RATE;

fn response_budget() -> NonZeroUsize {
    NonZeroUsize::new(448).expect("fixture response budget is non-zero")
}

fn make_engine() -> EngineImpl<TestPools> {
    EngineImpl::new(
        EngineConfig::builder()
            .sample_rate(SAMPLE_RATE)
            .grid_id(BeatGridId::allocate().expect("fixture grid id"))
            .session(mock::session_at(SAMPLE_RATE))
            .pools(pools())
            .response_budget_frames(response_budget())
            .build(),
        EventBus::default(),
    )
}

#[kithara::test]
fn engine_config_defaults() {
    let engine = make_engine();
    assert_eq!(engine.master_sample_rate(), 44100);
}

#[kithara::test]
fn engine_config_builder() {
    let config = EngineConfig::builder()
        .grid_id(BeatGridId::allocate().expect("fixture grid id"))
        .sample_rate(NonZeroU32::new(48_000).expect("fixture sample rate is non-zero"))
        .channels(1)
        .eq_layout(kithara_effects::eq::generate_log_spaced_bands(5))
        .pools(pools())
        .response_budget_frames(response_budget())
        .build();
    let engine = EngineImpl::new(config, EventBus::default());
    assert_eq!(engine.master_sample_rate(), 48000);
}

#[kithara::test]
fn an_engine_holds_no_slot_until_its_host_seats_it() {
    assert!(make_engine().slot().is_none());
}

#[kithara::test]
fn engine_subscribe_works() {
    let engine = make_engine();
    let _rx = engine.subscribe::<kithara_play::EngineEvent>();
}

#[kithara::test]
fn engine_master_sample_rate_returns_config_until_a_host_takes_it() {
    let config = EngineConfig::builder()
        .grid_id(BeatGridId::allocate().expect("fixture grid id"))
        .sample_rate(NonZeroU32::new(48_000).expect("fixture sample rate is non-zero"))
        .pools(pools())
        .response_budget_frames(response_budget())
        .build();
    let engine = EngineImpl::new(config, EventBus::default());
    assert_eq!(engine.master_sample_rate(), 48000);
}
