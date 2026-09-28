use kithara_beat::BeatGridModel;
use kithara_platform::{
    CancelToken,
    sync::{Arc, Mutex},
};

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

impl Held {
    fn replace(&mut self, model: Arc<BeatGridModel>) {
        if self.model.as_deref() == Some(&*model) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.model = Some(model);
    }
}

impl PreparedGrid {
    /// Hand the track the model a load's own read answered with, unless
    /// that load is over by now: the slot can outlive the load that asked,
    /// and what a later load or the track's owner put there stays. The
    /// check holds the slot, so an answer that lands orders before any put
    /// made after its load ended.
    pub(crate) fn answer(&self, model: Arc<BeatGridModel>, load: Option<&CancelToken>) {
        let mut held = self.held.lock();
        if load.is_some_and(CancelToken::is_cancelled) {
            return;
        }
        held.replace(model);
    }

    /// Hand the track the model it was opened with, or the one its owner
    /// found. A model equal to the one held changes nothing.
    pub fn put(&self, model: Arc<BeatGridModel>) {
        self.held.lock().replace(model);
    }

    /// Which model this slot holds, and the generation it holds it at.
    #[must_use]
    pub fn read(&self) -> (u64, Option<Arc<BeatGridModel>>) {
        let held = self.held.lock();
        (held.generation, held.model.clone())
    }
}

#[cfg(test)]
mod tests {
    use kithara_beat::{BeatGridState, GridBeat, RawBeatGrid, SCHEMA_VERSION};
    use kithara_test_utils::kithara;

    use super::*;

    /// Two beats at `bpm`, one beat apart.
    fn model(bpm: f64) -> Arc<BeatGridModel> {
        let spacing = 60.0 / bpm;
        Arc::new(
            BeatGridModel::try_from(RawBeatGrid {
                schema_version: SCHEMA_VERSION,
                model_id: "slot".to_owned(),
                revision: 1,
                state: BeatGridState::Final,
                duration: None,
                bpm,
                beats: (0..2)
                    .map(|ordinal| GridBeat {
                        at: f64::from(ordinal) * spacing,
                        ordinal: i64::from(ordinal),
                        confidence: Some(1.0),
                    })
                    .collect(),
                downbeats: Vec::new(),
                meter: None,
            })
            .expect("the fixture grid holds together"),
        )
    }

    #[kithara::test]
    fn an_answer_from_a_load_that_is_over_leaves_the_slot_as_it_is() {
        let slot = PreparedGrid::default();
        slot.put(model(124.0));
        let (generation, held) = slot.read();
        let load = CancelToken::never().child();
        load.cancel();

        slot.answer(model(120.0), Some(&load));

        let (after, kept) = slot.read();
        assert_eq!(after, generation, "nothing new reached the slot");
        assert_eq!(kept, held, "the model put after the load ended stays");
    }

    #[kithara::test]
    fn an_answer_from_a_live_load_reaches_the_slot() {
        let slot = PreparedGrid::default();
        slot.put(model(124.0));
        let (generation, _) = slot.read();
        let load = CancelToken::never().child();

        slot.answer(model(120.0), Some(&load));

        let (after, held) = slot.read();
        assert_ne!(after, generation, "the answer is a new generation");
        assert_eq!(held, Some(model(120.0)), "the live load's answer is held");
    }
}
