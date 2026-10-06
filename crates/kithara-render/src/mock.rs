//! The audio-thread side of a deck, driven by a test in place of a `DeckMixer`.

use kithara_command::Step;
use kithara_signal::SessionFrame;
use ringbuf::traits::Producer;

use crate::bridge::{DeckApplied, DeckPart, NodeInputs, PlaybackShared, PlayerNotification};

/// Take the batches the deck's inbox holds, each as its parts, as the deck does at its next
/// block.
#[must_use]
pub fn take_batches(inputs: &mut NodeInputs) -> Vec<Vec<DeckPart>> {
    let mut batches: Vec<Vec<DeckPart>> = Vec::new();
    inputs.deck.run_block(SessionFrame::default(), 1, |step| {
        if let Step::Due(mut due) = step {
            batches.push(std::mem::take(due.commands_mut()));
            due.apply(DeckApplied::default());
        }
    });
    batches
}

/// Send `notification` to the control side as the deck's audio thread does.
///
/// # Errors
/// Returns the notification when the ring is full.
pub fn notify(
    inputs: &mut NodeInputs,
    notification: PlayerNotification,
) -> Result<(), PlayerNotification> {
    inputs.notif_tx.try_push(notification)
}

/// Take on the item `epoch` made leading, publishing its playhead, as the audio thread does.
pub fn adopt(playback: &PlaybackShared, epoch: u64, position: f64, duration: f64) {
    playback.adopt(epoch, position, duration);
}

/// Publish the playhead of the track the audio thread renders, taking nothing on.
pub fn publish_playhead(playback: &PlaybackShared, position: f64, duration: f64) {
    playback.position.store(position);
    playback.duration.store(duration);
}

/// Publish how far the rendered track is decoded and cached.
pub fn publish_buffered(playback: &PlaybackShared, frontier: f64, cached: f64) {
    playback.frontier.store(frontier);
    playback.cached.store(cached);
}

/// Whether the track leading under `epoch` describes the playhead now.
#[must_use]
pub fn publishes(playback: &PlaybackShared, epoch: u64) -> bool {
    playback.publishing().admit(epoch)
}
