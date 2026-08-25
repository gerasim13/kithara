use std::num::NonZeroU32;

use kithara_audio::{
    AlignmentPlan, AlignmentRequest, AssetAxis, AssetFrame, Beat, BeatEvidence, BeatMap, BeatMapId,
    BeatMapRevision, BeatMapSnapshot, BeatMapSnapshotError, BeatMarker, BeatOrdinal,
    BeatsPerMinute, FrameUncertainty, HostAxis, HostBeatMap, HostEpoch, MapAxis,
    MapCoordinateError, MapPoint, MapPosition, MapQuery, MapSegment, MapStamp, MapState,
    MapUnavailable, Meter, MeterFacts, PlanTransition, PresentationFrontier, SegmentEndpoint,
    SegmentError, SegmentFacts, SegmentSet, SessionAnchor, SessionBeat, SessionFrame, SyncError,
};
use kithara_test_utils::kithara;

fn map_id() -> BeatMapId {
    BeatMapId::allocate().expect("invariant: fixture map identity space is available")
}

fn sample_rate() -> NonZeroU32 {
    NonZeroU32::new(48_000).expect("invariant: fixture sample rate is non-zero")
}

fn exact() -> FrameUncertainty {
    uncertainty(0.0)
}

fn uncertainty(value: f64) -> FrameUncertainty {
    FrameUncertainty::new(value).expect("invariant: fixture uncertainty is finite and non-negative")
}

fn metered(evidence: BeatEvidence, meter: Meter) -> SegmentFacts {
    SegmentFacts::new(
        evidence,
        exact(),
        Some(MeterFacts::new(meter, evidence, exact())),
    )
}

fn host_meter(meter: Meter) -> MeterFacts {
    MeterFacts::new(meter, BeatEvidence::Declared, exact())
}

fn asset_frame(value: f64) -> MapPosition {
    AssetFrame::new(value)
        .expect("invariant: fixture asset frame is valid")
        .into()
}

fn observed(frame: f64, ordinal: i64) -> BeatMarker {
    marker(frame, ordinal, BeatEvidence::Observed)
}

fn marker(frame: f64, ordinal: i64, evidence: BeatEvidence) -> BeatMarker {
    BeatMarker::new(
        asset_frame(frame),
        Some(BeatOrdinal::new(ordinal)),
        evidence,
        exact(),
    )
}

fn observed_segment(
    start_frame: f64,
    start_ordinal: i64,
    end_frame: f64,
    end_ordinal: i64,
) -> MapSegment {
    segment(
        observed(start_frame, start_ordinal),
        observed(end_frame, end_ordinal),
        BeatEvidence::Interpolated,
    )
}

fn segment(start: BeatMarker, end: BeatMarker, evidence: BeatEvidence) -> MapSegment {
    MapSegment::new(
        start,
        end,
        metered(
            evidence,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: fixture segment is valid")
}

fn playhead_snapshot() -> BeatMapSnapshot {
    const END_FRAME: u64 = 1_536_000;
    asset_snapshot(
        map_id(),
        MapState::Complete,
        END_FRAME + 1,
        vec![observed_segment(0.0, 0, 1_536_000.0, 64)],
    )
}

fn seeded_snapshot() -> BeatMapSnapshot {
    asset_snapshot(
        map_id(),
        MapState::Complete,
        401,
        vec![
            segment(
                marker(0.0, -1, BeatEvidence::Extrapolated),
                observed(100.0, 0),
                BeatEvidence::Extrapolated,
            ),
            observed_segment(100.0, 0, 300.0, 2),
            segment(
                observed(300.0, 2),
                marker(400.0, 3, BeatEvidence::Extrapolated),
                BeatEvidence::Extrapolated,
            ),
        ],
    )
}

fn resolved<T: std::fmt::Debug>(query: MapQuery<T>) -> T {
    match query {
        MapQuery::Resolved(value) => value,
        other => panic!("expected a resolved map query, got {other:?}"),
    }
}

fn asset_snapshot(
    id: BeatMapId,
    state: MapState,
    frame_count: u64,
    segments: Vec<MapSegment>,
) -> BeatMapSnapshot {
    asset_snapshot_at_rate(id, state, sample_rate(), frame_count, segments)
}

fn asset_snapshot_at_rate(
    id: BeatMapId,
    state: MapState,
    sample_rate: NonZeroU32,
    frame_count: u64,
    segments: Vec<MapSegment>,
) -> BeatMapSnapshot {
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate, frame_count));
    let segments =
        SegmentSet::new(axis, segments).expect("invariant: fixture segment topology is valid");
    BeatMapSnapshot::initial(id, state, segments)
        .expect("invariant: fixture snapshot state is valid")
}

fn next_revision(revision: BeatMapRevision) -> BeatMapRevision {
    revision
        .checked_next()
        .expect("invariant: fixture map revision space is available")
}

#[kithara::test]
fn two_beat_gap_preserves_musical_distance() {
    let period = 24_000.0;
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(2.0 * period, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: explicit two-beat span is valid");
    let snapshot = asset_snapshot(map_id(), MapState::Building, 72_001, vec![segment]);

    let start = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(0.0))));
    let midpoint = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(period))));
    let end =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(2.0 * period))));

    assert_eq!(f64::from(*start.value().value()), 0.0);
    assert_eq!(f64::from(*midpoint.value().value()), 1.0);
    assert_eq!(midpoint.evidence(), BeatEvidence::Interpolated);
    assert_eq!(f64::from(*end.value().value()), 2.0);

    let round_trip = resolved(snapshot.position_at(MapPoint::<Beat>::new(
        snapshot.stamp(),
        *midpoint.value().value(),
    )));
    assert_eq!(*round_trip.value().value(), asset_frame(period));
}

