use super::{
    BeatEstimate, BeatGridId, BeatGridQuery, BeatGridRegion, BeatGridRevision, BeatGridStamp,
    BeatGridState, BeatGridUnavailable, BeatGridView,
};
use crate::{
    Beat, BeatEvidence, BeatsPerMinute, FrameUncertainty, MapAxis, MapPoint, MapPosition, Meter,
    MeterFacts, SessionAnchor, SessionAxis, SessionBeat, SessionEpoch,
};

const SECONDS_PER_MINUTE: f64 = 60.0;

/// Immutable mathematical view of a live session clock.
#[derive(Debug)]
pub(super) struct SessionGridView {
    id: BeatGridId,
    revision: BeatGridRevision,
    axis: SessionAxis,
    anchor: SessionAnchor,
    meter: Option<MeterFacts>,
}

impl SessionGridView {
    pub(super) fn new(
        id: BeatGridId,
        revision: BeatGridRevision,
        epoch: SessionEpoch,
        anchor: SessionAnchor,
        meter: Option<MeterFacts>,
    ) -> Self {
        Self {
            id,
            revision,
            axis: SessionAxis::new(anchor.sample_rate(), epoch),
            anchor,
            meter,
        }
    }

    fn stale<T>(&self, given: BeatGridStamp) -> Option<BeatGridQuery<T>> {
        let expected = self.stamp();
        (given != expected).then_some(BeatGridQuery::Stale { expected, given })
    }
}

impl BeatGridView for SessionGridView {
    fn id(&self) -> BeatGridId {
        self.id
    }

    fn revision(&self) -> BeatGridRevision {
        self.revision
    }

    fn state(&self) -> BeatGridState {
        BeatGridState::Live
    }

    fn axis(&self) -> MapAxis {
        MapAxis::Session(self.axis)
    }

    fn region_at(&self, position: MapPoint<MapPosition>) -> BeatGridQuery<BeatGridRegion> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        if !matches!(position.value(), MapPosition::Session(_)) {
            return BeatGridQuery::Unavailable(BeatGridUnavailable::AxisMismatch);
        }
        BeatGridQuery::Resolved(BeatGridRegion::Unbounded)
    }

    fn beat_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> BeatGridQuery<BeatEstimate<MapPoint<Beat>>> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        let MapPosition::Session(frame) = *position.value() else {
            return BeatGridQuery::Unavailable(BeatGridUnavailable::AxisMismatch);
        };
        let Ok(session_beat) = self.anchor.beat_at(frame) else {
            return BeatGridQuery::OutsideDomain;
        };
        let Ok(beat) = Beat::new(f64::from(session_beat)) else {
            return BeatGridQuery::OutsideDomain;
        };
        BeatGridQuery::Resolved(BeatEstimate::new(
            MapPoint::new(self.stamp(), beat),
            BeatEvidence::Declared,
            FrameUncertainty::ZERO,
            self.stamp(),
        ))
    }

    fn position_at(
        &self,
        beat: MapPoint<Beat>,
    ) -> BeatGridQuery<BeatEstimate<MapPoint<MapPosition>>> {
        if let Some(stale) = self.stale(beat.stamp()) {
            return stale;
        }
        let Ok(session_beat) = SessionBeat::new(f64::from(*beat.value())) else {
            return BeatGridQuery::OutsideDomain;
        };
        let Ok(frame) = self.anchor.frame_at(session_beat) else {
            return BeatGridQuery::OutsideDomain;
        };
        let Ok(rounded_beat) = self.anchor.beat_at(frame) else {
            return BeatGridQuery::OutsideDomain;
        };
        let residual_frames = ((f64::from(session_beat) - f64::from(rounded_beat))
            / self.anchor.beats_per_frame())
        .abs();
        let Ok(uncertainty) = FrameUncertainty::new(residual_frames) else {
            return BeatGridQuery::OutsideDomain;
        };
        BeatGridQuery::Resolved(BeatEstimate::new(
            MapPoint::new(self.stamp(), MapPosition::Session(frame)),
            BeatEvidence::Declared,
            uncertainty,
            self.stamp(),
        ))
    }

    fn tempo_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> BeatGridQuery<BeatEstimate<BeatsPerMinute>> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        if !matches!(position.value(), MapPosition::Session(_)) {
            return BeatGridQuery::Unavailable(BeatGridUnavailable::AxisMismatch);
        }
        let bpm = self.anchor.beats_per_second() * SECONDS_PER_MINUTE;
        let Ok(tempo) = BeatsPerMinute::try_from(bpm) else {
            return BeatGridQuery::Unavailable(BeatGridUnavailable::NoGeometry);
        };
        BeatGridQuery::Resolved(BeatEstimate::new(
            tempo,
            BeatEvidence::Declared,
            FrameUncertainty::ZERO,
            self.stamp(),
        ))
    }

    fn meter_at(&self, beat: MapPoint<Beat>) -> BeatGridQuery<BeatEstimate<Meter>> {
        if let Some(stale) = self.stale(beat.stamp()) {
            return stale;
        }
        let Some(meter) = self.meter else {
            return BeatGridQuery::Unavailable(BeatGridUnavailable::NoMeter);
        };
        let (value, evidence, uncertainty) = meter.into_parts();
        BeatGridQuery::Resolved(BeatEstimate::new(
            value,
            evidence,
            uncertainty,
            self.stamp(),
        ))
    }
}
