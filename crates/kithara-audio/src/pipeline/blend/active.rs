use kithara_decode::PcmChunk;

use crate::pipeline::blend::side::BlendSide;

/// What the blender currently has to mix.
///
/// `Single` is not a way around the blender — it is the blender's one-input
/// arm. PCM takes the same path through it as it does with two inputs, and
/// effects are applied once downstream either way, so a track that never
/// switches variant is sample-identical to one that has no blender at all.
pub(crate) enum ActiveDecode {
    Single(BlendSide),
}

impl ActiveDecode {
    /// The side whose frames are what the listener is hearing, and whose
    /// decoder answers for position, seeking and metadata. During a ramp that
    /// is already the incoming one: the outgoing generation has no decoder
    /// left to answer with.
    pub(crate) fn audible(&self) -> &BlendSide {
        match self {
            Self::Single(side) => side,
        }
    }

    pub(crate) fn audible_mut(&mut self) -> &mut BlendSide {
        match self {
            Self::Single(side) => side,
        }
    }

    /// The blender's output: one chunk of PCM, mixed from however many inputs
    /// are live. Effects run after this, once.
    pub(crate) fn next(&mut self) -> Option<PcmChunk> {
        match self {
            Self::Single(side) => side.emit(),
        }
    }

    /// The blender's output once no further input can arrive: whatever is held
    /// back is owed to the listener rather than kept for a ramp that will
    /// never start.
    pub(crate) fn release(&mut self) -> Option<PcmChunk> {
        match self {
            Self::Single(side) => side.release(),
        }
    }
}