#[kithara::test]
fn pickup_ordinals_keep_canonical_downbeat_zero() {
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    assert_eq!(meter.downbeat(), BeatOrdinal::new(0));
    assert_eq!(
        Beat::try_from(meter.downbeat()),
        Beat::new(0.0),
        "consumers must not rebuild the ordinal exactness conversion",
    );
    assert_eq!(
        Beat::try_from(BeatOrdinal::new(9_007_199_254_740_993)),
        Err(MapCoordinateError::InexactBeatOrdinal)
    );
    let segment = MapSegment::new(
        observed(0.0, -2),
        observed(96_000.0, 2),
        metered(BeatEvidence::Interpolated, meter),
    )
    .expect("invariant: pickup fixture topology is valid");
    let snapshot = asset_snapshot(map_id(), MapState::Building, 96_001, vec![segment]);

    let pickup = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(0.0))));
    let downbeat =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(48_000.0))));
    let meter_at_pickup = resolved(snapshot.meter_at(MapPoint::new(
        snapshot.stamp(),
        Beat::new(-1.0).expect("invariant: pickup beat is finite"),
    )));

    assert_eq!(f64::from(*pickup.value().value()), -2.0);
    assert_eq!(f64::from(*downbeat.value().value()), 0.0);
    assert_eq!(meter_at_pickup.value().downbeat(), BeatOrdinal::new(0));

    let changed =
        Meter::with_downbeat(3, BeatOrdinal::new(8)).expect("invariant: changed meter is valid");
    assert_eq!(changed.downbeat(), BeatOrdinal::new(8));
}

#[kithara::test]
fn analyzer_snapshot_requires_marker_span_evidence() {
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let unresolved = BeatMarker::new(asset_frame(48_000.0), None, BeatEvidence::Observed, exact());
    let result = MapSegment::new(observed(0.0, 0), unresolved, facts);

    assert!(matches!(
        result,
        Err(SegmentError::MissingOrdinal {
            endpoint: SegmentEndpoint::End,
        })
    ));

    let explicit = MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
        .expect("explicit musical span must normalize");
    let snapshot = asset_snapshot(map_id(), MapState::Building, 72_001, vec![explicit]);
    let midpoint =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(24_000.0))));
    assert_eq!(f64::from(*midpoint.value().value()), 1.0);
    assert_eq!(midpoint.evidence(), BeatEvidence::Interpolated);
}

#[kithara::test]
fn scalar_tempo_and_segments_share_declared_topology() {
    let facts = metered(
        BeatEvidence::Observed,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Building,
        96_001,
        vec![
            MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
                .expect("invariant: first fixture slope is valid"),
            MapSegment::new(observed(48_000.0, 2), observed(96_000.0, 3), facts)
                .expect("invariant: second fixture slope is valid"),
        ],
    );

    let first = resolved(snapshot.tempo_at(MapPoint::new(snapshot.stamp(), asset_frame(24_000.0))));
    let second =
        resolved(snapshot.tempo_at(MapPoint::new(snapshot.stamp(), asset_frame(72_000.0))));

    assert_eq!(f64::from(*first.value()), 120.0);
    assert_eq!(f64::from(*second.value()), 60.0);
    assert_eq!(first.evidence(), BeatEvidence::Observed);
    assert_eq!(second.evidence(), BeatEvidence::Observed);
    let _: BeatsPerMinute = *second.value();
    let beat = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(72_000.0))));
    assert_eq!(f64::from(*beat.value().value()), 2.5);
    let inverse = resolved(snapshot.position_at(*beat.value()));
    assert_eq!(*inverse.value().value(), asset_frame(72_000.0));
}

#[kithara::test]
fn tempo_only_geometry_does_not_fabricate_meter() {
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        SegmentFacts::new(BeatEvidence::Observed, exact(), None),
    )
    .expect("invariant: tempo-only fixture topology is valid");
    let building = asset_snapshot(map_id(), MapState::Building, 48_001, vec![segment.clone()]);
    let beat = Beat::new(1.0).expect("invariant: fixture beat is finite");

    assert!(matches!(
        building.tempo_at(MapPoint::new(building.stamp(), asset_frame(24_000.0))),
        MapQuery::Resolved(_)
    ));
    match building.meter_at(MapPoint::new(building.stamp(), beat)) {
        MapQuery::Uncovered { required } => {
            assert_eq!(required.start(), asset_frame(0.0));
            assert_eq!(required.end(), asset_frame(48_000.0));
        }
        other => panic!("expected pending meter evidence, got {other:?}"),
    }

    let complete = asset_snapshot(map_id(), MapState::Complete, 48_001, vec![segment]);
    assert!(matches!(
        complete.meter_at(MapPoint::new(complete.stamp(), beat)),
        MapQuery::Unavailable(MapUnavailable::NoMeter)
    ));
}

