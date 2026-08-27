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

/// Reset effects chain (e.g. after seek).
pub(crate) fn reset_effects(effects: &mut [Box<dyn AudioEffect>]) {
    for effect in &mut *effects {
        effect.reset();
    }
}
