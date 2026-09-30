use core::num::NonZeroU32;

use firewheel::{
    StreamInfo,
    channel_config::{ChannelConfig, ChannelCount},
    diff::{Diff, Patch},
    event::ProcEvents,
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

use crate::PlayError;

mod consts {
    /// Tone of a beat click.
    pub(super) const BEAT_HZ: f64 = 1_760.0;
    /// Tone of a downbeat click.
    pub(super) const DOWNBEAT_HZ: f64 = 2_200.0;
    /// How long one click sounds.
    pub(super) const CLICK_SECONDS: f64 = 0.01;
    /// The session transport counts bars of four beats from session beat 0.
    pub(super) const BEATS_PER_BAR: i64 = 4;
    /// Peak of a beat click relative to a downbeat click.
    pub(super) const BEAT_RATIO: f64 = 0.625;
}

/// The downbeat click peak and the limiter ceiling the click ducks the mix
/// under: the ducked mix plus the click never exceeds the ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Duck {
    level: f32,
    ceiling: f32,
}

impl Duck {
    /// # Errors
    ///
    /// Returns [`PlayError::InvalidParameter`] unless `0 < level <= ceiling`.
    pub(crate) fn new(level: f32, ceiling: f32) -> Result<Self, PlayError> {
        if level > 0.0 && level <= ceiling {
            Ok(Self { level, ceiling })
        } else {
            Err(PlayError::InvalidParameter {
                name: "metronome_level".to_owned(),
                value: level,
            })
        }
    }
}

/// The Host metronome between the limiter and `graph_out`: a click on every
/// session beat while enabled and the transport runs.
#[derive(Diff, Patch, Debug, Clone, Copy, PartialEq)]
pub(crate) struct MetronomeNode {
    pub(crate) enabled: bool,
    #[diff(skip)]
    duck: Duck,
}

impl MetronomeNode {
    pub(crate) const fn new(enabled: bool, duck: Duck) -> Self {
        Self { enabled, duck }
    }
}

/// One sounding click: a falling saw under a linear decay, ducking the mix
/// by its own envelope.
#[derive(Clone, Copy, Debug)]
struct Click {
    elapsed: f64,
    rate: f64,
    hz: f64,
    peak: f64,
    duck: f64,
}

impl Click {
    fn new(downbeat: bool, sample_rate: NonZeroU32, duck: Duck) -> Self {
        let level = f64::from(duck.level);
        let (hz, peak) = if downbeat {
            (consts::DOWNBEAT_HZ, level)
        } else {
            (consts::BEAT_HZ, level * consts::BEAT_RATIO)
        };
        Self {
            elapsed: 0.0,
            rate: f64::from(sample_rate.get()),
            hz,
            peak,
            duck: peak / f64::from(duck.ceiling),
        }
    }

    /// Keeps the click's elapsed time, not its frame count, across a rate change.
    fn retune(&mut self, sample_rate: NonZeroU32) {
        let rate = f64::from(sample_rate.get());
        self.elapsed *= rate / self.rate;
        self.rate = rate;
    }

    /// Ducks `left`/`right` under the click and adds it. Returns whether the
    /// click still sounds after these frames.
    fn render(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        let frames = (consts::CLICK_SECONDS * self.rate).round();
        let cycles_per_frame = self.hz / self.rate;
        for (l, r) in left.iter_mut().zip(right) {
            if self.elapsed >= frames {
                break;
            }
            let phase = (self.elapsed * cycles_per_frame).fract();
            let envelope = 1.0 - self.elapsed / frames;
            self.elapsed += 1.0;
            let gain = (1.0 - self.duck * envelope).to_f32().unwrap_or_default();
            let click = (self.peak * phase.mul_add(2.0, -1.0) * envelope)
                .to_f32()
                .unwrap_or_default();
            *l = l.mul_add(gain, click);
            *r = r.mul_add(gain, click);
        }
        self.elapsed < frames
    }
}

#[derive(Debug, Default)]
struct Metronome {
    click: Option<Click>,
}

impl Metronome {
    const fn sounding(&self) -> bool {
        self.click.is_some()
    }

    fn retune(&mut self, sample_rate: NonZeroU32) {
        if let Some(click) = self.click.as_mut() {
            click.retune(sample_rate);
        }
    }

    /// Carries the sounding click over `left`/`right`; with none, the mix
    /// passes untouched.
    fn continue_click(&mut self, left: &mut [f32], right: &mut [f32]) {
        if let Some(click) = self.click.as_mut()
            && !click.render(left, right)
        {
            self.click = None;
        }
    }

