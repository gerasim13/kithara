use kithara_bufpool::{BytePool, PooledOwned};
use kithara_decode::{PcmChunk, PcmSpec};
use kithara_events::{AudioEvent, AudioFormat, DeferredBus, Event};
use kithara_stream::PlayheadWrite;

use crate::traits::AudioEffect;

pub(crate) struct EofDrain {
    spec: Option<PcmSpec>,
    exhausted: PooledOwned<32, Vec<u8>>,
    active: bool,
    chunks: u64,
    samples: u64,
}

impl EofDrain {
    pub(crate) fn new(effect_count: usize, byte_pool: &BytePool) -> Self {
        Self {
            exhausted: byte_pool.get_with(|buffer| buffer.resize(effect_count, 0)),
            active: false,
            chunks: 0,
            samples: 0,
            spec: None,
        }
    }

    pub(crate) fn next(&mut self, effects: &mut [Box<dyn AudioEffect>]) -> Option<PcmChunk> {
        if effects.is_empty() {
            return None;
        }
        if !self.active {
            self.exhausted.fill(0);
            self.active = true;
        }
        let last = effects.len() - 1;
        pull(effects, &mut self.exhausted, last)
    }

    pub(crate) const fn reset(&mut self) {
        self.active = false;
    }

    pub(crate) const fn stats(&self) -> (u64, u64) {
        (self.chunks, self.samples)
    }

    pub(crate) fn track(
        &mut self,
        chunk: &PcmChunk,
        playhead: &dyn PlayheadWrite,
        emit: Option<&DeferredBus<Event>>,
    ) {
        self.chunks += 1;
        self.samples += chunk.samples.len() as u64;
        playhead.set_decoded_frontier(chunk.meta.end_timestamp);
        if self.chunks == 1 {
            if let Some(emit) = emit {
                emit.enqueue(
                    AudioEvent::FormatDetected {
                        spec: AudioFormat::new(
                            chunk.spec().channels,
                            chunk.spec().sample_rate.get(),
                        ),
                    }
                    .into(),
                );
            }
            self.spec = Some(chunk.spec());
        }
        if let Some(old) = self.spec
            && old != chunk.spec()
        {
            if let Some(emit) = emit {
                emit.enqueue(
                    AudioEvent::FormatChanged {
                        old: AudioFormat::new(old.channels, old.sample_rate.get()),
                        new: AudioFormat::new(
                            chunk.spec().channels,
                            chunk.spec().sample_rate.get(),
                        ),
                    }
                    .into(),
                );
            }
            self.spec = Some(chunk.spec());
        }
    }
}

fn pull(
    effects: &mut [Box<dyn AudioEffect>],
    exhausted: &mut [u8],
    stage: usize,
) -> Option<PcmChunk> {
    if stage == 0 {
        return effects[0].flush();
    }
    loop {
        if exhausted[stage] == 0 {
            if let Some(chunk) = pull(effects, exhausted, stage - 1) {
                if let Some(output) = effects[stage].process(chunk) {
                    return Some(output);
                }
                continue;
            }
            exhausted[stage] = 1;
        }
        return effects[stage].flush();
    }
}
