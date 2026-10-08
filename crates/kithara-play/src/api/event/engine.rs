use kithara_events::{Event, SlotId};
use kithara_platform::time::Duration;

#[derive(Clone, Debug, Event)]
pub enum EngineEvent {
    Started,
    Stopped,
    SlotAllocated {
        slot: SlotId,
    },
    SlotReleased {
        slot: SlotId,
    },
    CrossfadeStarted {
        from: SlotId,
        to: SlotId,
        duration: Duration,
    },
    CrossfadeProgress {
        from: SlotId,
        to: SlotId,
        progress: f32,
    },
    CrossfadeCompleted {
        from: SlotId,
        to: SlotId,
    },
    CrossfadeCancelled,
    MasterVolumeChanged {
        volume: f32,
    },
}
