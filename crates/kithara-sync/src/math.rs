use std::{num::NonZeroU32, time::Duration};

use kithara_beat::BeatGridModel;
use kithara_signal::SessionFrame;
use kithara_warp::{MIN_SPEED, SessionBeat, SpeedCurve};
use num_traits::ToPrimitive;

use crate::{Bound, TempoTrajectory};

/// Track-media seconds ahead of the Host phase, modulo `period` media seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseError {
    pub seconds: f64,
    pub period: f64,
    /// One track beat in media seconds; correction ignores whole-beat offsets.
    pub beat_seconds: f64,
}

impl PhaseError {
    fn shortest(self) -> f64 {
        let phase = self.seconds.rem_euclid(self.beat_seconds);
        if phase > self.beat_seconds / 2.0 {
            phase - self.beat_seconds
        } else {
            phase
        }
    }
}

/// One speed and its residence time in output seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CorrectionStep {
    pub speed: f32,
    pub seconds: f64,
}

/// A staircase whose integrated speed difference cancels the phase error.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionPlan {
    pub steps: Vec<CorrectionStep>,
}

impl CorrectionPlan {
    /// The renderer curve that replaces the remaining correction in one command.
    /// Residence times are rounded on the output rate; every adjacent step keeps
    /// a distinct frame, including a zero-duration step between bounded speeds.
    ///
    /// # Panics
    /// Panics for a non-finite or unplayable speed, a non-finite or negative
    /// residence, or a cumulative duration or distinct frame offset that is
    /// not representable.
    #[must_use]
    pub fn curve(&self, sample_rate: NonZeroU32) -> SpeedCurve {
        match self.checked_curve(sample_rate) {
            Some((curve, _frames)) => curve,
            None => panic!("correction plan is not representable in output frames"),
        }
    }

    /// Returns the curve and its last frame offset, or `None` if unrepresentable.
    #[must_use]
    pub fn checked_curve(&self, sample_rate: NonZeroU32) -> Option<(SpeedCurve, u64)> {
        let steps = self.frame_steps(sample_rate).collect::<Option<Vec<_>>>()?;
        let frames = steps.last().map_or(0, |(frame, _speed)| *frame);
        Some((SpeedCurve::Steps(steps.into()), frames))
    }

    fn frame_steps(
        &self,
        sample_rate: NonZeroU32,
    ) -> impl Iterator<Item = Option<(u64, f32)>> + '_ {
        let mut seconds = 0.0;
        let mut previous: Option<u64> = None;
        self.steps.iter().map(move |step| {
            if !step.speed.is_finite()
                || step.speed < MIN_SPEED
                || !step.seconds.is_finite()
                || step.seconds < 0.0
            {
                return None;
            }
            let next_seconds = seconds + step.seconds;
            let duration = next_seconds * f64::from(sample_rate.get());
            let rounded = (seconds * f64::from(sample_rate.get())).round();
            let limit = u64::MAX.to_f64()?;
            if !duration.is_finite()
                || duration >= limit
                || !rounded.is_finite()
                || rounded < 0.0
                || rounded >= limit
            {
                return None;
            }
            let rounded = rounded.to_u64()?;
            let frame = match previous {
                Some(frame) => rounded.max(frame.checked_add(1)?),
                None => rounded,
            };
            previous = Some(frame);
            seconds = next_seconds;
            Some((frame, step.speed))
        })
    }
}

/// Whether a position lies inside the grid's inclusive observed beat span.
#[must_use]
pub fn covers(grid: &BeatGridModel, position: Duration) -> bool {
    let raw = grid.as_raw();
    match (raw.beats.first(), raw.beats.last()) {
        (Some(first), Some(last)) => {
            match (
                Duration::try_from_secs_f64(first.at),
                Duration::try_from_secs_f64(last.at),
            ) {
                (Ok(first), Ok(last)) => (first..=last).contains(&position),
                _ => false,
            }
        }
        _ => false,
    }
}

