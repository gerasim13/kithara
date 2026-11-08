use audioadapter_buffers::direct::InterleavedSlice;
use bevy_platform::time::Instant;
use firewheel::{backend::BackendProcessInfo, node::StreamStatus, processor::FirewheelProcessor};
use kithara_platform::time::Duration;

/// Settles queued schedules at the old geometry before returning the actual processor.
pub(crate) struct SessionProcessor {
    processor: FirewheelProcessor,
    channels: usize,
    last: BackendProcessInfo,
}

impl SessionProcessor {
    pub(crate) fn new(processor: FirewheelProcessor, channels: usize) -> Self {
        Self {
            processor,
            channels,
            last: BackendProcessInfo {
                frames: 0,
                process_timestamp: Some(Instant::now()),
                duration_since_stream_start: Duration::ZERO,
                input_stream_status: StreamStatus::empty(),
                output_stream_status: StreamStatus::empty(),
                dropped_frames: 0,
                process_to_playback_delay: None,
            },
        }
    }

    pub(crate) fn process(&mut self, output: &mut [f32], info: BackendProcessInfo) {
        // CPAL and the offline owner supply complete interleaved frames.
        let input = InterleavedSlice::new(&[] as &[f32], 0, 0)
            .expect("zero input frames have zero samples");
        let mut output = InterleavedSlice::new_mut(output, self.channels, info.frames)
            .expect("the admitted output contains complete interleaved frames");
        self.last = info.clone();
        self.processor.process(&input, &mut output, info);
    }
}

impl Drop for SessionProcessor {
    fn drop(&mut self) {
        let mut stopped = self.last.clone();
        stopped.frames = 0;
        stopped.input_stream_status = StreamStatus::empty();
        stopped.output_stream_status = StreamStatus::empty();
        stopped.dropped_frames = 0;
        // Keep update_instant: process0 republishes the shared sample clock.
        self.process(&mut [], stopped);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use firewheel::{ActivateInfo, FirewheelConfig, FirewheelContext};
    use kithara_test_utils::kithara;

    use super::*;
    use crate::rt::MasterNode;

    #[kithara::test]
    fn stopping_settles_the_real_queued_schedule_without_moving_its_clock() {
        let mut context = FirewheelContext::new(FirewheelConfig::default());
        let mut shape = ActivateInfo {
            sample_rate: NonZeroU32::new(48_000).expect("rate"),
            max_block_frames: NonZeroU32::new(128).expect("block"),
            num_stream_in_channels: 0,
            num_stream_out_channels: 2,
            input_to_output_latency_seconds: 0.0,
        };
        let processor = context.activate(shape).expect("initial graph");
        let mut actual = SessionProcessor::new(processor, 2);
        let mut info = actual.last.clone();
        info.frames = 4;
        actual.process(&mut [0.0; 8], info);
        let clock = context.heartbeat();
        let timestamp = context.audio_clock_instant();
        let node = context
            .add_node(MasterNode, None)
            .expect("real master node");
        context
            .connect(node, context.graph_out_node_id(), &[(0, 0), (1, 1)], true)
            .expect("master route");
        context.update().expect("queued old-geometry schedule");
        drop(actual);
        assert_eq!(context.heartbeat(), clock);
        assert_eq!(context.audio_clock_instant(), timestamp);
        assert!(!context.is_active());
        shape.max_block_frames = NonZeroU32::new(64).expect("new geometry");
        let processor = context.activate(shape).expect("same context restart");
        let mut actual = SessionProcessor::new(processor, 2);
        let mut info = actual.last.clone();
        info.frames = 1;
        actual.process(&mut [0.0; 2], info);
        assert_eq!(
            context.heartbeat(),
            clock.checked_add(1).expect("one rendered frame")
        );
    }
}
