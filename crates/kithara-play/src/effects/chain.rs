use kithara_decode::PcmChunk;

use super::AudioEffect;

/// Apply the effect chain to the chunk.
pub(crate) fn apply_effects(
    effects: &mut [Box<dyn AudioEffect>],
    mut chunk: PcmChunk,
) -> Option<PcmChunk> {
    for effect in &mut *effects {
        chunk = effect.process(chunk)?;
    }
    Some(chunk)
}

pub(crate) fn held_source_frames(effects: &[Box<dyn AudioEffect>]) -> u64 {
    effects.iter().fold(0_u64, |total, effect| {
        total.saturating_add(effect.held_source_frames())
    })
}

/// Reset effects chain (e.g. after seek).
pub(crate) fn reset_effects(effects: &mut [Box<dyn AudioEffect>]) {
    for effect in &mut *effects {
        effect.reset();
    }
}
