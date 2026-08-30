use std::sync::atomic::{AtomicU64, Ordering};

use kithara_audio::ConsumerWakeMode;
use kithara_platform::sync::{Arc, Mutex};

use super::{AllocatedSlot, Cmd, Reply, SessionDispatcher, SessionSampleRate};
use crate::{
    PlayError, SessionDuckingMode, SharedEq, SlotId,
    bridge::{NodeInputs, slot_channels},
};

struct TestSession {
    next_player: AtomicU64,
    next_slot: AtomicU64,
    nodes: Mutex<Vec<NodeInputs>>,
}

impl SessionDispatcher for TestSession {
    fn consumer_wake_mode(&self) -> ConsumerWakeMode {
        ConsumerWakeMode::RealtimeDeferred
    }

    fn exec(&self, cmd: Cmd) -> Result<Reply, PlayError> {
        let reply = match cmd {
            Cmd::RegisterPlayer { .. } => {
                Reply::PlayerRegistered(self.next_player.fetch_add(1, Ordering::Relaxed))
            }
            Cmd::AllocateSlot { .. } => {
                let slot = SlotId::new(self.next_slot.fetch_add(1, Ordering::Relaxed));
                let (inputs, control) = slot_channels(SharedEq::new(10));
                self.nodes.lock().push(inputs);
                Reply::SlotAllocated(AllocatedSlot::new(control, slot))
            }
            Cmd::QuerySampleRate => Reply::SampleRate(SessionSampleRate::new(None, 44_100)),
            Cmd::SessionDucking => Reply::SessionDucking(SessionDuckingMode::Off),
            _ => Reply::Ok,
        };
        Ok(reply)
    }
}

pub(crate) fn test_session() -> Arc<dyn SessionDispatcher> {
    Arc::new(TestSession {
        next_player: AtomicU64::new(1),
        next_slot: AtomicU64::new(0),
        nodes: Mutex::default(),
    })
}