#[kithara::test]
fn segment_derived_values_report_segment_evidence() {
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        SegmentFacts::new(
            BeatEvidence::Interpolated,
            exact(),
            Some(MeterFacts::new(
                meter,
                BeatEvidence::Extrapolated,
                FrameUncertainty::new(1.0).expect("invariant: fixture meter uncertainty is valid"),
            )),
        ),
    )
    .expect("invariant: fixture topology is valid");
    let snapshot = asset_snapshot(map_id(), MapState::Building, 48_001, vec![segment]);

    let tempo = resolved(snapshot.tempo_at(MapPoint::new(snapshot.stamp(), asset_frame(0.0))));
    let meter = resolved(snapshot.meter_at(MapPoint::new(
        snapshot.stamp(),
        Beat::new(0.0).expect("invariant: fixture beat is finite"),
    )));

    assert_eq!(tempo.evidence(), BeatEvidence::Interpolated);
    assert_eq!(meter.evidence(), BeatEvidence::Extrapolated);
    assert_eq!(f64::from(meter.uncertainty()), 1.0);
}

#[kithara::test]
fn building_gap_is_uncovered_but_complete_extent_is_outside_domain() {
    let facts = metered(
        BeatEvidence::Observed,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let segments = vec![
        MapSegment::new(observed(0.0, 0), observed(24_000.0, 1), facts)
            .expect("invariant: first fixture region is valid"),
        MapSegment::new(observed(72_000.0, 3), observed(120_000.0, 5), facts)
            .expect("invariant: second fixture region is valid"),
    ];
    let building = asset_snapshot(map_id(), MapState::Building, 120_001, segments.clone());

    match building.beat_at(MapPoint::new(building.stamp(), asset_frame(48_000.0))) {
        MapQuery::Uncovered { required } => {
            assert_eq!(required.start(), asset_frame(24_000.0));
            assert_eq!(required.end(), asset_frame(72_000.0));
        }
        other => panic!("expected the sparse gap to be uncovered, got {other:?}"),
    }

    let complete = asset_snapshot(map_id(), MapState::Complete, 120_001, segments);

    assert!(matches!(
        complete.beat_at(MapPoint::new(complete.stamp(), asset_frame(48_000.0))),
        MapQuery::OutsideDomain
    ));
    assert!(matches!(
        complete.beat_at(MapPoint::new(complete.stamp(), asset_frame(120_001.0))),
        MapQuery::OutsideDomain
    ));
}

#[kithara::test]
fn missing_beat_reports_the_corresponding_position_gap() {
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Building,
        120_001,
        vec![
            MapSegment::new(observed(0.0, 0), observed(24_000.0, 1), facts)
                .expect("invariant: first fixture segment is valid"),
            MapSegment::new(observed(72_000.0, 3), observed(120_000.0, 5), facts)
                .expect("invariant: second fixture segment is valid"),
        ],
    );
    let missing = MapPoint::new(
        snapshot.stamp(),
        Beat::new(2.0).expect("invariant: fixture beat is finite"),
    );

    match snapshot.position_at(missing) {
        MapQuery::Uncovered { required } => {
            assert_eq!(required.start(), asset_frame(24_000.0));
            assert_eq!(required.end(), asset_frame(72_000.0));
        }
        other => panic!("expected the musical gap to be uncovered, got {other:?}"),
    }
}

#[kithara::test]
fn stamped_point_from_another_revision_is_stale() {
    let id = map_id();
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: fixture topology is valid");
    let current = asset_snapshot(id, MapState::Building, 72_001, vec![segment]);
    let other_stamp = MapStamp::new(id, next_revision(current.revision()));
    let other_point = MapPoint::new(other_stamp, asset_frame(24_000.0));
    let other_beat = MapPoint::new(
        other_stamp,
        Beat::new(1.0).expect("invariant: fixture beat is finite"),
    );

    assert!(matches!(
        current.beat_at(other_point),
        MapQuery::Stale { expected, given }
            if expected == current.stamp() && given == other_stamp
    ));
    assert!(matches!(
        current.position_at(other_beat),
        MapQuery::Stale { expected, given }
            if expected == current.stamp() && given == other_stamp
    ));
}

#[kithara::test]
fn overlapping_segments_are_rejected() {
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 96_001));
    let facts = metered(
        BeatEvidence::Observed,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let result = SegmentSet::new(
        axis,
        vec![
            MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
                .expect("invariant: first segment is valid alone"),
            MapSegment::new(observed(24_000.0, 1), observed(72_000.0, 3), facts)
                .expect("invariant: second segment is valid alone"),
        ],
    );

    assert!(matches!(result, Err(SegmentError::Overlap { index: 1 })));
}

#[kithara::test]
fn one_sided_segment_boundaries_are_rejected() {
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 96_001));
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let first = MapSegment::new(observed(0.0, 0), observed(24_000.0, 1), facts)
        .expect("invariant: first segment is valid alone");

    for second in [
        MapSegment::new(observed(24_000.0, 2), observed(48_000.0, 3), facts)
            .expect("invariant: beat-jump segment is valid alone"),
        MapSegment::new(observed(48_000.0, 1), observed(72_000.0, 2), facts)
            .expect("invariant: repeated-beat segment is valid alone"),
    ] {
        let result = SegmentSet::new(axis, vec![first.clone(), second]);

        assert!(matches!(
            result,
            Err(SegmentError::NonInvertibleBoundary { index: 1 })
        ));
    }
}

