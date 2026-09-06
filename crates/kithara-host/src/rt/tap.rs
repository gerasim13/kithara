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
use kithara_signal::AudioSpec;
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
    sample_rate: NonZeroU32,
    outputs: Option<OutputGroup>,
}

impl TapProcessor {
    fn new(mut outputs: Option<OutputGroup>, sample_rate: NonZeroU32) -> Self {
        if let Some(outputs) = outputs.as_mut() {
            outputs.reconfigure(AudioSpec::new(2, sample_rate));
        }
        Self {
            sample_rate,
            outputs,
        }
    }

    fn adopt_rate(&mut self, sample_rate: NonZeroU32) {
        if sample_rate != self.sample_rate {
            if let Some(outputs) = self.outputs.as_mut() {
                outputs.reconfigure(AudioSpec::new(2, sample_rate));
            }
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
    use kithara_platform::sync::{
        Arc,
        atomic::AtomicU64,
        mpsc::{self, Sender},
    };
    use ringbuf::{
        HeapRb,
        traits::{Observer, Split},
    };

    use super::*;
    use crate::bridge::MixTapWriter;

    struct RateOutput(Sender<AudioSpec>);

    impl LiveOutput for RateOutput {
        fn reconfigure(&mut self, spec: AudioSpec) {
            let _ = self.0.send(spec);
        }

        fn write_stereo(&mut self, _frames: usize, _left: &[f32], _right: &[f32]) {}
    }

    fn rate(hz: u32) -> NonZeroU32 {
        NonZeroU32::new(hz).expect("test rate is non-zero")
    }

    #[kithara::test]
    fn a_new_route_receives_the_current_device_rate() {
        let (observed_tx, observed_rx) = mpsc::channel();
        let mut outputs = OutputGroup::new();
        outputs.push(RateOutput(observed_tx));

        let _processor = TapProcessor::new(Some(outputs), rate(48_000));

        assert_eq!(
            observed_rx.recv().expect("initial route format"),
            AudioSpec::new(2, rate(48_000))
        );
    }

    #[kithara::test]
    fn a_changed_device_rate_keeps_the_feed() {
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
            cons.write_is_held(),
            "a rate change reconfigures the feed instead of ending it"
        );
    }
}
