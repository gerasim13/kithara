use std::{num::NonZeroU32, ops::RangeInclusive};

use kithara_beat::BeatGridModel;
use kithara_platform::time::Duration;
use kithara_signal::{ExponentialApproach, SessionFrame};
use kithara_warp::{
    AssetAxis, AssetFrame, Beat, BeatGridId, BeatGridQuery, BeatGridRegion, BeatGridRevision,
    BeatGridSnapshot, MIN_SPEED, MapPoint, MapPosition, SpeedCurve,
};
use num_traits::ToPrimitive;

use crate::TempoTrajectory;

/// Source geometry and the observed span eligible for synchronized admission.
#[derive(Debug)]
pub struct SourceGrid {
    grid: BeatGridSnapshot,
    rate: NonZeroU32,
    observed: Option<RangeInclusive<Duration>>,
    metered: bool,
}

impl SourceGrid {
    /// Materializes the served model on its known media axis without extending coverage.
    #[must_use]
    pub fn new(model: &BeatGridModel, axis: AssetAxis) -> Option<Self> {
        let observed = match (model.as_raw().beats.first(), model.as_raw().beats.last()) {
            (Some(first), Some(last)) => Some(
                Duration::try_from_secs_f64(first.at).ok()?
                    ..=Duration::try_from_secs_f64(last.at).ok()?,
            ),
            _ => None,
        };
        Some(Self {
            grid: BeatGridSnapshot::model(
                BeatGridId::allocate().ok()?,
                BeatGridRevision::first(),
                model,
                axis,
            )
            .ok()?,
            rate: axis.sample_rate(),
            observed,
            metered: model.as_raw().meter.is_some(),
        })
    }

    pub(super) fn covers(&self, position: Duration) -> bool {
        self.observed
            .as_ref()
            .is_some_and(|span| span.contains(&position))
    }

    pub(super) fn phase_axes(&self, seconds: f64) -> Option<(f64, f64, bool)> {
        let beat = self.beat_at(seconds)?;
        if !self.metered {
            return Some((0.0, 1.0, false));
        }
        let point = MapPoint::new(self.grid.stamp(), Beat::new(beat).ok()?);
        match self.grid.meter_at(point) {
            BeatGridQuery::Resolved(meter) => Some((
                i64::from(meter.value().downbeat()).to_f64()?,
                f64::from(meter.value().beats_per_bar()),
                true,
            )),
            _ => None,
        }
    }

    fn position(&self, seconds: f64) -> Option<MapPoint<MapPosition>> {
        Some(MapPoint::new(
            self.grid.stamp(),
            MapPosition::Asset(AssetFrame::new(seconds * f64::from(self.rate.get())).ok()?),
        ))
    }

    pub(super) fn beat_at(&self, seconds: f64) -> Option<f64> {
        let BeatGridQuery::Resolved(beat) = self.grid.beat_at(self.position(seconds)?) else {
            return None;
        };
        Some(f64::from(*beat.value().value()))
    }

    pub(super) fn media_at(&self, beat: f64) -> Option<f64> {
        let point = self.point_at(beat)?;
        let MapPosition::Asset(frame) = point.value() else {
            return None;
        };
        Some(f64::from(*frame) / f64::from(self.rate.get()))
    }

    fn point_at(&self, beat: f64) -> Option<MapPoint<MapPosition>> {
        let point = MapPoint::new(self.grid.stamp(), Beat::new(beat).ok()?);
        let BeatGridQuery::Resolved(position) = self.grid.position_at(point) else {
            return None;
        };
        Some(*position.value())
    }

    fn slope(&self, position: MapPoint<MapPosition>) -> Option<f64> {
        let BeatGridQuery::Resolved(tempo) = self.grid.tempo_at(position) else {
            return None;
        };
        Some(60.0 / f64::from(*tempo.value()))
    }

    pub(super) fn slope_at(&self, seconds: f64) -> Option<f64> {
        self.slope(self.position(seconds)?)
    }