#[kithara::test]
fn sparse_snapshot_is_honest_and_invertible() {
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Building,
        120_001,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(48_000.0, 2),
                metered(BeatEvidence::Interpolated, meter),
            )
            .expect("invariant: interpolated fixture segment is valid"),
            MapSegment::new(
                marker(72_000.0, 3, BeatEvidence::Extrapolated),
                marker(120_000.0, 5, BeatEvidence::Extrapolated),
                metered(BeatEvidence::Extrapolated, meter),
            )
            .expect("invariant: extrapolated fixture segment is valid"),
        ],
    );

    for (frame, expected_beat, evidence) in [
        (0.0, 0.0, BeatEvidence::Observed),
        (24_000.0, 1.0, BeatEvidence::Interpolated),
        (96_000.0, 4.0, BeatEvidence::Extrapolated),
    ] {
        let estimate =
            resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(frame))));
        assert_eq!(f64::from(*estimate.value().value()), expected_beat);
        assert_eq!(estimate.evidence(), evidence);
        let inverse = resolved(snapshot.position_at(*estimate.value()));
        assert_eq!(*inverse.value().value(), asset_frame(frame));
    }

    assert!(matches!(
        snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(60_000.0))),
        MapQuery::Uncovered { .. }
    ));
    let meter_estimate = resolved(snapshot.meter_at(MapPoint::new(
        snapshot.stamp(),
        Beat::new(1.0).expect("invariant: fixture beat is finite"),
    )));
    assert_eq!(*meter_estimate.value(), meter);
    assert_eq!(meter_estimate.evidence(), BeatEvidence::Interpolated);
}

#[kithara::test]
fn progressive_extrapolation_is_immediately_queryable() {
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let low = uncertainty(2.0);
    let high = uncertainty(480.0);
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Building,
        144_001,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(48_000.0, 2),
                SegmentFacts::new(
                    BeatEvidence::Observed,
                    low,
                    Some(MeterFacts::new(meter, BeatEvidence::Observed, low)),
                ),
            )
            .expect("invariant: observed fixture segment is valid"),
            MapSegment::new(
                marker(48_000.0, 2, BeatEvidence::Extrapolated),
                marker(144_000.0, 6, BeatEvidence::Extrapolated),
                SegmentFacts::new(
                    BeatEvidence::Extrapolated,
                    high,
                    Some(MeterFacts::new(meter, BeatEvidence::Extrapolated, high)),
                ),
            )
            .expect("invariant: extrapolated fixture segment is valid"),
        ],
    );
    let estimate =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(72_000.0))));

    assert_eq!(f64::from(*estimate.value().value()), 3.0);
    assert_eq!(estimate.evidence(), BeatEvidence::Extrapolated);
    assert_eq!(estimate.uncertainty(), high);
    assert_eq!(estimate.stamp(), snapshot.stamp());
}

#[kithara::test]
fn unavailable_successor_regulates_host_restart_and_asset_identity() {
    let id = map_id();
    let initial_axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(4)));
    let initial = BeatMapSnapshot::unavailable(id, initial_axis);
    let next_axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(5)));
    let restarted = initial
        .unavailable_successor(initial.stamp(), next_axis)
        .expect("a newer host epoch is a valid unavailable successor");

    assert_eq!(restarted.id(), id);
    assert_eq!(restarted.axis(), next_axis);
    assert_eq!(restarted.revision(), next_revision(initial.revision()));
    assert_eq!(
        restarted.state(),
        MapState::Unavailable(MapUnavailable::NoGeometry)
    );
    assert_eq!(
        restarted.unavailable_successor(initial.stamp(), next_axis),
        Err(BeatMapSnapshotError::Stale {
            expected: restarted.stamp(),
            given: initial.stamp(),
        })
    );

    let asset_axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 48_001));
    let asset = BeatMapSnapshot::unavailable(map_id(), asset_axis);
    let changed_asset_axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 96_001));
    assert_eq!(
        asset.unavailable_successor(asset.stamp(), changed_asset_axis),
        Err(BeatMapSnapshotError::AxisChanged {
            expected: asset_axis,
            given: changed_asset_axis,
        })
    );
    assert_eq!(
        restarted.unavailable_successor(restarted.stamp(), initial_axis),
        Err(BeatMapSnapshotError::AxisChanged {
            expected: next_axis,
            given: initial_axis,
        })
    );
}

#[derive(Debug)]
struct GroupCompatibleFake {
    snapshot: BeatMapSnapshot,
}

impl BeatMap for GroupCompatibleFake {
    fn id(&self) -> BeatMapId {
        self.snapshot.id()
    }

    fn snapshot(&self) -> BeatMapSnapshot {
        self.snapshot.clone()
    }

    fn align_to(
        &self,
        target: &dyn BeatMap,
        request: AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError> {
        self.snapshot.align_to(target, request)
    }

    fn reconcile_to(
        &self,
        target: &dyn BeatMap,
        active: &AlignmentPlan,
        frontier: PresentationFrontier,
    ) -> Result<PlanTransition, SyncError> {
        self.snapshot.reconcile_to(target, active, frontier)
    }
}

fn beat_from(map: &dyn BeatMap, position: MapPosition) -> f64 {
    let snapshot = map.snapshot();
    let estimate = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), position)));
    f64::from(*estimate.value().value())
}

