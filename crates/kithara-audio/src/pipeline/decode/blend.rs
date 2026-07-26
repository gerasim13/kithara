use kithara_decode::PcmChunk;

use crate::pipeline::{decode::core::DecoderSession, gapless::GaplessStage};

/// One input to the blender: a decoder generation together with the gapless
/// stage that owns its trim contract.
///
/// The stage belongs to the side, not to the pipeline, because two generations
/// blending across a variant switch are at different points of their own
/// leading/trailing contracts — a shared trimmer would apply one side's state
/// to the other's frames.
pub(crate) struct BlendSide {
    pub(crate) session: DecoderSession,
    pub(crate) gapless: GaplessStage,
}

impl BlendSide {
    pub(crate) fn new(session: DecoderSession, gapless: GaplessStage) -> Self {
        Self { session, gapless }
    }
}

/// What the blender currently has to mix.
///
/// `Single` is not a way around the blender — it is the blender's one-input
/// arm. PCM takes the same path through it as it does with two inputs, and
/// effects are applied once downstream either way, so a track that never
/// switches variant is bit-identical to one that has no blender at all.
pub(crate) enum ActiveDecode {
    Single(BlendSide),
}

impl ActiveDecode {
    /// The side whose frames are what the listener is hearing, and whose
    /// decoder answers for position, seeking and metadata.
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
            Self::Single(side) => side.gapless.next(),
        }
    }
}
