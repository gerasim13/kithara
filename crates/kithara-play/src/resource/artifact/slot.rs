use kithara_beat::BeatGridModel;
use kithara_platform::sync::{Arc, Mutex};

/// The prepared beat grid of one load, as that load knows it right now.
///
/// A track opened with a value in hand holds it from the start; a track opened
/// with a source holds nothing until its read answers. The slot is what the
/// two share, so a late answer reaches the very load that asked for it and
/// nothing else: the load that has moved on dropped its end of the slot.
///
/// The generation is what a reader compares. A player rebuilds the geometry it
/// publishes only when the slot actually changed, so a grid revision never
/// advances for a model nobody replaced.
#[derive(Default)]
pub struct PreparedGrid {
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    generation: u64,
    model: Option<Arc<BeatGridModel>>,
}

impl PreparedGrid {
    /// A slot whose model is already in hand.
    #[must_use]
    pub fn holding(model: Arc<BeatGridModel>) -> Self {
        Self {
            held: Mutex::new(Held {
                generation: 1,
                model: Some(model),
            }),
        }
    }

    /// Which model this slot holds, and the generation it holds it at.
    #[must_use]
    pub fn read(&self) -> (u64, Option<Arc<BeatGridModel>>) {
        let held = self.held.lock();
        (held.generation, held.model.clone())
    }

    /// Hand this load the model its source answered with.
    pub fn put(&self, model: Arc<BeatGridModel>) {
        let mut held = self.held.lock();
        held.generation = held.generation.wrapping_add(1);
        held.model = Some(model);
    }
}
