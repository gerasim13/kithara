use core::{
    f64::consts::{PI, TAU},
    num::NonZeroU32,
};

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
    /// Rise of a click from silence to its peak. A raised-cosine rise and
    /// fall keep the click and its duck band-limited. Between samples the
    /// duck's modulation still folds the mix's content near Nyquist back over
    /// the limiter's true-peak ceiling, the more so the louder the click and
    /// the closer that content sits to Nyquist. Measured with a tone at the
    /// ceiling at 44.1 kHz under a full duck: under a thousandth of a decibel
    /// up to 16 kHz and about a thousandth up to 20 kHz; above 20 kHz, up to
    /// about two hundredths of a decibel at the default level and about four
    /// tenths at a level equal to the ceiling.
    pub(super) const ATTACK_SECONDS: f64 = 0.002;
    /// Fall of a click from its peak back to silence.
    pub(super) const DECAY_SECONDS: f64 = 0.008;
    /// The session transport counts bars of four beats from session beat 0.
    pub(super) const BEATS_PER_BAR: i64 = 4;
    /// Peak of a beat click relative to a downbeat click.
    pub(super) const BEAT_RATIO: f64 = 0.625;
}

/// The downbeat click peak and how deep a click ducks the mix at its own
/// peak. A depth of at least the level over the limiter ceiling keeps the
/// ducked mix plus the click under the ceiling at a sample; a depth of one
/// mutes the mix at the peak of every click.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Duck {
    level: f32,
    depth: f32,
}