/// The cue's beat phase, interpolated between observed beat ordinals.
fn phase_at(grid: &BeatGridModel, position: Duration) -> Option<f64> {
    if !covers(grid, position) {
        return None;
    }
    let raw = grid.as_raw();
    let index = raw.beats.partition_point(|beat| {
        Duration::try_from_secs_f64(beat.at).is_ok_and(|at| at <= position)
    });
    let before = raw.beats.get(index.checked_sub(1)?)?;
    let fraction = match raw.beats.get(index) {
        Some(after) => {
            let before_at = Duration::try_from_secs_f64(before.at).ok()?;
            let after_at = Duration::try_from_secs_f64(after.at).ok()?;
            let ordinals = (i128::from(after.ordinal) - i128::from(before.ordinal)).to_f64()?;
            (position - before_at).as_secs_f64() / (after_at - before_at).as_secs_f64()
                * ordinals
        }
        None => 0.0,
    };
    let (origin, stride) = raw.meter.map_or((0, 1), |meter| {
        (meter.origin_beat_ordinal, meter.beats_per_bar.get())
    });
    let whole = (i128::from(before.ordinal) - i128::from(origin))
        .rem_euclid(i128::from(stride))
        .to_f64()?;
    Some((whole + fraction).rem_euclid(f64::from(stride)))
}

/// Track speed at a frame, as Host BPM divided by analyzed track BPM.
///
/// # Panics
/// Panics if the numeric conversion cannot produce an output speed.
#[must_use]
pub fn speed(host: &TempoTrajectory, grid: &BeatGridModel, at: SessionFrame) -> f32 {
    (host.tempo_at(at).beats_per_minute() / grid.as_raw().bpm)
        .to_f32()
        .unwrap_or_else(|| unreachable!("f64 values can be converted to f32"))
}

/// The nearest in-phase entry on the requested side of a frame.
///
/// On constant tempo this is a Host boundary plus the media offset divided
/// by speed; beat inversion also accounts for intervening Host tempo steps.
/// Candidates are compared on rounded session frames, inclusively on both sides.
#[must_use]
pub fn entry(
    host: &TempoTrajectory,
    grid: &BeatGridModel,
    position: Duration,
    bound: Bound,
) -> Option<SessionFrame> {
    let offset = phase_at(grid, position)?;
    let stride = if grid.as_raw().meter.is_some() {
        host.beats_per_bar()
    } else {
        1.0
    };
    let (frame, after) = match bound {
        Bound::AtOrAfter(frame) => (frame, true),
        Bound::AtOrBefore(frame) => (frame, false),
    };
    let direction = if after { 1.0 } else { -1.0 };
    let admitted = |candidate| {
        if after { candidate >= frame } else { candidate <= frame }
    };
    let frame_at = |ordinal| {
        let beat = SessionBeat::new(ordinal * stride + offset).ok()?;
        host.try_frame_at(beat)
    };
    let mut ordinal = ((f64::from(host.beat_at(frame)) - offset) / stride).floor();
    let mut candidate = frame_at(ordinal)?;
    while !admitted(candidate) {
        let next = ordinal + direction;
        if next == ordinal {
            return None;
        }
        ordinal = next;
        candidate = frame_at(ordinal)?;
    }
    loop {
        let next = ordinal - direction;
        if next == ordinal {
            break;
        }
        let Some(closer) = frame_at(next) else {
            break;
        };
        if !admitted(closer) {
            break;
        }
        ordinal = next;
        candidate = closer;
    }
    Some(candidate)
}

