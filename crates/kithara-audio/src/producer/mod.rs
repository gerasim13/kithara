use crate::{AudioEvent, DecoderEvent};
use kithara_events::EventSet;

/// Deferred diagnostics from the owner-thread decoder chain.
#[derive(Clone, Debug, EventSet)]
#[non_exhaustive]
pub enum AudioLaneEvent {
    Decoder(DecoderEvent),
    Audio(AudioEvent),
}
#[cfg(test)]
mod gate;
