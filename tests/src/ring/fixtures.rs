use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU32, AtomicUsize, Ordering},
};

use firewheel::{
    FirewheelCtx, StreamInfo,
    channel_config::{ChannelConfig, ChannelCount},
    event::ProcEvents,
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        NodeID, ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
};
use kithara::platform::sync::Arc;

use super::RingBackend;

#[derive(Clone, Default)]
pub struct CountingProbe {
    inner: Arc<CountingProbeInner>,
}

#[derive(Default)]
struct CountingProbeInner {
    constructions: AtomicUsize,
    construction_sample_rate: AtomicU32,
    new_streams: AtomicUsize,
}

impl CountingProbe {
    pub fn construction_count(&self) -> usize {
        self.inner.constructions.load(Ordering::SeqCst)
    }

    pub fn construction_sample_rate(&self) -> Option<NonZeroU32> {
        NonZeroU32::new(self.inner.construction_sample_rate.load(Ordering::SeqCst))
    }

    pub fn new_stream_count(&self) -> usize {
        self.inner.new_streams.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub struct CountingNode {
    probe: CountingProbe,
}

impl CountingNode {
    #[must_use]
    pub fn new(probe: CountingProbe) -> Self {
        Self { probe }
    }
}

impl AudioNode for CountingNode {
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _configuration: &Self::Configuration,
        cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        self.probe
            .inner
            .construction_sample_rate
            .store(cx.stream_info.sample_rate.get(), Ordering::SeqCst);
        self.probe
            .inner
            .constructions
            .fetch_add(1, Ordering::SeqCst);
        CountingProcessor {
            probe: self.probe.clone(),
        }
    }

    fn info(&self, _configuration: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("ring_counting")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::STEREO,
            })
    }
}

struct CountingProcessor {
    probe: CountingProbe,
}

impl AudioNodeProcessor for CountingProcessor {
    fn new_stream(&mut self, _stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.probe.inner.new_streams.fetch_add(1, Ordering::SeqCst);
    }

    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        for output in &mut *buffers.outputs {
            output[..info.frames].fill(0.0);
        }
        ProcessStatus::ClearAllOutputs
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicToneNode;

impl AudioNode for DeterministicToneNode {
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _configuration: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> impl AudioNodeProcessor {
        DeterministicToneProcessor
    }

    fn info(&self, _configuration: &Self::Configuration) -> AudioNodeInfo {
        AudioNodeInfo::new()
            .debug_name("ring_deterministic_tone")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::STEREO,
            })
    }
}

struct DeterministicToneProcessor;

impl AudioNodeProcessor for DeterministicToneProcessor {
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        _events: &mut ProcEvents,
        _extra: &mut ProcExtra,
    ) -> ProcessStatus {
        for frame in 0..info.frames {
            let absolute = info.clock_samples.0 + frame as i64;
            let sample = absolute.rem_euclid(64) as f32 / 128.0 - 0.25;
            for output in &mut *buffers.outputs {
                output[frame] = sample;
            }
        }
        ProcessStatus::OutputsModified
    }
}

pub fn install_stereo_source<N>(
    ctx: &mut FirewheelCtx<RingBackend>,
    node: N,
) -> Result<NodeID, String>
where
    N: AudioNode<Configuration = EmptyConfig> + 'static,
{
    let node_id = ctx.add_node(node, None);
    let graph_out = ctx.graph_out_node_id();
    ctx.connect(node_id, graph_out, &[(0, 0), (1, 1)], false)
        .map_err(|error| format!("connect ring fixture to graph output failed: {error}"))?;
    Ok(node_id)
}