/// Phase ahead of the Host, modulo a Host bar or one beat without track meter.
///
/// # Panics
/// Panics unless the track grid covers the position; callers wait for analysis first.
#[must_use]
pub fn phase_error(
    host: &TempoTrajectory,
    grid: &BeatGridModel,
    position: Duration,
    at: SessionFrame,
) -> PhaseError {
    let phase = phase_at(grid, position)
        .unwrap_or_else(|| panic!("phase requires a covering track grid"));
    let stride = if grid.as_raw().meter.is_some() {
        host.beats_per_bar()
    } else {
        1.0
    };
    let seconds_per_beat = 60.0 / grid.as_raw().bpm;
    let period = stride * seconds_per_beat;
    let host_phase = f64::from(host.beat_at(at)).rem_euclid(stride);
    PhaseError {
        seconds: (phase - host_phase).rem_euclid(stride) * seconds_per_beat,
        period,
        beat_seconds: seconds_per_beat,
    }
}

/// The nearer valid media position among the preceding and following phase matches.
///
/// # Panics
/// Panics if the phase period is not finite and positive, or the target is unrepresentable.
#[must_use]
pub fn jump_target(position: Duration, error: PhaseError) -> Duration {
    assert!(error.seconds.is_finite() && error.period.is_finite() && error.period > 0.0);
    let phase = error.seconds.rem_euclid(error.period);
    let backwards = Duration::from_secs_f64(phase);
    if phase <= error.period / 2.0
        && let Some(target) = position.checked_sub(backwards)
    {
        target
    } else {
        position + Duration::from_secs_f64(error.period - phase)
    }
}

/// Bounded speed steps, ending at `to` with zero integrated phase error.
///
/// Slew steps reside for one output second each. A final bounded excursion
/// cancels their integral and the shortest phase error before restoring `to`.
///
/// # Panics
/// Panics for invalid speeds, phase or epsilon, or when no representable
/// correcting speed exists within epsilon and the renderer's speed range.
#[must_use]
pub fn correction(from: f32, to: f32, error: PhaseError, epsilon: f32) -> CorrectionPlan {
    checked_correction(from, to, error, epsilon).unwrap_or_else(|| {
        panic!("correction requires finite inputs and representable bounded speed steps")
    })
}

/// Returns bounded correction steps, or `None` for invalid or unrepresentable inputs.
#[must_use]
pub fn checked_correction(
    from: f32,
    to: f32,
    error: PhaseError,
    epsilon: f32,
) -> Option<CorrectionPlan> {
    if !from.is_finite()
        || from < MIN_SPEED
        || !to.is_finite()
        || to < MIN_SPEED
        || !epsilon.is_finite()
        || epsilon <= 0.0
        || !error.seconds.is_finite()
        || !error.period.is_finite()
        || error.period <= 0.0
        || !error.beat_seconds.is_finite()
        || error.beat_seconds <= 0.0
    {
        return None;
    }
    let mut steps = Vec::new();
    let mut current = from;
    let mut remaining = error.shortest();
    while current != to {
        current = step_towards(current, to, epsilon)?;
        let seconds = if current == to { 0.0 } else { 1.0 };
        remaining += (f64::from(current) - f64::from(to)) * seconds;
        if !remaining.is_finite() {
            return None;
        }
        steps.push(CorrectionStep {
            speed: current,
            seconds,
        });
    }
    if remaining != 0.0 {
        let target = to - remaining.signum().to_f32()? * epsilon;
        if !target.is_finite() || target < MIN_SPEED {
            return None;
        }
        let adjusted = step_towards(to, target, epsilon)?;
        let seconds = -remaining / (f64::from(adjusted) - f64::from(to));
        if !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        steps.push(CorrectionStep {
            speed: adjusted,
            seconds,
        });
    }
    steps.push(CorrectionStep {
        speed: to,
        seconds: 0.0,
    });
    Some(CorrectionPlan { steps })
}

