use kithara_bufpool::{BytePool, PooledOwned};
use kithara_decode::PcmChunk;

use super::AudioEffect;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum StageState {
    UpstreamActive,
    Flushing,
    Exhausted,
}

pub(crate) enum EffectDrainStep {
    Produced(PcmChunk),
    Progress,
    Exhausted,
}

pub(crate) struct EffectDrain {
    exhausted: PooledOwned<32, Vec<u8>>,
    active: bool,
}

impl EffectDrain {
    pub(crate) fn new(effect_count: usize, pool: &BytePool) -> Self {
        Self {
            exhausted: pool.get_with(|buffer| buffer.resize(effect_count, 0)),
            active: false,
        }
    }

    pub(crate) fn step(&mut self, effects: &mut [Box<dyn AudioEffect>]) -> EffectDrainStep {
        if effects.is_empty() {
            return EffectDrainStep::Exhausted;
        }
        if !self.active {
            self.exhausted.fill(StageState::UpstreamActive as u8);
            self.active = true;
        }
        pull(effects, &mut self.exhausted, effects.len() - 1)
    }

    pub(crate) const fn reset(&mut self) {
        self.active = false;
    }
}

fn pull(
    effects: &mut [Box<dyn AudioEffect>],
    exhausted: &mut [u8],
    stage: usize,
) -> EffectDrainStep {
    if stage == 0 {
        return flush_stage(effects, exhausted, stage);
    }

    if exhausted[stage] == StageState::UpstreamActive as u8 {
        match pull(effects, exhausted, stage - 1) {
            EffectDrainStep::Produced(chunk) => {
                return effects[stage]
                    .process(chunk)
                    .map_or(EffectDrainStep::Progress, EffectDrainStep::Produced);
            }
            EffectDrainStep::Progress => return EffectDrainStep::Progress,
            EffectDrainStep::Exhausted => exhausted[stage] = StageState::Flushing as u8,
        }
    }
    flush_stage(effects, exhausted, stage)
}

fn flush_stage(
    effects: &mut [Box<dyn AudioEffect>],
    exhausted: &mut [u8],
    stage: usize,
) -> EffectDrainStep {
    if exhausted[stage] == StageState::Exhausted as u8 {
        return EffectDrainStep::Exhausted;
    }
    effects[stage].flush().map_or_else(
        || {
            exhausted[stage] = StageState::Exhausted as u8;
            EffectDrainStep::Exhausted
        },
        EffectDrainStep::Produced,
    )
}
