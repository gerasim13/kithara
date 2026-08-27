use kithara_decode::{PcmChunk, PcmSpec};
use kithara_events::{AudioEvent, AudioFormat, DeferredBus, Event};

#[derive(Default)]
pub(crate) struct DecodedOutput {
    spec: Option<PcmSpec>,
    chunks: u64,
    samples: u64,
}

impl DecodedOutput {
    pub(crate) const fn stats(&self) -> (u64, u64) {
        (self.chunks, self.samples)
    }

    pub(crate) fn track(&mut self, chunk: &PcmChunk, emit: Option<&DeferredBus<Event>>) {
        self.chunks += 1;
        self.samples += chunk.samples.len() as u64;
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