fn align_maps(
    source: &dyn BeatMap,
    target: &dyn BeatMap,
    request: AlignmentRequest,
) -> Result<AlignmentPlan, SyncError> {
    source.align_to(target, request)
}

fn reconcile_maps(
    source: &dyn BeatMap,
    target: &dyn BeatMap,
    active: &AlignmentPlan,
    frontier: PresentationFrontier,
) -> Result<PlanTransition, SyncError> {
    source.reconcile_to(target, active, frontier)
}

type AlignContract =
    fn(&dyn BeatMap, &dyn BeatMap, AlignmentRequest) -> Result<AlignmentPlan, SyncError>;
type ReconcileContract = fn(
    &dyn BeatMap,
    &dyn BeatMap,
    &AlignmentPlan,
    PresentationFrontier,
) -> Result<PlanTransition, SyncError>;

fn observe_plan_transition(transition: &PlanTransition) -> bool {
    matches!(
        transition,
        PlanTransition::Unchanged | PlanTransition::Replace { .. }
    )
}

#[kithara::test]
fn beat_map_exposes_object_safe_alignment_and_reconciliation() {
    let _align_contract: AlignContract = align_maps;
    let _reconcile_contract: ReconcileContract = reconcile_maps;
    let _transition_contract: fn(&PlanTransition) -> bool = observe_plan_transition;
}

#[kithara::test]
fn asset_host_and_group_fake_satisfy_one_object_safe_contract() {
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let asset = asset_snapshot(
        map_id(),
        MapState::Building,
        48_001,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(48_000.0, 2),
                metered(BeatEvidence::Interpolated, meter),
            )
            .expect("invariant: fixture asset topology is valid"),
        ],
    );
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: fixture host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: fixture host relation is valid");
    let host = HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        Some(host_meter(meter)),
    );
    let group_snapshot = asset_snapshot(
        map_id(),
        MapState::Building,
        48_001,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(48_000.0, 2),
                metered(BeatEvidence::Interpolated, meter),
            )
            .expect("invariant: fixture group topology is valid"),
        ],
    );
    assert_ne!(group_snapshot.id(), asset.id());
    assert_ne!(group_snapshot.stamp(), asset.stamp());
    let group_fake = GroupCompatibleFake {
        snapshot: group_snapshot,
    };
    let maps: [&dyn BeatMap; 3] = [&asset, &host, &group_fake];
    let positions = [
        asset_frame(24_000.0),
        MapPosition::Host(SessionFrame::new(24_000)),
        asset_frame(24_000.0),
    ];

    for (map, position) in maps.into_iter().zip(positions) {
        assert_eq!(beat_from(map, position), 1.0);
    }
}

#[kithara::test]
fn external_segment_snapshots_reject_incompatible_states() {
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 48_001));
    let segments = SegmentSet::new(axis, Vec::new())
        .expect("invariant: an empty fixture segment set is valid");

    for state in [
        MapState::Live,
        MapState::Unavailable(MapUnavailable::NoGeometry),
        MapState::Unavailable(MapUnavailable::AxisMismatch),
        MapState::Unavailable(MapUnavailable::NoMeter),
    ] {
        assert_eq!(
            BeatMapSnapshot::initial(map_id(), state, segments.clone()),
            Err(BeatMapSnapshotError::InvalidState { axis, state })
        );
    }

    let host_axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(2)));
    let host_segments = SegmentSet::new(host_axis, Vec::new())
        .expect("invariant: an empty host fixture segment set is valid");
    assert_eq!(
        BeatMapSnapshot::initial(map_id(), MapState::Complete, host_segments),
        Err(BeatMapSnapshotError::InvalidState {
            axis: host_axis,
            state: MapState::Complete,
        })
    );
}

#[kithara::test]
fn empty_host_segment_snapshot_reports_a_host_native_gap() {
    let axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(3)));
    let snapshot = BeatMapSnapshot::initial(
        map_id(),
        MapState::Building,
        SegmentSet::new(axis, Vec::new())
            .expect("invariant: an empty host fixture segment set is valid"),
    )
    .expect("invariant: a building host snapshot is valid");
    let beat = Beat::new(0.0).expect("invariant: fixture beat is finite");

    let MapQuery::Uncovered { required } =
        snapshot.position_at(MapPoint::new(snapshot.stamp(), beat))
    else {
        panic!("expected an uncovered host-native region");
    };
    assert_eq!(required.start(), MapPosition::Host(SessionFrame::new(0)));
    assert_eq!(required.end(), MapPosition::Host(SessionFrame::new(0)));
}

#[kithara::test]
fn unavailable_snapshot_is_an_infallible_external_owner_seed() {
    let axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(4)));
    let revision = BeatMapRevision::first();
    let snapshot = BeatMapSnapshot::unavailable(map_id(), axis);
    let beat = Beat::new(0.0).expect("invariant: fixture beat is finite");

    assert_eq!(snapshot.revision(), revision);
    assert_eq!(snapshot.axis(), axis);
    assert_eq!(
        snapshot.state(),
        MapState::Unavailable(MapUnavailable::NoGeometry)
    );
    assert!(matches!(
        snapshot.position_at(MapPoint::new(snapshot.stamp(), beat)),
        MapQuery::Unavailable(MapUnavailable::NoGeometry)
    ));
}

