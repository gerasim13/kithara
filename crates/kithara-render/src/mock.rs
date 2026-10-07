//! The audio-thread side of a deck, driven by a test in place of a `DeckMixer`.

use kithara_command::Step;
use kithara_signal::SessionFrame;
use ringbuf::traits::Producer;

use crate::{
    bridge::{DeckApplied, DeckEvent, DeckPart, MixerInputs, Released, Slot},
    rt::track::PlayerResource,
};

/// Take the batches the deck's inbox holds, each as its parts, and apply them on `at`, as the
/// mixer does at its next block.
#[must_use]
pub fn take_batches(inputs: &mut MixerInputs, at: SessionFrame) -> Vec<Vec<DeckPart>> {
    let mut batches: Vec<Vec<DeckPart>> = Vec::new();
    inputs.inbox.run_block(at, 1, |step| {
        if let Step::Due(mut due) = step {
            batches.push(std::mem::take(due.commands_mut()));
            due.apply(DeckApplied::default());
        }
    });
    batches
}

/// Report `event` to the deck's owner as the mixer does.
///
/// # Errors
/// Returns the event when the ring is full.
pub fn report(inputs: &mut MixerInputs, event: DeckEvent) -> Result<(), DeckEvent> {
    inputs.events.try_push(event)
}

/// A deck's mixer with no audio: it holds the consumers attached to its slots and answers every
/// batch the way the mixer does, its parts back as they applied.
pub struct MockDeck {
    inputs: MixerInputs,
    held: Vec<Option<Box<PlayerResource>>>,
}

impl MockDeck {
    #[must_use]
    pub fn new(inputs: MixerInputs) -> Self {
        let held = std::iter::repeat_with(|| None)
            .take(inputs.config.slots().get())
            .collect();
        Self { inputs, held }
    }

    /// Applies every batch due by `at` on its own frame; a `Stop` reports the slot at
    /// `stopped_at` seconds.
    pub fn block(&mut self, at: SessionFrame, stopped_at: f64) {
        let held = &mut self.held;
        self.inputs.inbox.run_block(at, 1, |step| {
            if let Step::Due(mut due) = step {
                let mut applied = DeckApplied::default();
                let commands = due.commands_mut();
                for _ in 0..commands.len() {
                    let part = commands.remove(0);
                    if let Some(left) = hold(held, part, &mut applied, stopped_at) {
                        commands.push(left);
                    }
                }
                due.apply(applied);
            }
        });
    }

    /// Reports `event` to the deck's owner as the mixer does.
    ///
    /// # Errors
    /// Returns the event when the ring is full.
    pub fn report(&mut self, event: DeckEvent) -> Result<(), DeckEvent> {
        report(&mut self.inputs, event)
    }

    /// The source of the consumer `slot` holds.
    #[must_use]
    pub fn held(&self, slot: Slot) -> Option<&str> {
        self.held
            .get(usize::from(slot.get()))
            .and_then(Option::as_ref)
            .map(|pcm| &**pcm.src())
    }
}

/// What `part` leaves in the receipt once `held` applied it.
fn hold(
    held: &mut [Option<Box<PlayerResource>>],
    part: DeckPart,
    applied: &mut DeckApplied,
    stopped_at: f64,
) -> Option<DeckPart> {
    let entry = |held: &mut [Option<Box<PlayerResource>>], slot: Slot| {
        held.get_mut(usize::from(slot.get()))
            .and_then(Option::take)
            .map(|pcm| DeckPart::Released(Released::Pcm { slot, pcm }))
    };
    match part {
        DeckPart::Attach { slot, pcm } | DeckPart::Replace { slot, pcm } => {
            let left = entry(held, slot);
            if let Some(entry) = held.get_mut(usize::from(slot.get())) {
                *entry = Some(pcm);
            }
            left
        }
        DeckPart::Detach { slot } => entry(held, slot),
        DeckPart::Stop { slot, fade } => {
            applied.stopped_at = Some(stopped_at);
            Some(DeckPart::Stop { slot, fade })
        }
        DeckPart::Eq(crate::bridge::DeckEqChange::Layout(_)) => None,
        other => Some(other),
    }
}