    pub(super) fn maximum_slope(&self) -> Option<f64> {
        let mut position = self.position(self.observed.as_ref()?.start().as_secs_f64())?;
        let mut maximum: f64 = 0.0;
        loop {
            maximum = maximum.max(self.slope(position)?);
            let BeatGridQuery::Resolved(BeatGridRegion::Bounded(region)) =
                self.grid.region_at(position)
            else {
                return None;
            };
            let end = MapPoint::new(self.grid.stamp(), region.end());
            let BeatGridQuery::Resolved(BeatGridRegion::Bounded(next)) = self.grid.region_at(end)
            else {
                return Some(maximum);
            };
            if next.end() == region.end() {
                return Some(maximum);
            }
            position = end;
        }
    }

    pub(super) fn nearest_media(&self, beats: [f64; 2], requested: Duration) -> Option<Duration> {
        let mut candidates = [None; 2];
        for (candidate, beat) in candidates.iter_mut().zip(beats) {
            let point = MapPoint::new(self.grid.stamp(), Beat::new(beat).ok()?);
            let position = match self.grid.position_at(point) {
                BeatGridQuery::Resolved(position) => *position.value().value(),
                BeatGridQuery::OutsideDomain => continue,
                BeatGridQuery::Uncovered { required }
                    if required.start() == required.end()
                        && matches!(self.grid.beat_at(MapPoint::new(self.grid.stamp(), required.start())),
                            BeatGridQuery::Resolved(start) if beat < f64::from(*start.value().value())) =>
                {
                    continue;
                }
                _ => return None,
            };
            let MapPosition::Asset(frame) = position else {
                return None;
            };
            *candidate = Some(
                Duration::try_from_secs_f64(f64::from(frame) / f64::from(self.rate.get())).ok()?,
            );
        }
        candidates
            .into_iter()
            .flatten()
            .min_by_key(|candidate| requested.abs_diff(*candidate))
    }

    fn interval_at(&self, beat: f64) -> Option<(f64, Option<f64>)> {
        let mut position = self.point_at(beat)?;
        let slope = self.slope(position)?;
        loop {
            let BeatGridQuery::Resolved(BeatGridRegion::Bounded(region)) =
                self.grid.region_at(position)
            else {
                return None;
            };
            let end = region.end();
            let end_position = MapPoint::new(self.grid.stamp(), end);
            let BeatGridQuery::Resolved(BeatGridRegion::Bounded(next)) =
                self.grid.region_at(end_position)
            else {
                return None;
            };
            if next.end() == end {
                return Some((slope, None));
            }
            if self.slope(end_position)? != slope {
                let BeatGridQuery::Resolved(end_beat) = self.grid.beat_at(end_position) else {
                    return None;
                };
                return Some((slope, Some(f64::from(*end_beat.value().value()))));
            }
            position = end_position;
        }
    }
}

fn session_frame(start: SessionFrame, offset: u64) -> Option<SessionFrame> {
    Some(SessionFrame::new(
        i64::from(start).checked_add(i64::try_from(offset).ok()?)?,
    ))
}

fn host_law(
    host: &TempoTrajectory,
    frame: SessionFrame,
    offset: f64,
) -> Option<ExponentialApproach> {
    let anchor = host.anchor_at(frame);
    let initial = anchor.tempo_at(frame) + offset;
    let target = anchor.target_beats_per_second() + offset;
    ExponentialApproach::new(
        initial,
        target,
        if initial == target {
            0.0
        } else {
            anchor.approach().time_constant() * f64::from(host.sample_rate().get())
        },
    )
}

pub(super) fn host_advance(
    host: &TempoTrajectory,
    from: SessionFrame,
    to: SessionFrame,
) -> Option<f64> {
    if to < from {
        return Some(-host_advance(host, to, from)?);
    }
    let mut start = from;
    let mut beats = 0.0;
    for end in host.changes_after(from).filter(|end| *end < to).chain([to]) {
        let frames = end.frames_since(start)?.to_f64()?;
        beats += host_law(host, start, 0.0)?.advance(frames)? / f64::from(host.sample_rate().get());
        start = end;
    }
    Some(beats)
}

