//! A started player holding one slot, for the tests that retire it.
#![cfg(any(not(target_arch = "wasm32"), feature = "offline"))]

use std::num::NonZeroUsize;

use kithara_events::EventBus;
use kithara_test_utils::bufpool::{TestPools, pools};

use super::{
    super::{
        dispatch::run_cmd,
        protocol::{Cmd, PlayerId, Reply, SessionStream},
        state::SessionState,
    },
    graph::attach_player,
};
use crate::api::SlotId;

/// Registers and starts a player on `state` and allocates it one slot.
pub(crate) fn running_slot<T: SessionStream>(
    state: &mut SessionState<T, TestPools>,
) -> (PlayerId, SlotId) {
    let sample_rate = SessionState::<T, TestPools>::DEFAULT_SAMPLE_RATE;
    let grid_id = attach_player(state);
    let player_id = match run_cmd(
        state,
        Cmd::RegisterPlayer {
            grid_id,
            bus: EventBus::default(),
            eq_layout: Vec::new(),
            gate_smoothing: kithara_play::DEFAULT_GATE_SMOOTHING,
            pools: pools(),
            sample_rate,
        },
    ) {
        Reply::PlayerRegistered(registered) => registered.id,
        Reply::Err(error) => panic!("player registration failed: {error}"),
        _ => panic!("player registration returned an unexpected reply"),
    };
    match run_cmd(
        state,
        Cmd::StartPlayer {
            player_id,
            sample_rate,
            render_quantum_frames: None,
            response_budget_frames: NonZeroUsize::new(448),
            master_volume: 1.0,
        },
    ) {
        Reply::Ok => {}
        Reply::Err(error) => panic!("player start failed: {error}"),
        _ => panic!("player start returned an unexpected reply"),
    }
    match run_cmd(state, Cmd::AllocateSlot { player_id }) {
        Reply::SlotAllocated(allocated) => (player_id, allocated.slot),
        Reply::Err(error) => panic!("slot allocation failed: {error}"),
        _ => panic!("slot allocation returned an unexpected reply"),
    }
}