#[kithara::test]
fn touching_segment_seams_belong_to_the_following_segment() {
    let first_meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let second_meter = Meter::new(3).expect("invariant: fixture meter is valid");
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Building,
        96_001,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(48_000.0, 2),
                metered(BeatEvidence::Interpolated, first_meter),
            )
            .expect("invariant: first fixture segment is valid"),
            MapSegment::new(
                marker(48_000.0, 2, BeatEvidence::Extrapolated),
                marker(96_000.0, 4, BeatEvidence::Extrapolated),
                metered(BeatEvidence::Extrapolated, second_meter),
            )
            .expect("invariant: second fixture segment is valid"),
        ],
    );
    let seam_position = MapPoint::new(snapshot.stamp(), asset_frame(48_000.0));
    let seam_beat = MapPoint::new(
        snapshot.stamp(),
        Beat::new(2.0).expect("invariant: fixture beat is finite"),
    );

    assert_eq!(
        resolved(snapshot.beat_at(seam_position)).evidence(),
        BeatEvidence::Extrapolated
    );
    assert_eq!(
        resolved(snapshot.position_at(seam_beat)).evidence(),
        BeatEvidence::Extrapolated
    );
    let meter = resolved(snapshot.meter_at(seam_beat));
    assert_eq!(*meter.value(), second_meter);
    assert_eq!(meter.evidence(), BeatEvidence::Extrapolated);
}

#[kithara::test]
fn host_revision_is_assigned_by_the_transport_owner() {
    let revision = BeatMapRevision::first()
        .checked_next()
        .expect("invariant: the second host-map revision exists");
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: fixture host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: fixture host relation is valid");
    let host = HostBeatMap::new(
        map_id(),
        revision,
        HostEpoch::new(2),
        anchor,
        Some(host_meter(
            Meter::new(4).expect("invariant: fixture meter is valid"),
        )),
    );

    assert_eq!(host.snapshot().revision(), revision);
}

#[kithara::test]
fn host_inverse_reports_frame_rounding_uncertainty() {
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: fixture host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: fixture host relation is valid");
    let host = HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        Some(host_meter(
            Meter::new(4).expect("invariant: fixture meter is valid"),
        )),
    );
    let snapshot = host.snapshot();
    let half_frame =
        Beat::new(anchor.beats_per_frame() / 2.0).expect("invariant: half-frame beat is finite");

    let estimate = resolved(snapshot.position_at(MapPoint::new(snapshot.stamp(), half_frame)));

    assert_eq!(
        *estimate.value().value(),
        MapPosition::Host(SessionFrame::new(1))
    );
    assert!((f64::from(estimate.uncertainty()) - 0.5).abs() <= f64::EPSILON);
}

#[kithara::test]
fn host_without_meter_keeps_beat_queries_live_but_meter_is_unavailable() {
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: fixture host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: fixture host relation is valid");
    let host = HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        None,
    );
    let snapshot = host.snapshot();
    let beat = resolved(snapshot.beat_at(MapPoint::new(
        snapshot.stamp(),
        MapPosition::Host(SessionFrame::new(24_000)),
    )));

    assert_eq!(f64::from(*beat.value().value()), 1.0);
    assert_eq!(beat.evidence(), BeatEvidence::Declared);
    assert!(matches!(
        snapshot.meter_at(MapPoint::new(
            snapshot.stamp(),
            Beat::new(1.0).expect("invariant: fixture beat is finite"),
        )),
        MapQuery::Unavailable(MapUnavailable::NoMeter)
    ));
}

#[kithara::test]
fn asset_axis_is_independent_of_host_sample_rate() {
    let source_rate = NonZeroU32::new(44_100).expect("invariant: source rate is non-zero");
    let asset = asset_snapshot_at_rate(
        map_id(),
        MapState::Complete,
        source_rate,
        44_101,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(44_100.0, 2),
                metered(
                    BeatEvidence::Interpolated,
                    Meter::new(4).expect("invariant: fixture meter is valid"),
                ),
            )
            .expect("invariant: source-native segment is valid"),
        ],
    );
    let host_anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: host relation is valid");
    let host = HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        host_anchor,
        Some(host_meter(
            Meter::new(4).expect("invariant: fixture meter is valid"),
        )),
    );

    assert!(matches!(
        asset.axis(),
        MapAxis::Asset(axis) if axis.sample_rate() == source_rate
    ));
    assert_eq!(beat_from(&asset, asset_frame(22_050.0)), 1.0);
    assert_eq!(
        beat_from(&host, MapPosition::Host(SessionFrame::new(24_000))),
        1.0
    );
    assert_eq!(
        beat_from(&host, MapPosition::Host(SessionFrame::new(-24_000))),
        -1.0
    );
}

#[kithara::test]
fn marker_beyond_asset_extent_is_rejected() {
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 48_000));
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: segment is valid before bounded-axis validation");

    let result = SegmentSet::new(axis, vec![segment]);

    assert!(matches!(
        result,
        Err(SegmentError::OutsideExtent { index: 0 })
    ));
}

