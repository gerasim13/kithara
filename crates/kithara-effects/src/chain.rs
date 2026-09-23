use kithara_signal::AudioChunk;

use crate::AudioEffect;

/// Apply the effect chain to the chunk.
pub fn apply_effects(
    effects: &mut [Box<dyn AudioEffect>],
    mut chunk: AudioChunk,
) -> Option<AudioChunk> {
    for effect in &mut *effects {
        chunk = effect.process(chunk)?;
    }
    Some(chunk)
}

/// Frames the chain still holds back from the source it was fed.
#[must_use]
pub fn held_source_frames(effects: &[Box<dyn AudioEffect>]) -> u64 {
    effects.iter().fold(0_u64, |total, effect| {
        total.saturating_add(effect.held_source_frames())
    })
}

/// Reset effects chain (e.g. after seek).
pub fn reset_effects(effects: &mut [Box<dyn AudioEffect>]) {
    for effect in &mut *effects {
        effect.reset();
    }
}