impl Duck {
    /// # Errors
    ///
    /// Returns [`PlayError::InvalidParameter`] naming `metronome_level` unless
    /// `0 < level <= ceiling`, and naming `metronome_duck` unless
    /// `level / ceiling <= depth <= 1`.
    pub(crate) fn new(level: f32, depth: f32, ceiling: f32) -> Result<Self, PlayError> {
        let refused = |name: &str, value| PlayError::InvalidParameter {
            name: name.to_owned(),
            value,
        };
        if level > 0.0 && level <= ceiling {
            // WHY: The product of two `f32` values is exact in `f64`, so a
            // depth of exactly the level over the ceiling is kept.
            if depth <= 1.0 && f64::from(depth) * f64::from(ceiling) >= f64::from(level) {
                Ok(Self { level, depth })
            } else {
                Err(refused("metronome_duck", depth))
            }
        } else {
            Err(refused("metronome_level", level))
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

/// One sounding click: a sine tone under a raised-cosine rise and fall,
/// ducking the mix by its own envelope.
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
            duck: f64::from(duck.depth),
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
        let attack = (consts::ATTACK_SECONDS * self.rate).round().max(1.0);
        let decay = (consts::DECAY_SECONDS * self.rate).round().max(1.0);
        let frames = attack + decay;
        let cycles_per_frame = self.hz / self.rate;
        for (l, r) in left.iter_mut().zip(right) {
            if self.elapsed >= frames {
                break;
            }
            let phase = (self.elapsed * cycles_per_frame).fract();
            let envelope = if self.elapsed < attack {
                0.5 * (1.0 - (PI * self.elapsed / attack).cos())
            } else {
                0.5 * (1.0 + (PI * (self.elapsed - attack) / decay).cos())
            };
            self.elapsed += 1.0;
            let gain = (1.0 - self.duck * envelope).to_f32().unwrap_or_default();
            let click = (self.peak * (TAU * phase).sin() * envelope)
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
    /// The next session beat owed a click while the transport runs on from
    /// [`Self::reached`]. A beat is owed by its ordinal, not by the frame it
    /// rounds to: a route restart rounds the beats on the new rate's frame
    /// grid, which can move a beat across the restart frame in either
    /// direction.
    owed: Option<i64>,
    /// The session beat the last rendered block ended on. A block starting
    /// anywhere else follows a seek or a stretch the metronome did not
    /// render, so no beat before it is owed.
    reached: Option<SessionBeat>,
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
    /// click first, then a new click on every owed session beat whose frame
    /// is before the block's end. A block continuing the last one owes the
    /// beats from its owed beat on, clicking one a restart rounded behind the
    /// block on its first frame; any other block owes the beats from its
    /// start on. Returns whether the block was touched.
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
            let continues = self
                .reached
                .is_some_and(|reached| anchor.frame_at(reached).is_ok_and(|frame| frame == start));
            let owed = self.owed.filter(|_| continues);
            let mut ordinal = owed.or_else(|| {
                anchor
                    .beat_at(start)
                    .ok()
                    .and_then(|beat| f64::from(beat).floor().to_i64())
            });
            while let Some(beat) = ordinal {
                let Some(offset) = beat
                    .to_f64()
                    .and_then(|whole| SessionBeat::new(whole).ok())
                    .and_then(|whole| anchor.frame_at(whole).ok())
                    .and_then(|frame| i64::from(frame).checked_sub(first))
                else {
                    break;
                };
                let offset = match usize::try_from(offset) {
                    Ok(offset) => offset,
                    Err(_) if owed.is_some() => 0,
                    Err(_) => {
                        ordinal = beat.checked_add(1);
                        continue;
                    }
                };
                if offset >= left.len() {
                    break;
                }
                ordinal = beat.checked_add(1);
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
            self.owed = ordinal;
            let end = i64::try_from(left.len())
                .ok()
                .and_then(|frames| first.checked_add(frames));
            self.reached = end.and_then(|end| anchor.beat_at(SessionFrame::new(end)).ok());
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
    use core::f32::consts::PI;

    use kithara_effects::mock::reconstructed_peak;

    use super::*;

    #[kithara::test]
    fn a_duck_never_lifts_a_ceiling_signal_over_the_ceiling() {
        const CEILING: f32 = 0.25;
        const FRAMES: usize = 512;

        let rate = NonZeroU32::new(44_100).expect("test rate");
        // WHY: 0.8 is exactly 0.2 over 0.25 in `f32`: the shallowest duck
        // the level allows under the ceiling.
        for depth in [0.8, 1.0] {
            let duck = Duck::new(0.2, depth, CEILING).expect("a duck under the ceiling");
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
                        "a click ducking {depth} over a {level} mix stays under the ceiling"
                    );
                    assert!(
                        left.iter().any(|sample| *sample != level),
                        "the click sounds over the mix"
                    );
                }
            }
        }
    }

    #[kithara::test]
    fn a_click_holds_the_true_peak_to_the_documented_bound() {
        const CEILING: f32 = 0.98;
        // WHY: The overshoot documented on `consts::ATTACK_SECONDS` for mix
        // content up to 16 kHz. Under a full duck a mix held at the ceiling
        // reconstructs about a ten-thousandth of a decibel over it, where the
        // envelope's fall ends on a step in its curvature.
        const DOCUMENTED_OVER_DB: f32 = 0.001;
        const RISE_FRAMES: f32 = 512.0;
        const ONSET: usize = 1_024;
        const FRAMES: u16 = 2_048;
        // WHY: A moving mix under a full-depth duck, far enough under Nyquist
        // that the duck's modulation folds nothing back over the ceiling.
        const TONE_HZ: f64 = 10_000.0;
        const TONE_PHASE: f64 = 2.1;
        const RATE: u16 = 44_100;

        let rate = NonZeroU32::new(u32::from(RATE)).expect("test rate");
        let held: Vec<f32> = (0..FRAMES)
            .map(|frame| {
                let rise = (f32::from(frame) / RISE_FRAMES).min(1.0);
                CEILING * 0.5 * (1.0 - (PI * rise).cos())
            })
            .collect();
        let tone: Vec<f32> = held
            .iter()
            .zip(0..FRAMES)
            .map(|(level, frame)| {
                let cycles = (f64::from(frame) * TONE_HZ / f64::from(RATE)).fract();
                let tone = f64::from(*level) * TAU.mul_add(cycles, TONE_PHASE).cos();
                tone.to_f32().expect("a tone sample fits f32")
            })
            .collect();
        let silence = vec![0.0; held.len()];
        let held_again = held.clone();
        for (mix, level, depth) in [
            (silence, CEILING, 1.0),
            (
                held,
                crate::consts::DEFAULT_METRONOME_LEVEL,
                crate::consts::DEFAULT_METRONOME_DUCK,
            ),
            // WHY: 0.5 is exactly half the ceiling over the ceiling in
            // `f32`: the shallowest duck that level allows.
            (held_again, CEILING / 2.0, 0.5),
            (tone, CEILING, 1.0),
        ] {
            let duck = Duck::new(level, depth, CEILING).expect("a duck under the ceiling");
            for downbeat in [true, false] {
                let mut left = mix.clone();
                let mut right = mix.clone();
                Click::new(downbeat, rate, duck).render(
                    left.get_mut(ONSET..).expect("onset inside the mix"),
                    right.get_mut(ONSET..).expect("onset inside the mix"),
                );
                let peak = reconstructed_peak(&left);
                let over_db = 20.0 * (peak / CEILING).log10();
                assert!(
                    over_db <= DOCUMENTED_OVER_DB,
                    "a click at {level} ducking {depth} reconstructs to {peak}, {over_db} dB over the ceiling"
                );
            }
        }
    }

    /// A 120 BPM transport at 44.1 kHz playing session beat `beat` on
    /// session frame `frame`: a beat every 22 050 frames.
    fn transport(frame: i64, beat: f64) -> SessionAnchor {
        const BEATS_PER_SECOND: f64 = 2.0;
        SessionAnchor::new(
            SessionFrame::new(frame),
            SessionBeat::new(beat).expect("finite beat"),
            BEATS_PER_SECOND,
            NonZeroU32::new(44_100).expect("test rate"),
        )
        .expect("a positive tempo")
    }

    /// A metronome that clicked session beat 1 and rendered its click out:
    /// one block of silence around the beat. Returns the frame after it.
    fn past_beat_one(metronome: &mut Metronome, duck: Duck) -> i64 {
        const LEAD: i64 = 256;
        const FRAMES: usize = 1_024;
        let beat_one = 22_050;
        let start = beat_one - LEAD;
        let mut left = [0.0; FRAMES];
        let mut right = [0.0; FRAMES];
        metronome.render(
            Some(transport(0, 0.0)),
            SessionFrame::new(start),
            duck,
            &mut left,
            &mut right,
        );
        assert!(
            left.iter().any(|sample| *sample != 0.0) && !metronome.sounding(),
            "beat 1 clicks and its click ends inside the block"
        );
        start + i64::try_from(FRAMES).expect("block length")
    }

    #[kithara::test]
    fn a_seek_forward_clicks_none_of_the_beats_it_jumps_over() {
        const TARGET: f64 = 40.25;
        let duck = Duck::new(
            crate::consts::DEFAULT_METRONOME_LEVEL,
            crate::consts::DEFAULT_METRONOME_DUCK,
            0.98,
        )
        .expect("default duck");
        let mut metronome = Metronome::default();
        let seek = past_beat_one(&mut metronome, duck);

        let mut left = [0.0; 1_024];
        let mut right = [0.0; 1_024];
        let touched = metronome.render(
            Some(transport(seek, TARGET)),
            SessionFrame::new(seek),
            duck,
            &mut left,
            &mut right,
        );

        assert!(
            !touched && left.iter().chain(&right).all(|sample| *sample == 0.0),
            "no beat lies between the seek target and the block's end"
        );
    }

    #[kithara::test]
    fn a_seek_backward_clicks_the_beats_it_plays_again() {
        const TARGET: f64 = 0.99;
        let duck = Duck::new(
            crate::consts::DEFAULT_METRONOME_LEVEL,
            crate::consts::DEFAULT_METRONOME_DUCK,
            0.98,
        )
        .expect("default duck");
        let mut metronome = Metronome::default();
        let seek = past_beat_one(&mut metronome, duck);
        let anchor = transport(seek, TARGET);
        let beat_one = anchor
            .frame_at(SessionBeat::new(1.0).expect("whole beat"))
            .expect("beat 1 after the seek");
        let offset = usize::try_from(i64::from(beat_one) - seek).expect("beat 1 inside the block");

        let mut left = [0.0; 1_024];
        let mut right = [0.0; 1_024];
        metronome.render(
            Some(anchor),
            SessionFrame::new(seek),
            duck,
            &mut left,
            &mut right,
        );

        assert_eq!(
            left.iter().position(|sample| *sample != 0.0),
            Some(offset + 1),
            "beat 1 clicks again, rising from its frame after the seek"
        );
    }

    #[kithara::test]
    fn a_metronome_level_sits_above_zero_and_at_most_the_ceiling() {
        let refused = |level| {
            matches!(
                Duck::new(level, 1.0, 0.98),
                Err(PlayError::InvalidParameter { name, .. }) if name == "metronome_level"
            )
        };
        assert!(refused(f32::NAN), "NaN");
        assert!(refused(0.0), "zero");
        assert!(refused(0.99), "over the ceiling");
        assert!(Duck::new(0.98, 1.0, 0.98).is_ok(), "at the ceiling");
    }

    #[kithara::test]
    fn a_metronome_duck_sits_between_the_level_over_the_ceiling_and_one() {
        let refused = |depth| {
            matches!(
                Duck::new(0.49, depth, 0.98),
                Err(PlayError::InvalidParameter { name, .. }) if name == "metronome_duck"
            )
        };
        assert!(refused(f32::NAN), "NaN");
        assert!(
            refused(0.49),
            "too shallow to keep the click under the ceiling"
        );
        assert!(
            Duck::new(0.49, 0.5, 0.98).is_ok(),
            "the level over the ceiling"
        );
        assert!(Duck::new(0.49, 1.0, 0.98).is_ok(), "a full duck");
        assert!(refused(1.01), "deeper than muting the mix");
    }
}
