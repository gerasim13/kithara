use kithara_bufpool::HasPool;
use kithara_events::TrackId;
use kithara_play::Resource;

use super::{Queue, QueueControl};

/// Probe-only operations for deterministic queue fixtures.
pub trait QueueProbe {
    fn complete_load_for_test(&self, id: TrackId, resource: Resource);
    fn insert_loaded_for_test(&self, resource: Resource) -> TrackId;
    fn mark_played_for_test(&self, id: TrackId);
    fn register_for_test(&self) -> TrackId;
    fn supply_test_resource_for_respawn(&self, id: TrackId, resource: Resource);
}

impl<S> QueueProbe for QueueControl<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    delegate::delegate! {
        to self {
            #[call(probe_complete_load)]
            fn complete_load_for_test(&self, id: TrackId, resource: Resource);
            #[call(probe_insert_loaded)]
            fn insert_loaded_for_test(&self, resource: Resource) -> TrackId;
            #[call(probe_mark_played)]
            fn mark_played_for_test(&self, id: TrackId);
            #[call(probe_register)]
            fn register_for_test(&self) -> TrackId;
            #[call(probe_supply_respawn_resource)]
            fn supply_test_resource_for_respawn(&self, id: TrackId, resource: Resource);
        }
    }
}

impl<S> QueueProbe for Queue<S>
where
    S: HasPool<u8> + HasPool<f32> + Send + Sync + 'static,
{
    fn complete_load_for_test(&self, id: TrackId, resource: Resource) {
        QueueProbe::complete_load_for_test(&self.control, id, resource);
    }

    fn insert_loaded_for_test(&self, resource: Resource) -> TrackId {
        QueueProbe::insert_loaded_for_test(&self.control, resource)
    }

    fn mark_played_for_test(&self, id: TrackId) {
        QueueProbe::mark_played_for_test(&self.control, id);
    }

    fn register_for_test(&self) -> TrackId {
        QueueProbe::register_for_test(&self.control)
    }

    fn supply_test_resource_for_respawn(&self, id: TrackId, resource: Resource) {
        QueueProbe::supply_test_resource_for_respawn(&self.control, id, resource);
    }
}
