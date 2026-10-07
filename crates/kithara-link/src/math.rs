use kithara_beat::BeatGridModel;
use kithara_play::{Bound, Position};
use kithara_signal::SessionFrame;
use kithara_warp::{MIN_SPEED, SessionBeat, SpeedCurve};

use crate::TempoTrajectory;

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
    #[must_use]
    pub fn curve(&self) -> SpeedCurve {
        todo!("SpeedCurve::Steps in kithara-warp")
    }
}

/// Whether the grid's observed beats bracket a position and prove its phase.
#[must_use]
pub fn covers(grid: &BeatGridModel, position: Position) -> bool {
    let raw = grid.as_raw();
    match (raw.beats.first(), raw.beats.last()) {
        (Some(first), Some(last)) => {
            (first.at..=last.at).contains(&position.as_secs_f64())
                && boundary_at_or_before(grid, position).is_some()
        }
        _ => false,
    }
}

/// The last phase boundary at or before a position: a bar line, or a beat
/// when the grid proves no meter.
fn boundary_at_or_before(grid: &BeatGridModel, position: Position) -> Option<Position> {
    let raw = grid.as_raw();
    let metered = raw.meter.is_some();
    raw.downbeats
        .iter()
        .filter(|_| metered)
        .map(|beat| beat.at)
        .chain(raw.beats.iter().filter(|_| !metered).map(|beat| beat.at))
        .take_while(|seconds| *seconds <= position.as_secs_f64())
        .last()
        .and_then(|seconds| Position::try_from_secs_f64(seconds).ok())
}

/// Track speed at a frame, as Host BPM divided by analyzed track BPM.
#[must_use]
pub fn speed(host: &TempoTrajectory, grid: &BeatGridModel, at: SessionFrame) -> f32 {
    (host.tempo_at(at).beats_per_minute() / grid.as_raw().bpm) as f32
}

/// The nearest in-phase entry on the requested side of a frame.
///
/// On constant tempo this is a Host boundary plus the media offset divided
/// by speed; beat inversion also accounts for intervening Host tempo steps.
#[must_use]
pub fn entry(
    host: &TempoTrajectory,
    grid: &BeatGridModel,
    position: Position,
    bound: Bound,
) -> Option<SessionFrame> {
    if !covers(grid, position) {
        return None;
    }
    let offset = (position - boundary_at_or_before(grid, position)?).as_secs_f64()
        * grid.as_raw().bpm
        / 60.0;
    let stride = if grid.as_raw().meter.is_some() {
        host.beats_per_bar()
    } else {
        1.0
    };
    let (frame, after) = match bound {
        Bound::AtOrAfter(frame) => (frame, true),
        Bound::AtOrBefore(frame) => (frame, false),
    };
    let boundary = (f64::from(host.beat_at(frame)) - offset) / stride;
    let ordinal = if after {
        boundary.ceil()
    } else {
        boundary.floor()
    };
    let beat = SessionBeat::new(ordinal * stride + offset).ok()?;
    let candidate = host.try_frame_at(beat)?;
    if (after && candidate < frame) || (!after && candidate > frame) {
        let beat = SessionBeat::new(f64::from(beat) + if after { stride } else { -stride }).ok()?;
        host.try_frame_at(beat)
    } else {
        Some(candidate)
    }
}

/// Phase ahead of the Host, modulo a Host bar or one beat without track meter.
///
/// # Panics
/// Panics unless the track grid covers the position; callers wait for analysis first.
#[must_use]
pub fn phase_error(
    host: &TempoTrajectory,
    grid: &BeatGridModel,
    position: Position,
    at: SessionFrame,
) -> PhaseError {
    assert!(
        covers(grid, position),
        "phase requires a covering track grid"
    );
    let boundary = match boundary_at_or_before(grid, position) {
        Some(boundary) => boundary,
        None => unreachable!("a covering grid has a preceding phase boundary"),
    };
    let stride = if grid.as_raw().meter.is_some() {
        host.beats_per_bar()
    } else {
        1.0
    };
    let seconds_per_beat = 60.0 / grid.as_raw().bpm;
    let period = stride * seconds_per_beat;
    let host_phase = f64::from(host.beat_at(at)).rem_euclid(stride) * seconds_per_beat;
    PhaseError {
        seconds: ((position - boundary).as_secs_f64() - host_phase).rem_euclid(period),
        period,
        beat_seconds: seconds_per_beat,
    }
}

/// The nearer valid media position among the preceding and following phase matches.
///
/// # Panics
/// Panics if the phase period is not finite and positive, or the target is unrepresentable.
#[must_use]
pub fn jump_target(position: Position, error: PhaseError) -> Position {
    assert!(error.seconds.is_finite() && error.period.is_finite() && error.period > 0.0);
    let phase = error.seconds.rem_euclid(error.period);
    let backwards = Position::from_secs_f64(phase);
    if phase <= error.period / 2.0
        && let Some(target) = position.checked_sub(backwards)
    {
        target
    } else {
        position + Position::from_secs_f64(error.period - phase)
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
    assert!(from.is_finite() && from >= MIN_SPEED && to.is_finite() && to >= MIN_SPEED);
    assert!(epsilon.is_finite() && epsilon > 0.0);
    assert!(error.seconds.is_finite() && error.period.is_finite() && error.period > 0.0);
    assert!(error.beat_seconds.is_finite() && error.beat_seconds > 0.0);
    let mut steps = Vec::new();
    let mut current = from;
    let mut remaining = error.shortest();
    while current != to {
        current = step_towards(current, to, epsilon);
        let seconds = if current == to { 0.0 } else { 1.0 };
        remaining += (f64::from(current) - f64::from(to)) * seconds;
        steps.push(CorrectionStep {
            speed: current,
            seconds,
        });
    }
    if remaining != 0.0 {
        let target = to - remaining.signum() as f32 * epsilon;
        assert!(
            target.is_finite() && target >= MIN_SPEED,
            "correction needs a playable excursion"
        );
        let adjusted = step_towards(to, target, epsilon);
        steps.push(CorrectionStep {
            speed: adjusted,
            seconds: -remaining / (f64::from(adjusted) - f64::from(to)),
        });
    }
    steps.push(CorrectionStep {
        speed: to,
        seconds: 0.0,
    });
    CorrectionPlan { steps }
}

fn step_towards(from: f32, to: f32, epsilon: f32) -> f32 {
    let shift = (to - from).clamp(-epsilon, epsilon);
    let mut next = from + shift;
    if (next - from).abs() > epsilon {
        next = if shift > 0.0 {
            next.next_down()
        } else {
            next.next_up()
        };
    }
    assert!(
        next != from,
        "epsilon must permit a representable speed step"
    );
    next
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use kithara_beat::{BeatGridModel, BeatGridState, GridBeat, GridDownbeat, Meter, RawBeatGrid};
    use kithara_host::api::Tempo;
    use kithara_test_utils::kithara;

    use super::*;
    use crate::TempoStep;

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
                    at: ordinal as f64,
                    ordinal,
                })
                .collect(),
            downbeats: [0, 4, 8]
                .into_iter()
                .map(|beat_ordinal| GridDownbeat {
                    confidence: None,
                    at: beat_ordinal as f64,
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
