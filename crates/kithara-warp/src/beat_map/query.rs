use super::{
    Beat, BeatEstimate, BeatEvidence, BeatMapGeometry, BeatMapId, BeatMapRevision, BeatMapSnapshot,
    BeatsPerMinute, FrameUncertainty, MapAxis, MapPoint, MapPosition, MapQuery, MapRegion,
    MapStamp, MapState, MapUnavailable, Meter, SECONDS_PER_MINUTE, SegmentSet, SessionBeat,
    SessionFrame,
};

impl BeatMapSnapshot {
    delegate::delegate! {
        to self.data {
            /// Returns the stable map identity.
            #[must_use]
            #[field]
            pub fn id(&self) -> BeatMapId;
            /// Returns the immutable map revision.
            #[must_use]
            #[field]
            pub fn revision(&self) -> BeatMapRevision;
            /// Returns the snapshot lifecycle state.
            #[must_use]
            #[field]
            pub fn state(&self) -> MapState;
            /// Returns the typed coordinate axis used by this snapshot.
            #[must_use]
            #[field]
            pub fn axis(&self) -> MapAxis;
        }
    }

    /// Returns the composite identity and revision.
    #[must_use]
    pub fn stamp(&self) -> MapStamp {
        MapStamp::new(self.id(), self.revision())
    }