fn step_towards(from: f32, to: f32, epsilon: f32) -> Option<f32> {
    let shift = (to - from).clamp(-epsilon, epsilon);
    let mut next = from + shift;
    if (next - from).abs() > epsilon {
        next = if shift > 0.0 {
            next.next_down()
        } else {
            next.next_up()
        };
    }
    (next.is_finite() && next >= MIN_SPEED && next != from && (next - from).abs() <= epsilon)
        .then_some(next)
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use kithara_beat::{BeatGridModel, BeatGridState, GridBeat, GridDownbeat, Meter, RawBeatGrid};
    use std::time::Duration as Position;

    use crate::Tempo;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::TempoStep;

    mod fixtures;
    mod preparation;
    mod relocation;

    #[kithara::test]
    fn entry_family_uses_the_media_offset_and_the_requested_side() {
        let host = TempoTrajectory::new(
            TempoStep {
                frame: SessionFrame::new(0),
                beat: SessionBeat::default(),
                tempo: Tempo::DEFAULT,
            },
            NonZeroU16::new(4).expect("meter"),
            NonZeroU32::new(48_000).expect("rate"),
        );
        let model = BeatGridModel::try_from(RawBeatGrid {
            state: BeatGridState::Final,
            duration: Some(8.0),
            meter: Some(Meter {
                beats_per_bar: NonZeroU16::new(4).expect("meter"),
                origin_beat_ordinal: 0,
            }),
            model_id: "constant".into(),
            beats: (0..=8)
                .map(|ordinal| GridBeat {
                    confidence: None,
                    at: ordinal.to_f64().expect("fixture beat ordinal fits f64"),
                    ordinal,
                })
                .collect(),
            downbeats: [0, 4, 8]
                .into_iter()
                .map(|beat_ordinal| GridDownbeat {
                    confidence: None,
                    at: beat_ordinal.to_f64().expect("fixture downbeat ordinal fits f64"),
                    beat_ordinal,
                })
                .collect(),
            bpm: 60.0,
            schema_version: 1,
            revision: 0,
        })
        .expect("consistent grid");
        let grid = model;
        let position = Position::from_secs_f64(0.5);
        assert_eq!(speed(&host, &grid, SessionFrame::new(0)), 2.0);
        assert_eq!(
            entry(
                &host,
                &grid,
                position,
                Bound::AtOrAfter(SessionFrame::new(20_000))
            ),
            Some(SessionFrame::new(108_000))
        );
        assert_eq!(
            entry(
                &host,
                &grid,
                position,
                Bound::AtOrBefore(SessionFrame::new(20_000))
            ),
            Some(SessionFrame::new(12_000))
        );
        assert_eq!(
            entry(
                &host,
                &grid,
                position,
                Bound::AtOrAfter(SessionFrame::new(12_000))
            ),
            Some(SessionFrame::new(12_000))
        );
        assert_eq!(
            phase_error(&host, &grid, position, SessionFrame::new(12_000)).seconds,
            0.0
        );
        assert_eq!(
            jump_target(
                position,
                PhaseError {
                    seconds: 0.1,
                    period: 4.0,
                    beat_seconds: 1.0,
                }
            ),
            Position::from_secs_f64(0.4)
        );
        assert!(
            entry(
                &host,
                &grid,
                Position::from_secs(9),
                Bound::AtOrAfter(SessionFrame::new(0))
            )
            .is_none()
        );
    }

    #[kithara::test]
    fn correction_bounds_every_step_and_cancels_the_phase_integral() {
        let epsilon = 0.001;
        for (from, to, seconds) in [
            (1.0, 1.0, 0.01),
            (1.0, 1.1, -0.01),
            (1.2, 1.0, 0.02),
            (1.0, 1.0, 0.0),
            (1.0, 1.0, 1.01),
        ] {
            let error = PhaseError {
                seconds,
                period: 2.0,
                beat_seconds: 0.5,
            };
            let plan = correction(from, to, error, epsilon);
            let mut previous = from;
            let mut phase = error.shortest();
            for step in &plan.steps {
                assert!((step.speed - previous).abs() <= epsilon);
                assert!(step.seconds >= 0.0 && step.seconds.is_finite());
                phase += (f64::from(step.speed) - f64::from(to)) * step.seconds;
                previous = step.speed;
            }
            assert_eq!(previous, to);
            assert!(phase.abs() < 1e-12);
        }
    }
}