fn crossing(law: ExponentialApproach, beats: f64, rate: f64) -> Option<u64> {
    if !beats.is_finite() || beats <= 0.0 {
        return None;
    }
    let mut high = (beats * rate / law.initial().min(law.target()))
        .ceil()
        .to_u64()?;
    high = high.max(1);
    if law.advance(high.to_f64()?)? < beats * rate {
        high = high.checked_add(1)?;
    }
    let mut low = 0;
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if law.advance(middle.to_f64()?)? >= beats * rate {
            high = middle;
        } else {
            low = middle;
        }
    }
    Some(high)
}

fn push_law(
    laws: &mut Vec<(u64, ExponentialApproach)>,
    frame: u64,
    law: ExponentialApproach,
) -> Option<()> {
    if law.initial() < f64::from(MIN_SPEED)
        || law.target() < f64::from(MIN_SPEED)
        || law.initial() > 4.0
        || law.target() > 4.0
    {
        return None;
    }
    if let Some((at, previous)) = laws.last_mut()
        && *at == frame
    {
        *previous = law;
    } else if laws
        .last()
        .is_some_and(|(_, previous)| *previous == law && law.time_constant() == 0.0)
    {
        return Some(());
    } else {
        laws.push((frame, law));
    }
    Some(())
}

#[derive(Clone, Copy)]
enum CorrectionOffset {
    Media(f64),
    Beat(f64),
}

impl CorrectionOffset {
    fn beats(self, slope: f64) -> f64 {
        match self {
            Self::Media(offset) => offset / slope,
            Self::Beat(offset) => offset,
        }
    }
}

pub(super) struct Correction {
    pub(super) phase: f64,
    pub(super) starts: u64,
    pub(super) scale: f64,
    pub(super) epsilon: f32,
}

fn bridge_beat(
    source: &SourceGrid,
    host: ExponentialApproach,
    mut beat: f64,
    offset: CorrectionOffset,
    rate: f64,
) -> Option<f64> {
    if let CorrectionOffset::Beat(offset) = offset {
        return Some(
            beat + ExponentialApproach::new(
                host.initial() + offset,
                host.target() + offset,
                host.time_constant(),
            )?
            .advance(1.0)?
                / rate,
        );
    }
    let mut elapsed = 0.0;
    while elapsed < 1.0 {
        let (slope, end) = source.interval_at(beat)?;
        let law = ExponentialApproach::new(
            host.rate_at(elapsed)? + offset.beats(slope),
            host.target() + offset.beats(slope),
            host.time_constant(),
        )?;
        let remaining = 1.0 - elapsed;
        let reached = beat + law.advance(remaining)? / rate;
        let Some(end) = end.filter(|end| reached > *end) else {
            return Some(reached);
        };
        let mut low = 0.0;
        let mut high = remaining;
        loop {
            let middle = low + (high - low) / 2.0;
            if middle == low || middle == high {
                break;
            }
            if law.advance(middle)? / rate < end - beat {
                low = middle;
            } else {
                high = middle;
            }
        }
        let through = elapsed + high;
        if through <= elapsed {
            return None;
        }
        elapsed = through;
        beat = end;
    }
    Some(beat)
}

