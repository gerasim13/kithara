use core::num::NonZeroU32;

use firewheel::{
    StreamInfo,
    channel_config::{ChannelConfig, ChannelCount},
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
};
use kithara_output::{LiveOutput, OutputGroup};
use kithara_platform::sync::Mutex;
use kithara_test_utils::kithara;

pub(crate) struct TapNode {
    outputs: Mutex<Option<OutputGroup>>,
}

impl TapNode {
    pub(crate) fn new(outputs: OutputGroup) -> Self {
        Self {
            outputs: Mutex::new(Some(outputs)),
        }
    }
}

impl AudioNode for TapNode {
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        TapProcessor::new(self.outputs.lock().take(), cx.stream_info.sample_rate)
    }

    fn info(&self, _config: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("session_mix_tap")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::ZERO,
            })
    }
}

struct TapProcessor {
    outputs: Option<OutputGroup>,
    sample_rate: NonZeroU32,
}

impl TapProcessor {
    fn new(outputs: Option<OutputGroup>, sample_rate: NonZeroU32) -> Self {
        Self {
            outputs,
            sample_rate,
        }
    }

    fn adopt_rate(&mut self, sample_rate: NonZeroU32) {
        if sample_rate != self.sample_rate {
            self.outputs = None;
            self.sample_rate = sample_rate;
        }
    }
}

impl AudioNodeProcessor for TapProcessor {
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.adopt_rate(stream_info.sample_rate);
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let Some(outputs) = self.outputs.as_mut() else {
            return ProcessStatus::ClearAllOutputs;
        };
        let [left, right, ..] = buffers.inputs else {
            outputs.write_stereo(info.frames, &[], &[]);
            return ProcessStatus::ClearAllOutputs;
        };
        outputs.write_stereo(info.frames, left, right);

        ProcessStatus::ClearAllOutputs
    }
}

#[cfg(test)]
mod tests {
    use kithara_platform::sync::{Arc, atomic::AtomicU64};
    use ringbuf::{
        HeapRb,
        traits::{Observer, Split},
    };

    use super::*;
    use crate::bridge::MixTapWriter;

    fn rate(hz: u32) -> NonZeroU32 {
        NonZeroU32::new(hz).expect("test rate is non-zero")
    }

    #[kithara::test]
    fn a_changed_device_rate_ends_the_feed() {
        const CAPACITY: usize = 64;

        let (pcm, cons) = HeapRb::<f32>::new(CAPACITY).split();
        let writer = MixTapWriter::new(pcm, Arc::new(AtomicU64::new(0)));
        let mut outputs = OutputGroup::new();
        outputs.push(writer);
        let mut processor = TapProcessor::new(Some(outputs), rate(44_100));

        processor.adopt_rate(rate(44_100));
        assert!(cons.write_is_held(), "an equal rate keeps the feed running");

        processor.adopt_rate(rate(48_000));
        assert!(
            !cons.write_is_held(),
            "a rate change must end the feed rather than relabel the samples"
        );
    }
}