    /// Renders one block starting at session frame `start`: the sounding
    /// click first, then a new click on every whole session beat inside the
    /// block. Returns whether the block was touched.
    fn render(
        &mut self,
        trajectory: Option<SessionAnchor>,
        start: SessionFrame,
        duck: Duck,
        left: &mut [f32],
        right: &mut [f32],
    ) -> bool {
        let sounding = self.sounding();
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
                    .and_then(|whole| SessionBeat::new(whole).ok())
                    .and_then(|whole| anchor.frame_at(whole).ok())
                    .and_then(|frame| i64::from(frame).checked_sub(first))
                else {
                    break;
                };
                let Ok(offset) = usize::try_from(offset) else {
                    continue;
                };
                if offset >= left.len() {
                    break;
                }
                let (Some(left_run), Some(right_run)) =
                    (left.get_mut(cursor..offset), right.get_mut(cursor..offset))
                else {
                    break;
                };
                self.continue_click(left_run, right_run);
                self.click = Some(Click::new(
                    beat.rem_euclid(consts::BEATS_PER_BAR) == 0,
                    anchor.sample_rate(),
                    duck,
                ));
                cursor = offset;
                started = true;
            }
        }
        if let (Some(left_rest), Some(right_rest)) =
            (left.get_mut(cursor..), right.get_mut(cursor..))
        {
            self.continue_click(left_rest, right_rest);
        }
        sounding || started
    }
}

impl AudioNode for MetronomeNode {
    type Configuration = EmptyConfig;

    fn construct_processor(
        &self,
        _config: &Self::Configuration,
        _cx: ConstructProcessorContext,
    ) -> Result<impl AudioNodeProcessor, NodeError> {
        Ok(MetronomeProcessor {
            params: *self,
            metronome: Metronome::default(),
        })
    }

    fn info(&self, _config: &Self::Configuration) -> Result<AudioNodeInfo, NodeError> {
        Ok(AudioNodeInfo::new()
            .debug_name("session_metronome")
            .channel_config(ChannelConfig {
                num_inputs: ChannelCount::STEREO,
                num_outputs: ChannelCount::STEREO,
            }))
    }
}

struct MetronomeProcessor {
    params: MetronomeNode,
    metronome: Metronome,
}

impl AudioNodeProcessor for MetronomeProcessor {
    #[kithara::rtsan_forbid_blocking]
    fn events(&mut self, _info: &ProcInfo, events: &mut ProcEvents, _extra: &mut ProcExtra) {
        for patch in events.drain_patches::<MetronomeNode>() {
            self.params.apply(patch);
        }
    }

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
        let trajectory = if self.params.enabled {
            read_render_context(&extra.store, info)
                .ok()
                .and_then(|context| context.trajectory().copied())
        } else {
            None
        };
        if trajectory.is_none() && !self.metronome.sounding() {
            return ProcessStatus::Bypass;
        }
        let frames = info.frames;
        let ([in_left, in_right, ..], [out_left, out_right, ..]) =
            (buffers.inputs, buffers.outputs)
        else {
            return ProcessStatus::Bypass;
        };
        let (Some(in_left), Some(in_right), Some(out_left), Some(out_right)) = (
            in_left.get(..frames),
            in_right.get(..frames),
            out_left.get_mut(..frames),
            out_right.get_mut(..frames),
        ) else {
            return ProcessStatus::Bypass;
        };
        out_left.copy_from_slice(in_left);
        out_right.copy_from_slice(in_right);
        self.metronome.render(
            trajectory,
            SessionFrame::new(info.clock_samples.0),
            self.params.duck,
            out_left,
            out_right,
        );
        ProcessStatus::OutputsModified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[kithara::test]
    fn a_duck_never_lifts_a_ceiling_signal_over_the_ceiling() {
        const CEILING: f32 = 0.25;
        const FRAMES: usize = 512;

        let duck = Duck::new(0.2, CEILING).expect("level under the ceiling");
        let rate = NonZeroU32::new(44_100).expect("test rate");
        for downbeat in [true, false] {
            for level in [CEILING, -CEILING] {
                let mut left = [level; FRAMES];
                let mut right = [level; FRAMES];
                Click::new(downbeat, rate, duck).render(&mut left, &mut right);
                let bound = CEILING * (1.0 + 4.0 * f32::EPSILON);
                assert!(
                    left.iter()
                        .chain(&right)
                        .all(|sample| sample.abs() <= bound),
                    "a click over a {level} mix stays under the ceiling"
                );
                assert!(
                    left.iter().any(|sample| *sample != level),
                    "the click sounds over the mix"
                );
            }
        }
    }

    #[kithara::test]
    fn a_metronome_level_sits_above_zero_and_at_most_the_ceiling() {
        assert!(Duck::new(f32::NAN, 0.98).is_err(), "NaN");
        assert!(Duck::new(0.0, 0.98).is_err(), "zero");
        assert!(Duck::new(0.99, 0.98).is_err(), "over the ceiling");
        assert!(Duck::new(0.98, 0.98).is_ok(), "at the ceiling");
    }
}
