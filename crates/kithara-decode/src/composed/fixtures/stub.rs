use kithara_bufpool::SampleBuffer;
use kithara_platform::time::Duration;
use kithara_signal::AudioSpec;

use crate::{codec::FrameCodec, error::DecodeResult};

pub(in crate::composed) struct ConstFrameCodec {
    spec: AudioSpec,
    pcm: Vec<f32>,
    frames_per_call: u32,
}

pub(in crate::composed) struct LaggedQueueCodec {
    spec: AudioSpec,
    decoded_pts: Duration,
    pending_pts: Option<Duration>,
    pcm: Vec<f32>,
    frames_per_call: u32,
}

impl ConstFrameCodec {
    pub(in crate::composed) fn new(pcm: Vec<f32>, spec: AudioSpec, frames_per_call: u32) -> Self {
        Self {
            spec,
            pcm,
            frames_per_call,
        }
    }
}

impl LaggedQueueCodec {
    pub(in crate::composed) fn new(pcm: Vec<f32>, spec: AudioSpec, frames_per_call: u32) -> Self {
        Self {
            pcm,
            spec,
            frames_per_call,
            pending_pts: None,
            decoded_pts: Duration::ZERO,
        }
    }
}

impl FrameCodec for ConstFrameCodec {
    fn decode_frame(
        &mut self,
        _bytes: &[u8],
        _pts: Duration,
        _packet_desc: &[u8],
        out: &mut SampleBuffer,
    ) -> DecodeResult<u32> {
        super::write_silent_test_frame(&self.pcm, self.spec, self.frames_per_call, out)
    }

    fn flush(&mut self) -> DecodeResult<()> {
        Ok(())
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }
}

impl FrameCodec for LaggedQueueCodec {
    fn decode_frame(
        &mut self,
        _bytes: &[u8],
        pts: Duration,
        _packet_desc: &[u8],
        out: &mut SampleBuffer,
    ) -> DecodeResult<u32> {
        let Some(decoded_pts) = self.pending_pts.replace(pts) else {
            out.clear();
            return Ok(0);
        };
        self.decoded_pts = decoded_pts;
        super::write_silent_test_frame(&self.pcm, self.spec, self.frames_per_call, out)
    }

    fn decoded_pts(&self) -> Option<Duration> {
        Some(self.decoded_pts)
    }

    fn flush(&mut self) -> DecodeResult<()> {
        self.pending_pts = None;
        Ok(())
    }

    fn spec(&self) -> AudioSpec {
        self.spec
    }
}