#[kithara::test]
fn large_asset_extent_uses_exact_integer_boundary_semantics() {
    const FIRST_INEXACT_U64: u64 = 9_007_199_254_740_993;
    const LAST_REPRESENTABLE_BELOW: f64 = 9_007_199_254_740_992.0;
    const FIRST_REPRESENTABLE_ABOVE: f64 = 9_007_199_254_740_994.0;
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate(), FIRST_INEXACT_U64));
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let valid = MapSegment::new(
        observed(0.0, 0),
        observed(LAST_REPRESENTABLE_BELOW, 2),
        facts,
    )
    .expect("invariant: representable segment is valid alone");
    SegmentSet::new(axis, vec![valid])
        .expect("the last representable frame below the extent must remain valid");
    let outside = MapSegment::new(
        observed(0.0, 0),
        observed(FIRST_REPRESENTABLE_ABOVE, 2),
        facts,
    )
    .expect("invariant: outside segment is valid before extent validation");

    let result = SegmentSet::new(axis, vec![outside]);

    assert!(matches!(
        result,
        Err(SegmentError::OutsideExtent { index: 0 })
    ));
}

#[kithara::test]
fn segment_with_unrepresentable_tempo_is_rejected() {
    let axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 1));
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(f64::MIN_POSITIVE, 1),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: tiny-span segment is ordered before topology validation");

    let result = SegmentSet::new(axis, vec![segment]);

    assert!(matches!(
        result,
        Err(SegmentError::InvalidTempo { index: 0 })
    ));
}

#[kithara::test]
fn ordinal_outside_exact_beat_range_is_rejected() {
    const FIRST_INEXACT_I64: i64 = 9_007_199_254_740_993;
    let result = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, FIRST_INEXACT_I64),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    );

    assert!(matches!(
        result,
        Err(SegmentError::InexactOrdinal {
            endpoint: SegmentEndpoint::End,
        })
    ));
}

#[kithara::test]
fn a_record_with_no_grid_at_all_names_no_tempo() {
    let unavailable = BeatMapSnapshot::unavailable(
        map_id(),
        MapAxis::Asset(AssetAxis::new(sample_rate(), 24_001)),
    );
    assert!(matches!(
        unavailable.tempo_at(MapPoint::new(unavailable.stamp(), asset_frame(0.0))),
        MapQuery::Unavailable(MapUnavailable::NoGeometry)
    ));

    let complete = asset_snapshot(
        map_id(),
        MapState::Complete,
        24_001,
        vec![observed_segment(0.0, 0, 24_000.0, 1)],
    );
    assert!(matches!(
        complete.tempo_at(MapPoint::new(complete.stamp(), asset_frame(12_000.0))),
        MapQuery::Resolved(_)
    ));
}

#[kithara::test]
fn a_beat_before_the_seeded_start_resolves_to_no_source_frame() {
    let snapshot = seeded_snapshot();
    let before = MapPoint::new(
        snapshot.stamp(),
        Beat::new(-1.5).expect("invariant: negative fixture beat is finite"),
    );
    assert!(matches!(
        snapshot.position_at(before),
        MapQuery::OutsideDomain
    ));

    let start = MapPoint::new(
        snapshot.stamp(),
        Beat::new(-1.0).expect("invariant: seeded start beat is finite"),
    );
    assert!(matches!(snapshot.position_at(start), MapQuery::Resolved(_)));
}

#[kithara::test]
fn a_beat_past_the_seeded_end_resolves_to_no_source_frame() {
    let snapshot = seeded_snapshot();
    let end = MapPoint::new(
        snapshot.stamp(),
        Beat::new(3.0).expect("invariant: seeded end beat is finite"),
    );
    assert!(matches!(snapshot.position_at(end), MapQuery::Resolved(_)));

    let after = MapPoint::new(
        snapshot.stamp(),
        Beat::new(3.5).expect("invariant: post-domain beat is finite"),
    );
    assert!(matches!(
        snapshot.position_at(after),
        MapQuery::OutsideDomain
    ));
}

#[kithara::test]
fn a_position_before_the_first_detected_beat_resolves_to_a_track_beat() {
    let snapshot = seeded_snapshot();
    let estimate = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(50.0))));

    assert_eq!(f64::from(*estimate.value().value()), -0.5);
    assert_eq!(estimate.evidence(), BeatEvidence::Extrapolated);
}

#[kithara::test]
fn a_position_after_the_last_detected_beat_resolves_to_a_track_beat() {
    let snapshot = seeded_snapshot();
    let estimate = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(350.0))));

    assert_eq!(f64::from(*estimate.value().value()), 2.5);
    assert_eq!(estimate.evidence(), BeatEvidence::Extrapolated);
}

#[kithara::test]
fn the_beat_ordinal_of_every_detected_beat_is_unchanged_by_the_seeding() {
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Complete,
        501,
        vec![
            segment(
                marker(0.0, -3, BeatEvidence::Extrapolated),
                observed(100.0, -2),
                BeatEvidence::Extrapolated,
            ),
            observed_segment(100.0, -2, 200.0, -1),
            observed_segment(200.0, -1, 300.0, 0),
            observed_segment(300.0, 0, 400.0, 1),
            segment(
                observed(400.0, 1),
                marker(500.0, 2, BeatEvidence::Extrapolated),
                BeatEvidence::Extrapolated,
            ),
        ],
    );

    for (frame, expected) in [(100.0, -2.0), (200.0, -1.0), (300.0, 0.0), (400.0, 1.0)] {
        let estimate =
            resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(frame))));
        assert_eq!(f64::from(*estimate.value().value()), expected);
    }
}

#[kithara::test]
fn a_position_outside_the_source_extent_still_resolves_to_nothing() {
    let snapshot = seeded_snapshot();
    assert!(matches!(
        snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(401.0))),
        MapQuery::OutsideDomain
    ));

    assert!(matches!(
        snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(400.0))),
        MapQuery::Resolved(_)
    ));
}

