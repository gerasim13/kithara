use core::num::NonZeroU32;

use firewheel::{
    StreamInfo,
    channel_config::{ChannelConfig, ChannelCount},
    node::{
        AudioNode, AudioNodeInfo, AudioNodeProcessor, ConstructProcessorContext, EmptyConfig,
        NodeError, ProcBuffers, ProcExtra, ProcInfo, ProcStreamCtx, ProcessStatus,
    },
};
use kithara_play::rt::read_render_context;
use kithara_signal::SessionFrame;
use kithara_test_utils::kithara;
use kithara_warp::{SessionAnchor, SessionBeat};
use num_traits::ToPrimitive;

mod consts {
    pub(super) const BEAT_HZ: f64 = 1_760.0;
    pub(super) const DOWNBEAT_HZ: f64 = 2_200.0;
    pub(super) const BEAT_LEVEL: f64 = 0.45;
    pub(super) const DOWNBEAT_LEVEL: f64 = 0.72;
    pub(super) const CLICK_SECONDS: f64 = 0.01;
    /// The session transport counts bars of four beats from session beat 0.
    pub(super) const BEATS_PER_BAR: i64 = 4;
}

/// The Host click track: one decaying saw burst starting on the frame of every
/// whole session beat, higher and louder on the first beat of each bar.
///
/// It keeps no clock of its own. Each render reads the beats from the session
/// trajectory the caller passes for that block, so a click always sounds where
/// the transport places its beat.
#[derive(Debug, Default)]
pub struct Metronome {
    click: Option<Click>,
}

/// One click, counted in frames of the stream it sounds in.
#[derive(Clone, Copy, Debug)]
struct Click {
    elapsed: f64,
    rate: f64,
    hz: f64,
    level: f64,
}

impl Click {
    fn new(downbeat: bool, sample_rate: NonZeroU32) -> Self {
        let (hz, level) = if downbeat {
            (consts::DOWNBEAT_HZ, consts::DOWNBEAT_LEVEL)
        } else {
            (consts::BEAT_HZ, consts::BEAT_LEVEL)
        };
        Self {
            elapsed: 0.0,
            rate: f64::from(sample_rate.get()),
            hz,
            level,
        }
    }

    /// Writes the next samples of this click into `out`, then silence once it
    /// has ended. Returns whether it is still sounding after `out`.
    fn render(&mut self, out: &mut [f32]) -> bool {
        let frames = (consts::CLICK_SECONDS * self.rate).round();
        let cycles_per_frame = self.hz / self.rate;
        for sample in out {
            *sample = if self.elapsed < frames {
                let phase = (self.elapsed * cycles_per_frame).fract();
                let envelope = 1.0 - self.elapsed / frames;
                self.elapsed += 1.0;
                (self.level * phase.mul_add(2.0, -1.0) * envelope)
                    .to_f32()
                    .unwrap_or_default()
            } else {
                0.0
            };
        }
        self.elapsed < frames
    }

    /// Counts this click in frames of a stream at `rate`: the time it has
    /// sounded and its pitch stay.
    fn retune(&mut self, rate: f64) {
        self.elapsed *= rate / self.rate;
        self.rate = rate;
    }
}

impl Metronome {
    /// Renders the clicks sounding over the session frames from `start` into
    /// the mono `out`, one sample per frame. `trajectory` is the transport's
    /// beat map for these frames, or `None` while the transport is stopped:
    /// then only a click already sounding finishes. Returns whether any sample
    /// is non-silent.
    pub fn render(
        &mut self,
        trajectory: Option<SessionAnchor>,
        start: SessionFrame,
        out: &mut [f32],
    ) -> bool {
        let sounding = self.click.is_some();
        let mut cursor = 0;
        let mut started = false;
        if let Some(anchor) = trajectory {
            let first = i64::from(start);
            let mut ordinal = anchor
                .beat_at(start)
                .ok()
                .and_then(|beat| f64::from(beat).floor().to_i64());
            while let Some(beat) = ordinal {
                ordinal = beat.checked_add(1);
                let Some(offset) = beat
                    .to_f64()
                    .and_then(|beat| SessionBeat::new(beat).ok())
                    .and_then(|beat| anchor.frame_at(beat).ok())
                    .and_then(|frame| i64::from(frame).checked_sub(first))
                else {
                    break;
                };
                let Ok(offset) = usize::try_from(offset) else {
                    continue;
                };
                if offset >= out.len() {
                    break;
                }
                let Some(pending) = out.get_mut(cursor..offset) else {
                    break;
                };
                self.continue_click(pending);
                self.click = Some(Click::new(
                    beat.rem_euclid(consts::BEATS_PER_BAR) == 0,
                    anchor.sample_rate(),
                ));
                cursor = offset;
                started = true;
            }
        }
        if let Some(rest) = out.get_mut(cursor..) {
            self.continue_click(rest);
        }
        sounding || started
    }

    /// Carries a sounding click over to a stream at `sample_rate`, so it
    /// keeps its pitch and ends when it would have.
    pub(crate) fn retune(&mut self, sample_rate: NonZeroU32) {
        if let Some(click) = self.click.as_mut() {
            click.retune(f64::from(sample_rate.get()));
        }
    }

    fn continue_click(&mut self, out: &mut [f32]) {
        match self.click.as_mut() {
            Some(click) => {
                if !click.render(out) {
                    self.click = None;
                }
            }
            None => out.fill(0.0),
        }
    }
}

/// The session graph's metronome source: [`Metronome`] rendered from the
/// render context the session transport publishes for each block.
pub(crate) struct MetronomeNode;

impl AudioNode for MetronomeNode {
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        Ok(MetronomeProcessor::default())
    }

    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("session_metronome")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: ChannelCount::STEREO,
            }))
    }
}

#[derive(Default)]
struct MetronomeProcessor {
    metronome: Metronome,
}

impl AudioNodeProcessor for MetronomeProcessor {
    fn new_stream(&mut self, stream_info: &StreamInfo, _context: &mut ProcStreamCtx) {
        self.metronome.retune(stream_info.sample_rate);
    }

    #[kithara::rtsan_forbid_blocking]
    fn process(
        &mut self,
        info: &ProcInfo,
        buffers: ProcBuffers,
        extra: &mut ProcExtra,
    ) -> ProcessStatus {
        let trajectory = read_render_context(&extra.store, info)
            .ok()
            .and_then(|context| context.trajectory().copied());
        let [left, right, ..] = buffers.outputs else {
            return ProcessStatus::ClearAllOutputs;
        };
        let (Some(left), Some(right)) = (left.get_mut(..info.frames), right.get_mut(..info.frames))
        else {
            return ProcessStatus::ClearAllOutputs;
        };
        let start = SessionFrame::new(info.clock_samples.0);
        if !self.metronome.render(trajectory, start, left) {
            return ProcessStatus::ClearAllOutputs;
        }
        right.copy_from_slice(left);
        ProcessStatus::OutputsModified
    }
}
