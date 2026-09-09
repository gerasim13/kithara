use kithara_events::{AudioEvent, DeferredBus};
use kithara_signal::{AudioChunk, AudioSpec};

use crate::AudioLaneEvent;

#[derive(Default)]
pub(crate) struct DecodedOutput {
    spec: Option<AudioSpec>,
    chunks: u64,
    samples: u64,
}

impl DecodedOutput {
    pub(crate) const fn stats(&self) -> (u64, u64) {
        (self.chunks, self.samples)
    }

    pub(crate) fn track(&mut self, chunk: &AudioChunk, emit: Option<&DeferredBus<AudioLaneEvent>>) {
        self.chunks += 1;
        self.samples += chunk.samples.len() as u64;
        if self.chunks == 1 {
            if let Some(emit) = emit {
                emit.enqueue(AudioEvent::FormatDetected { spec: chunk.spec() });
            }
            self.spec = Some(chunk.spec());
        }
        if let Some(old) = self.spec
            && old != chunk.spec()
        {
            if let Some(emit) = emit {
                emit.enqueue(AudioEvent::FormatChanged {
                    old,
                    new: chunk.spec(),
                });
            }
            self.spec = Some(chunk.spec());
        }
    }
}
