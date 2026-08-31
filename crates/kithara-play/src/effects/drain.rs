use kithara_bufpool::{ByteBuffer, HasPool, PoolError, PoolRegion};
use kithara_signal::AudioChunk;

use super::AudioEffect;

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum StageState {
    UpstreamActive,
    Flushing,
    Exhausted,
}

pub(crate) enum EffectDrainStep {
    Produced(AudioChunk),
    Progress,
    Exhausted,
}

pub(crate) struct EffectDrain {
    exhausted: ByteBuffer,
    active: bool,
}

impl EffectDrain {
    pub(crate) fn new<S>(effect_count: usize, pools: &PoolRegion<S>) -> Result<Self, PoolError>
    where
        S: HasPool<u8>,
    {
        Ok(Self {
            exhausted: pools.get_with_len::<u8>(effect_count)?,
            active: false,
        })
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