#[kithara::test]
fn the_playhead_reads_the_beat_the_analysis_put_under_it() {
    let snapshot = playhead_snapshot();
    let estimate =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(192_000.0))));

    assert_eq!(f64::from(*estimate.value().value()), 8.0);
}

#[kithara::test]
fn a_playhead_between_markers_reads_a_fractional_beat() {
    let snapshot = playhead_snapshot();
    let estimate =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(204_000.0))));

    assert_eq!(f64::from(*estimate.value().value()), 8.5);
}

#[kithara::test]
fn a_playhead_past_the_analysed_markers_reads_no_beat() {
    let snapshot = playhead_snapshot();
    let end = resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(1_536_000.0))));
    assert_eq!(f64::from(*end.value().value()), 64.0);

    assert!(matches!(
        snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(1_536_001.0),)),
        MapQuery::OutsideDomain
    ));
}

#[kithara::test]
fn a_beat_of_advance_consumes_its_own_marker_span() {
    const BEAT_FRAMES: f64 = 24_000.0;
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Complete,
        192_001,
        vec![observed_segment(0.0, 0, 192_000.0, 8)],
    );

    for beat in 0_i32..8 {
        let point = MapPoint::new(
            snapshot.stamp(),
            Beat::new(f64::from(beat)).expect("invariant: fixture beat is finite"),
        );
        let position = resolved(snapshot.position_at(point));

        assert_eq!(
            *position.value().value(),
            asset_frame(f64::from(beat) * BEAT_FRAMES),
            "beat {beat} must land on its own source frame",
        );
    }
}

#[kithara::test]
fn drifting_grid_follows_the_local_slope() {
    let slow = sample_rate().get() * 60 / 118;
    let fast = sample_rate().get() * 60 / 122;
    let boundary = slow * 4;
    let end = boundary + fast * 4;
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Complete,
        u64::from(end) + 1,
        vec![
            observed_segment(0.0, 0, f64::from(boundary), 4),
            observed_segment(f64::from(boundary), 4, f64::from(end), 8),
        ],
    );
    let position = |beat: f64| {
        let estimate = resolved(snapshot.position_at(MapPoint::new(
            snapshot.stamp(),
            Beat::new(beat).expect("invariant: fixture beat is finite"),
        )));
        f64::try_from(*estimate.value().value())
            .expect("invariant: fixture position is on a numeric asset axis")
    };

    let early = position(1.0) - position(0.0);
    let late = position(7.0) - position(6.0);

    assert_eq!(early, f64::from(slow));
    assert_eq!(late, f64::from(fast));
    assert_ne!(early, late, "the local slopes must remain distinguishable");
}

#[kithara::test]
fn advance_past_the_analysed_domain_is_typed() {
    let snapshot = asset_snapshot(
        map_id(),
        MapState::Complete,
        72_001,
        vec![observed_segment(0.0, 0, 72_000.0, 3)],
    );
    let inside = MapPoint::new(
        snapshot.stamp(),
        Beat::new(2.0).expect("invariant: fixture beat is finite"),
    );
    assert!(matches!(
        snapshot.position_at(inside),
        MapQuery::Resolved(_)
    ));

    let outside = MapPoint::new(
        snapshot.stamp(),
        Beat::new(100.0).expect("invariant: fixture beat is finite"),
    );
    assert!(matches!(
        snapshot.position_at(outside),
        MapQuery::OutsideDomain
    ));
}

#[kithara::test]
fn a_deck_with_no_committed_grid_has_no_beat_advance() {
    let axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(1)));
    let unavailable = BeatMapSnapshot::unavailable(map_id(), axis);
    let zero = Beat::new(0.0).expect("invariant: fixture beat is finite");
    assert!(matches!(
        unavailable.position_at(MapPoint::new(unavailable.stamp(), zero)),
        MapQuery::Unavailable(MapUnavailable::NoGeometry)
    ));

    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::default(),
        2.0,
        sample_rate(),
    )
    .expect("invariant: fixture host relation is valid");
    let committed = HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        None,
    )
    .snapshot();
    assert!(matches!(
        committed.position_at(MapPoint::new(committed.stamp(), zero)),
        MapQuery::Resolved(_)
    ));
}

#[kithara::test]
fn the_beat_advance_follows_a_tempo_commit() {
    let first_anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::default(),
        2.0,
        sample_rate(),
    )
    .expect("invariant: initial host relation is valid");
    let first = HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        first_anchor,
        None,
    )
    .snapshot();
    let next_anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::default(),
        4.0,
        sample_rate(),
    )
    .expect("invariant: recommitted host relation is valid");
    let next = first
        .host_successor(first.stamp(), HostEpoch::new(1), next_anchor, None)
        .expect("invariant: transport owner can publish the next host relation");
    let at_one_beat = MapPosition::Host(SessionFrame::new(24_000));
    let before = resolved(first.beat_at(MapPoint::new(first.stamp(), at_one_beat)));
    let after = resolved(next.beat_at(MapPoint::new(next.stamp(), at_one_beat)));

    assert_eq!(f64::from(*before.value().value()), 1.0);
    assert_eq!(f64::from(*after.value().value()), 2.0);
    assert_eq!(next.revision(), next_revision(first.revision()));
}
