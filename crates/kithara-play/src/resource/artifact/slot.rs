use kithara_beat::BeatGridModel;
use kithara_platform::sync::{Arc, Mutex};

/// The prepared beat grid of one track, as its loads know it right now.
///
/// A track opened with a value in hand holds it from the start; a track opened
/// with a source holds nothing until its read answers. What outlives a single
/// load (a queue's track record) may own the slot and hand it to every load of
/// the track, so a grid it learns while a load is in flight reaches that load
/// too. Without such an owner the load and its read share the only ends, so a
/// late answer reaches the very load that asked for it and nothing else.
///
/// The generation is what a reader compares. A player rebuilds the geometry it
/// publishes only when the slot actually changed, so a grid revision never
/// advances for a model nobody replaced, nor for one equal to the model held.
#[derive(Default)]
pub struct PreparedGrid {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    model: Option<Arc<BeatGridModel>>,
    generation: u64,
}

impl PreparedGrid {
    /// Hand the track the model its source answered with, or the one its
    /// owner found. A model equal to the one held changes nothing.
    pub fn put(&self, model: Arc<BeatGridModel>) {
        let mut held = self.held.lock();
        if held.model.as_deref() == Some(&*model) {
            return;
        }
        held.generation = held.generation.wrapping_add(1);
        held.model = Some(model);
    }

    /// Which model this slot holds, and the generation it holds it at.
    #[must_use]
    pub fn read(&self) -> (u64, Option<Arc<BeatGridModel>>) {
        let held = self.held.lock();
        (held.generation, held.model.clone())
    }
}