pub(super) fn curve(
    host: &TempoTrajectory,
    source: &SourceGrid,
    media: Duration,
    start: SessionFrame,
    landing: Option<(Duration, SessionFrame)>,
    offsets: &[(u64, f64)],
    correction: Option<&Correction>,
) -> Option<(SpeedCurve, u64)> {
    let rate = f64::from(host.sample_rate().get());
    let first_beat = source.beat_at(media.as_secs_f64())?;
    let mut beat = first_beat;
    let mut boundaries = host
        .changes_after(start)
        .map(|frame| frame.frames_since(start))
        .collect::<Option<Vec<_>>>()?;
    boundaries.extend(
        offsets
            .windows(2)
            .filter(|pair| pair[0].1 != pair[1].1)
            .map(|pair| pair[1].0),
    );
    if let Some(correction) = &correction {
        boundaries.push(correction.starts);
    }
    let landing = match landing {
        Some((position, at)) => Some((position, at.frames_since(start)?)),
        None => None,
    };
    if let Some((_, at)) = landing {
        boundaries.push(at);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut frame = 0;
    let mut terminal = None;
    let mut laws = Vec::new();
    loop {
        if let Some((position, at)) = landing
            && frame == at
        {
            beat = source.beat_at(position.as_secs_f64())?;
        }
        let (slope, source_end) = source.interval_at(beat)?;
        if let Some(correction) = &correction
            && frame >= correction.starts
            && terminal.is_none()
        {
            let phase = correction.phase + beat
                - first_beat
                - host_advance(host, start, session_frame(start, frame)?)?;
            let frames = (phase.abs() * rate * correction.scale / f64::from(correction.epsilon))
                .ceil()
                .to_u64()?;
            let end = frame.checked_add(frames)?;
            let offset = if frames == 0 {
                0.0
            } else {
                -phase * rate / frames.to_f64()?
            };
            let target =
                beat + host_advance(
                    host,
                    session_frame(start, frame)?,
                    session_frame(start, end)?,
                )? + offset * frames.to_f64()? / rate;
            source.media_at(target)?;
            terminal = Some((end, offset));
            boundaries.push(end);
            boundaries.sort_unstable();
            boundaries.dedup();
        }
        let offset = match terminal {
            Some((end, offset)) if frame < end => CorrectionOffset::Beat(offset),
            Some(_) => CorrectionOffset::Beat(0.0),
            None => CorrectionOffset::Media(
                offsets
                    .get(
                        offsets
                            .partition_point(|(at, _)| *at <= frame)
                            .checked_sub(1)?,
                    )?
                    .1,
            ),
        };
        let law = host_law(host, session_frame(start, frame)?, offset.beats(slope))?;
        push_law(&mut laws, frame, law.scaled(slope)?)?;
        let next = boundaries.iter().copied().find(|next| *next > frame);
        let source_crossing = match source_end {
            Some(end) => Some(crossing(law, end - beat, rate)?),
            None => None,
        };
        let through = match (next, source_crossing) {
            (Some(next), Some(crossing)) => next.min(frame.checked_add(crossing)?),
            (Some(next), None) => next,
            (None, Some(crossing)) => frame.checked_add(crossing)?,
            (None, None) => break,
        };
        let elapsed = through.checked_sub(frame)?.to_f64()?;
        let mut reached = beat + law.advance(elapsed)? / rate;
        if source_end.is_some_and(|end| reached > end)
            && source_crossing.is_some_and(|crossing| frame.checked_add(crossing) == Some(through))
        {
            let before = through.checked_sub(1)?;
            let preceding = beat + law.advance(before.checked_sub(frame)?.to_f64()?)? / rate;
            reached = bridge_beat(
                source,
                host_law(host, session_frame(start, before)?, 0.0)?,
                preceding,
                offset,
                rate,
            )?;
            let speed = (source.media_at(reached)? - source.media_at(preceding)?) * rate;
            push_law(
                &mut laws,
                before,
                ExponentialApproach::new(speed, speed, 0.0)?,
            )?;
        }
        beat = reached;
        frame = through;
    }
    let frames = terminal.map_or(0, |(end, _)| end);
    let curve = if laws.len() > 1 {
        SpeedCurve::ApproachSteps(laws.into())
    } else {
        let first = laws.first()?.1;
        let rounded = first.target().to_f32()?;
        if first.time_constant() == 0.0 && f64::from(rounded) == first.target() {
            SpeedCurve::Constant(rounded)
        } else {
            SpeedCurve::Approach(first)
        }
    };
    Some((curve, frames))
}
