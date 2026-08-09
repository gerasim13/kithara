use std::num::NonZeroU32;

use beat::Slot;
use kithara_decode::PcmChunk;
use kithara_resampler::ResamplerBackend;
use num_traits::cast::AsPrimitive;

use crate::{
    analysis::slots::{beat, waveform},
    waveform::{BeatGrid, bucket::Waveform},
};

#[derive(fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub(crate) struct TrackAnalyzers<B>
where
    B: ResamplerBackend,
{
    pub(super) beat: beat::Slot<B>,
    pub(super) waveform: waveform::Slot,
    #[field(get, vis = "pub(crate)")]
    pub(super) source_frames: u64,
    /// Sample-rate axis frozen from the first decoded chunk of this pass.
    #[field(get, copy, vis = "pub(crate)")]
    pub(super) source_sample_rate: NonZeroU32,
}

impl<B> TrackAnalyzers<B>
where
    B: ResamplerBackend,
{
    pub(crate) fn finish_beat(self, detector: Option<&mut beat::Detector>) -> Option<BeatGrid> {
        Slot::finish(self.beat, detector)
    }

    pub(crate) fn finish_waveform(&mut self) -> Option<Waveform> {
        waveform::finish(std::mem::take(&mut self.waveform))
    }

    pub(crate) fn has_beat(&self) -> bool {
        !Slot::is_empty(&self.beat)
    }

    pub(crate) fn push(&mut self, chunk: &PcmChunk, detector: Option<&mut beat::Detector>) {
        let frames: u64 = chunk.frames().as_();
        self.source_frames = self.source_frames.saturating_add(frames);

        waveform::push(&mut self.waveform, chunk);
        Slot::push(&mut self.beat, chunk, detector);
    }
}