    /// Returns the validated immutable segment collection for a segment-backed map.
    #[must_use]
    pub fn segments(&self) -> Option<&SegmentSet> {
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => Some(segments),
            BeatMapGeometry::Host { .. } => None,
        }
    }

    /// Resolves a stamped map-native position to a stamped beat.
    pub fn beat_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> MapQuery<BeatEstimate<MapPoint<Beat>>> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        if position.value().kind() != self.axis().kind() {
            return MapQuery::Unavailable(MapUnavailable::AxisMismatch);
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        if self.outside_asset_extent(*position.value()) {
            return MapQuery::OutsideDomain;
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some((beat, evidence, uncertainty)) = segments
                    .by_position(*position.value())
                    .and_then(|segment| segment.beat_at(*position.value()))
                else {
                    return self.missing_position(*position.value());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), beat),
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { anchor, .. } => {
                let MapPosition::Host(frame) = *position.value() else {
                    return MapQuery::Unavailable(MapUnavailable::AxisMismatch);
                };
                let Ok(session_beat) = anchor.beat_at(frame) else {
                    return MapQuery::OutsideDomain;
                };
                let Ok(beat) = Beat::new(f64::from(session_beat)) else {
                    return MapQuery::OutsideDomain;
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), beat),
                    BeatEvidence::Declared,
                    FrameUncertainty::ZERO,
                    self.stamp(),
                ))
            }
        }
    }

    /// Resolves a stamped beat to a stamped map-native position.
    pub fn position_at(
        &self,
        beat: MapPoint<Beat>,
    ) -> MapQuery<BeatEstimate<MapPoint<MapPosition>>> {
        if let Some(stale) = self.stale(beat.stamp()) {
            return stale;
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some((position, evidence, uncertainty)) = segments
                    .by_beat(*beat.value())
                    .and_then(|segment| segment.position_at(*beat.value()))
                else {
                    return self.missing_beat(*beat.value());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), position),
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { anchor, .. } => {
                let Ok(session_beat) = SessionBeat::new(f64::from(*beat.value())) else {
                    return MapQuery::OutsideDomain;
                };
                let Ok(frame) = anchor.frame_at(session_beat) else {
                    return MapQuery::OutsideDomain;
                };
                let Ok(rounded_beat) = anchor.beat_at(frame) else {
                    return MapQuery::OutsideDomain;
                };
                let residual_frames = ((f64::from(session_beat) - f64::from(rounded_beat))
                    / anchor.beats_per_frame())
                .abs();
                let Ok(uncertainty) = FrameUncertainty::new(residual_frames) else {
                    return MapQuery::OutsideDomain;
                };
                MapQuery::Resolved(BeatEstimate::new(
                    MapPoint::new(self.stamp(), MapPosition::Host(frame)),
                    BeatEvidence::Declared,
                    uncertainty,
                    self.stamp(),
                ))
            }
        }
    }

    /// Resolves the local tempo derived from the same segment topology.
    pub fn tempo_at(
        &self,
        position: MapPoint<MapPosition>,
    ) -> MapQuery<BeatEstimate<BeatsPerMinute>> {
        if let Some(stale) = self.stale(position.stamp()) {
            return stale;
        }
        if position.value().kind() != self.axis().kind() {
            return MapQuery::Unavailable(MapUnavailable::AxisMismatch);
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        if self.outside_asset_extent(*position.value()) {
            return MapQuery::OutsideDomain;
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some((tempo, evidence, uncertainty)) = segments
                    .by_position(*position.value())
                    .and_then(|segment| segment.tempo_at(self.axis(), *position.value()))
                else {
                    return self.missing_position(*position.value());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    tempo,
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { anchor, .. } => {
                let bpm = anchor.beats_per_second() * SECONDS_PER_MINUTE;
                let Some(tempo) = BeatsPerMinute::new(bpm) else {
                    return MapQuery::Unavailable(MapUnavailable::NoGeometry);
                };
                MapQuery::Resolved(BeatEstimate::new(
                    tempo,
                    BeatEvidence::Declared,
                    FrameUncertainty::ZERO,
                    self.stamp(),
                ))
            }
        }
    }

    /// Resolves the meter carried by the segment containing `beat`.
    pub fn meter_at(&self, beat: MapPoint<Beat>) -> MapQuery<BeatEstimate<Meter>> {
        if let Some(stale) = self.stale(beat.stamp()) {
            return stale;
        }
        if let MapState::Unavailable(reason) = self.state() {
            return MapQuery::Unavailable(reason);
        }
        match &self.data.geometry {
            BeatMapGeometry::Segments(segments) => {
                let Some(segment) = segments.by_beat(*beat.value()) else {
                    return self.missing_beat(*beat.value());
                };
                let Some((meter, evidence, uncertainty)) = segment.meter_at(*beat.value()) else {
                    return self.missing_meter(segment.region());
                };
                MapQuery::Resolved(BeatEstimate::new(
                    meter,
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
            BeatMapGeometry::Host { meter, .. } => {
                let Some(meter) = *meter else {
                    return MapQuery::Unavailable(MapUnavailable::NoMeter);
                };
                let (value, evidence, uncertainty) = meter.into_parts();
                MapQuery::Resolved(BeatEstimate::new(
                    value,
                    evidence,
                    uncertainty,
                    self.stamp(),
                ))
            }
        }
    }

    fn stale<T>(&self, given: MapStamp) -> Option<MapQuery<T>> {
        let expected = self.stamp();
        (given != expected).then_some(MapQuery::Stale { expected, given })
    }

    fn missing_position<T>(&self, position: MapPosition) -> MapQuery<T> {
        match self.state() {
            MapState::Complete => MapQuery::OutsideDomain,
            MapState::Building | MapState::Live => MapQuery::Uncovered {
                required: match &self.data.geometry {
                    BeatMapGeometry::Segments(segments) => segments.uncovered_region(position),
                    BeatMapGeometry::Host { .. } => MapRegion::point(position),
                },
            },
            MapState::Unavailable(reason) => MapQuery::Unavailable(reason),
        }
    }

    fn missing_beat<T>(&self, beat: Beat) -> MapQuery<T> {
        match self.state() {
            MapState::Complete => MapQuery::OutsideDomain,
            MapState::Building | MapState::Live => MapQuery::Uncovered {
                required: match &self.data.geometry {
                    BeatMapGeometry::Segments(segments) => segments.uncovered_region_by_beat(beat),
                    BeatMapGeometry::Host { .. } => {
                        MapRegion::point(MapPosition::Host(SessionFrame::new(0)))
                    }
                },
            },
            MapState::Unavailable(reason) => MapQuery::Unavailable(reason),
        }
    }

    fn missing_meter<T>(&self, required: MapRegion) -> MapQuery<T> {
        match self.state() {
            MapState::Building => MapQuery::Uncovered { required },
            MapState::Complete | MapState::Live => MapQuery::Unavailable(MapUnavailable::NoMeter),
            MapState::Unavailable(reason) => MapQuery::Unavailable(reason),
        }
    }

    fn outside_asset_extent(&self, position: MapPosition) -> bool {
        match (self.axis(), position) {
            (MapAxis::Asset(axis), MapPosition::Asset(frame)) => !axis.contains_or_eof(frame),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use kithara_test_utils::kithara;

    use crate::{
        AssetAxis, AssetFrame, Beat, BeatEvidence, BeatMapId, BeatMapSnapshot, BeatMarker,
        BeatOrdinal, FrameUncertainty, MapAxis, MapPoint, MapPosition, MapQuery, MapSegment,
        MapState, SegmentError, SegmentFacts, SegmentSet,
    };

    struct Consts;

    impl Consts {
        const FRAME_COUNT: u64 = 48_000;
        const SAMPLE_RATE: u32 = 48_000;
        const EOF_FRAME: f64 = 48_000.0;
        const AFTER_EOF_FRAME: f64 = 48_000.5;
    }

    fn asset_frame(value: f64) -> AssetFrame {
        AssetFrame::new(value).expect("invariant: fixture asset frame is finite and non-negative")
    }

    fn marker(frame: f64, ordinal: i64) -> BeatMarker {
        BeatMarker::new(
            MapPosition::Asset(asset_frame(frame)),
            Some(BeatOrdinal::new(ordinal)),
            BeatEvidence::Observed,
            FrameUncertainty::ZERO,
        )
    }

    #[kithara::test]
    fn complete_asset_map_round_trips_a_segment_endpoint_at_eof() {
        let sample_rate = NonZeroU32::new(Consts::SAMPLE_RATE)
            .expect("invariant: fixture sample rate is non-zero");
        let asset_axis = AssetAxis::new(sample_rate, Consts::FRAME_COUNT);
        let eof = asset_frame(Consts::EOF_FRAME);
        assert!(
            !asset_axis.contains(eof),
            "the EOF boundary is not an addressable source sample"
        );
        let segment = MapSegment::new(
            marker(0.0, 0),
            marker(Consts::EOF_FRAME, 2),
            SegmentFacts::new(BeatEvidence::Interpolated, FrameUncertainty::ZERO, None),
        )
        .expect("invariant: fixture markers form an increasing affine relation");
        let segments = SegmentSet::new(MapAxis::Asset(asset_axis), vec![segment])
            .expect("a continuous segment may end exactly at the exclusive asset boundary");
        let map = BeatMapSnapshot::initial(
            BeatMapId::allocate().expect("invariant: fixture map id can be allocated"),
            MapState::Complete,
            segments,
        )
        .expect("invariant: complete state is valid for a bounded asset map");
        let endpoint_beat = Beat::new(2.0).expect("invariant: fixture beat is finite");

        let MapQuery::Resolved(position) =
            map.position_at(MapPoint::new(map.stamp(), endpoint_beat))
        else {
            panic!("the endpoint beat must resolve to the EOF boundary");
        };
        assert_eq!(*position.value().value(), MapPosition::Asset(eof));

        let MapQuery::Resolved(round_tripped) = map.beat_at(*position.value()) else {
            panic!("the EOF boundary must resolve through its segment geometry");
        };
        assert_eq!(*round_tripped.value().value(), endpoint_beat);

        let beyond_eof = asset_frame(Consts::AFTER_EOF_FRAME);
        assert!(matches!(
            map.beat_at(MapPoint::new(map.stamp(), MapPosition::Asset(beyond_eof),)),
            MapQuery::OutsideDomain
        ));
        let beyond_segment = MapSegment::new(
            marker(0.0, 0),
            marker(Consts::AFTER_EOF_FRAME, 2),
            SegmentFacts::new(BeatEvidence::Interpolated, FrameUncertainty::ZERO, None),
        )
        .expect("invariant: the overlong fixture remains an affine relation");
        assert_eq!(
            SegmentSet::new(MapAxis::Asset(asset_axis), vec![beyond_segment]),
            Err(SegmentError::OutsideExtent { index: 0 })
        );
    }

    #[kithara::test]
    fn uncovered_eof_uses_the_map_lifecycle_instead_of_current_geometry() {
        let sample_rate = NonZeroU32::new(Consts::SAMPLE_RATE)
            .expect("invariant: fixture sample rate is non-zero");
        let asset_axis = AssetAxis::new(sample_rate, Consts::FRAME_COUNT);
        let axis = MapAxis::Asset(asset_axis);
        let eof = MapPosition::Asset(asset_frame(Consts::EOF_FRAME));
        let beyond_eof = MapPosition::Asset(asset_frame(Consts::AFTER_EOF_FRAME));
        let building = BeatMapSnapshot::initial(
            BeatMapId::allocate().expect("invariant: fixture map id can be allocated"),
            MapState::Building,
            SegmentSet::empty(axis),
        )
        .expect("invariant: a bounded asset map may begin without geometry");

        assert!(matches!(
            building.beat_at(MapPoint::new(building.stamp(), eof)),
            MapQuery::Uncovered { .. }
        ));
        assert!(matches!(
            building.tempo_at(MapPoint::new(building.stamp(), eof)),
            MapQuery::Uncovered { .. }
        ));
        assert!(matches!(
            building.beat_at(MapPoint::new(building.stamp(), beyond_eof)),
            MapQuery::OutsideDomain
        ));
        assert!(matches!(
            building.tempo_at(MapPoint::new(building.stamp(), beyond_eof)),
            MapQuery::OutsideDomain
        ));

        let complete = BeatMapSnapshot::initial(
            BeatMapId::allocate().expect("invariant: fixture map id can be allocated"),
            MapState::Complete,
            SegmentSet::empty(axis),
        )
        .expect("invariant: a complete empty asset map has no covered positions");
        assert!(matches!(
            complete.beat_at(MapPoint::new(complete.stamp(), eof)),
            MapQuery::OutsideDomain
        ));
        assert!(matches!(
            complete.tempo_at(MapPoint::new(complete.stamp(), eof)),
            MapQuery::OutsideDomain
        ));
    }
}
