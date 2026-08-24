use std::num::NonZeroU32;

use kithara_audio::{
    AlignmentPlan, AlignmentRequest, AssetAxis, AssetBeatMap, AssetFrame, AssetMapPublishError,
    AssetMapUpdate, Beat, BeatEvidence, BeatMap, BeatMapId, BeatMapRevision, BeatMapSnapshot,
    BeatMapSnapshotError, BeatMarker, BeatOrdinal, BeatsPerMinute, FrameUncertainty, HostAxis,
    HostBeatMap, HostEpoch, MapAxis, MapCoordinateError, MapPoint, MapPosition, MapQuery,
    MapSegment, MapState, MapUnavailable, Meter, MeterFacts, PlanTransition, PresentationFrontier,
    SegmentDraft, SegmentEndpoint, SegmentError, SegmentFacts, SegmentSet, SessionAnchor,
    SessionBeat, SessionFrame, SyncError,
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

fn resolved<T: std::fmt::Debug>(query: MapQuery<T>) -> T {
    match query {
        MapQuery::Resolved(value) => value,
        other => panic!("expected a resolved map query, got {other:?}"),
    }
}

#[kithara::test]
fn two_beat_gap_preserves_musical_distance() {
    let period = 24_000.0;
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 72_001);
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(2.0 * period, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: explicit two-beat span is valid");
    let initial = map.snapshot();

    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            initial.stamp(),
            MapState::Building,
            vec![segment],
        ))
        .expect("invariant: first partial map publication is valid");

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
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 96_001);
    let segment = MapSegment::new(
        observed(0.0, -2),
        observed(96_000.0, 2),
        metered(BeatEvidence::Interpolated, meter),
    )
    .expect("invariant: pickup fixture topology is valid");
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![segment],
        ))
        .expect("invariant: pickup map publication is valid");

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
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 72_001);
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let unresolved = BeatMarker::new(asset_frame(48_000.0), None, BeatEvidence::Observed, exact());
    let draft = SegmentDraft::new(observed(0.0, 0), unresolved, facts);

    let result =
        AssetMapUpdate::try_from((map.snapshot().stamp(), MapState::Building, vec![draft]));

    assert!(matches!(
        result,
        Err(SegmentError::MissingOrdinal {
            endpoint: SegmentEndpoint::End,
        })
    ));

    let explicit = SegmentDraft::new(observed(0.0, 0), observed(48_000.0, 2), facts);
    let update =
        AssetMapUpdate::try_from((map.snapshot().stamp(), MapState::Building, vec![explicit]))
            .expect("explicit musical span must normalize");
    let snapshot = publisher
        .publish(update)
        .expect("explicit musical span must publish");
    let midpoint =
        resolved(snapshot.beat_at(MapPoint::new(snapshot.stamp(), asset_frame(24_000.0))));
    assert_eq!(f64::from(*midpoint.value().value()), 1.0);
    assert_eq!(midpoint.evidence(), BeatEvidence::Interpolated);
}

#[kithara::test]
fn scalar_tempo_and_segments_share_declared_topology() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 96_001);
    let facts = metered(
        BeatEvidence::Observed,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![
                MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
                    .expect("invariant: first fixture slope is valid"),
                MapSegment::new(observed(48_000.0, 2), observed(96_000.0, 3), facts)
                    .expect("invariant: second fixture slope is valid"),
            ],
        ))
        .expect("invariant: topology publication is valid");

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
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        SegmentFacts::new(BeatEvidence::Observed, exact(), None),
    )
    .expect("invariant: tempo-only fixture topology is valid");
    let building = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![segment.clone()],
        ))
        .expect("invariant: tempo-only building map is valid");
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

    let complete = publisher
        .publish(AssetMapUpdate::new(
            building.stamp(),
            MapState::Complete,
            vec![segment],
        ))
        .expect("invariant: tempo-only complete map is valid");
    assert!(matches!(
        complete.meter_at(MapPoint::new(complete.stamp(), beat)),
        MapQuery::Unavailable(MapUnavailable::NoMeter)
    ));
}

#[kithara::test]
fn segment_derived_values_report_segment_evidence() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
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
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![segment],
        ))
        .expect("invariant: topology publication is valid");

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
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 120_001);
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
    let building = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            segments.clone(),
        ))
        .expect("invariant: sparse building map is valid");

    match building.beat_at(MapPoint::new(building.stamp(), asset_frame(48_000.0))) {
        MapQuery::Uncovered { required } => {
            assert_eq!(required.start(), asset_frame(24_000.0));
            assert_eq!(required.end(), asset_frame(72_000.0));
        }
        other => panic!("expected the sparse gap to be uncovered, got {other:?}"),
    }

    let complete = publisher
        .publish(AssetMapUpdate::new(
            building.stamp(),
            MapState::Complete,
            segments,
        ))
        .expect("invariant: complete sparse map is valid");

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
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 120_001);
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![
                MapSegment::new(observed(0.0, 0), observed(24_000.0, 1), facts)
                    .expect("invariant: first fixture segment is valid"),
                MapSegment::new(observed(72_000.0, 3), observed(120_000.0, 5), facts)
                    .expect("invariant: second fixture segment is valid"),
            ],
        ))
        .expect("invariant: sparse fixture map is valid");
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
fn map_revision_publish_is_monotonic_and_old_snapshots_stay_immutable() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 72_001);
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let first = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![
                MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
                    .expect("invariant: first topology is valid"),
            ],
        ))
        .expect("invariant: first revision is valid");
    let first_answer = resolved(first.beat_at(MapPoint::new(first.stamp(), asset_frame(24_000.0))));

    let second = publisher
        .publish(AssetMapUpdate::new(
            first.stamp(),
            MapState::Building,
            vec![
                MapSegment::new(observed(0.0, 0), observed(47_000.0, 2), facts)
                    .expect("invariant: refined topology is valid"),
            ],
        ))
        .expect("invariant: second revision is valid");

    assert!(second.revision() > first.revision());
    assert_eq!(map.snapshot(), second);
    assert_eq!(
        f64::from(
            *resolved(first.beat_at(MapPoint::new(first.stamp(), asset_frame(24_000.0),)))
                .value()
                .value()
        ),
        f64::from(*first_answer.value().value())
    );
    assert_ne!(
        f64::from(
            *resolved(second.beat_at(MapPoint::new(second.stamp(), asset_frame(24_000.0),)))
                .value()
                .value()
        ),
        f64::from(*first_answer.value().value())
    );
}

#[kithara::test]
fn stale_asset_updates_do_not_publish() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let (foreign, _) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let initial = map.snapshot();
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: fixture segment is valid");
    let current = publisher
        .publish(AssetMapUpdate::new(
            initial.stamp(),
            MapState::Building,
            vec![segment.clone()],
        ))
        .expect("invariant: first publication is valid");

    for given in [initial.stamp(), foreign.snapshot().stamp()] {
        let result = publisher.publish(AssetMapUpdate::new(
            given,
            MapState::Building,
            vec![segment.clone()],
        ));

        assert!(matches!(
            result,
            Err(AssetMapPublishError::Stale { expected, given: rejected })
                if expected == current.stamp() && rejected == given
        ));
        assert_eq!(map.snapshot(), current);
    }
}

#[kithara::test]
fn stamped_point_from_old_revision_is_stale() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 72_001);
    let initial = map.snapshot();
    let old_point = MapPoint::new(initial.stamp(), asset_frame(24_000.0));
    let old_beat = MapPoint::new(
        initial.stamp(),
        Beat::new(1.0).expect("invariant: fixture beat is finite"),
    );
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: fixture topology is valid");
    let current = publisher
        .publish(AssetMapUpdate::new(
            initial.stamp(),
            MapState::Building,
            vec![segment],
        ))
        .expect("invariant: publication is valid");

    assert!(matches!(
        current.beat_at(old_point),
        MapQuery::Stale { expected, given }
            if expected == current.stamp() && given == initial.stamp()
    ));
    assert!(matches!(
        current.position_at(old_beat),
        MapQuery::Stale { expected, given }
            if expected == current.stamp() && given == initial.stamp()
    ));
}

#[kithara::test]
fn overlapping_segments_are_rejected_without_publication() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 96_001);
    let base = map.snapshot();
    let facts = metered(
        BeatEvidence::Observed,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let result = publisher.publish(AssetMapUpdate::new(
        base.stamp(),
        MapState::Building,
        vec![
            MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
                .expect("invariant: first segment is valid alone"),
            MapSegment::new(observed(24_000.0, 1), observed(72_000.0, 3), facts)
                .expect("invariant: second segment is valid alone"),
        ],
    ));

    assert!(matches!(
        result,
        Err(AssetMapPublishError::InvalidSegments(
            SegmentError::Overlap { index: 1 }
        ))
    ));
    assert_eq!(map.snapshot(), base);
}

#[kithara::test]
fn one_sided_segment_boundaries_are_rejected_without_publication() {
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
        let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 96_001);
        let base = map.snapshot();

        let result = publisher.publish(AssetMapUpdate::new(
            base.stamp(),
            MapState::Building,
            vec![first.clone(), second],
        ));

        assert!(matches!(
            result,
            Err(AssetMapPublishError::InvalidSegments(
                SegmentError::NonInvertibleBoundary { index: 1 }
            ))
        ));
        assert_eq!(map.snapshot(), base);
    }
}

#[kithara::test]
fn sparse_snapshot_is_honest_and_invertible() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 120_001);
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
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
        ))
        .expect("invariant: sparse fixture map is valid");

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
fn progressive_extrapolation_is_immediate_and_refines_immutably() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 144_001);
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let low = uncertainty(2.0);
    let high = uncertainty(480.0);
    let first = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
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
        ))
        .expect("invariant: first progressive fixture revision is valid");
    let probe = asset_frame(72_000.0);
    let first_estimate = resolved(first.beat_at(MapPoint::new(first.stamp(), probe)));
    assert_eq!(f64::from(*first_estimate.value().value()), 3.0);
    assert_eq!(first_estimate.evidence(), BeatEvidence::Extrapolated);
    assert_eq!(first_estimate.uncertainty(), high);

    let refined = publisher
        .publish(AssetMapUpdate::new(
            first.stamp(),
            MapState::Building,
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
                .expect("invariant: retained observed fixture segment is valid"),
                MapSegment::new(
                    observed(48_000.0, 2),
                    observed(96_000.0, 4),
                    SegmentFacts::new(
                        BeatEvidence::Observed,
                        low,
                        Some(MeterFacts::new(meter, BeatEvidence::Observed, low)),
                    ),
                )
                .expect("invariant: refined observed fixture segment is valid"),
                MapSegment::new(
                    marker(96_000.0, 4, BeatEvidence::Extrapolated),
                    marker(144_000.0, 6, BeatEvidence::Extrapolated),
                    SegmentFacts::new(
                        BeatEvidence::Extrapolated,
                        high,
                        Some(MeterFacts::new(meter, BeatEvidence::Extrapolated, high)),
                    ),
                )
                .expect("invariant: retained extrapolated fixture segment is valid"),
            ],
        ))
        .expect("invariant: refined progressive fixture revision is valid");
    let refined_estimate = resolved(refined.beat_at(MapPoint::new(refined.stamp(), probe)));
    assert_eq!(f64::from(*refined_estimate.value().value()), 3.0);
    assert_eq!(refined_estimate.evidence(), BeatEvidence::Observed);
    assert_eq!(refined_estimate.uncertainty(), low);

    let old_estimate = resolved(first.beat_at(MapPoint::new(first.stamp(), probe)));
    assert_eq!(old_estimate.evidence(), BeatEvidence::Extrapolated);
    assert_eq!(old_estimate.uncertainty(), high);
    assert_eq!(old_estimate.stamp(), first.stamp());
}

#[kithara::test]
fn late_meter_lane_rebases_over_latest_beat_revision() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let base = map.snapshot();
    let tempo_only = publisher
        .publish(AssetMapUpdate::new(
            base.stamp(),
            MapState::Building,
            vec![
                MapSegment::new(
                    observed(0.0, 0),
                    observed(48_000.0, 2),
                    SegmentFacts::new(BeatEvidence::Interpolated, exact(), None),
                )
                .expect("invariant: tempo-only fixture segment is valid"),
            ],
        ))
        .expect("invariant: tempo-only fixture revision is valid");
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let stale_result = publisher.publish(AssetMapUpdate::new(
        base.stamp(),
        MapState::Building,
        vec![
            MapSegment::new(
                observed(0.0, 0),
                observed(48_000.0, 2),
                metered(BeatEvidence::Interpolated, meter),
            )
            .expect("invariant: stale metered fixture segment is valid"),
        ],
    ));
    assert_eq!(
        stale_result,
        Err(AssetMapPublishError::Stale {
            expected: tempo_only.stamp(),
            given: base.stamp(),
        })
    );
    assert_eq!(map.snapshot(), tempo_only);

    let probe = asset_frame(24_000.0);
    let tempo_before = resolved(tempo_only.tempo_at(MapPoint::new(tempo_only.stamp(), probe)));
    let rebased = publisher
        .publish(AssetMapUpdate::new(
            tempo_only.stamp(),
            MapState::Building,
            vec![
                MapSegment::new(
                    observed(0.0, 0),
                    observed(48_000.0, 2),
                    metered(BeatEvidence::Interpolated, meter),
                )
                .expect("invariant: rebased metered fixture segment is valid"),
            ],
        ))
        .expect("invariant: rebased meter revision is valid");
    let tempo_after = resolved(rebased.tempo_at(MapPoint::new(rebased.stamp(), probe)));
    assert_eq!(tempo_before.value(), tempo_after.value());
    assert_eq!(
        *resolved(rebased.meter_at(MapPoint::new(
            rebased.stamp(),
            Beat::new(1.0).expect("invariant: fixture beat is finite"),
        )))
        .value(),
        meter
    );
    assert_eq!(
        tempo_only
            .segments()
            .map(SegmentSet::segments)
            .map(<[_]>::len),
        Some(1)
    );
    assert_eq!(
        rebased.segments().map(SegmentSet::segments).map(<[_]>::len),
        Some(1)
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

fn observe_plan_transition(transition: &PlanTransition) {
    match transition {
        PlanTransition::Unchanged | PlanTransition::Replace { .. } => {}
        _ => {}
    }
}

#[kithara::test]
fn beat_map_exposes_object_safe_alignment_and_reconciliation() {
    let _align_contract: fn(
        &dyn BeatMap,
        &dyn BeatMap,
        AlignmentRequest,
    ) -> Result<AlignmentPlan, SyncError> = align_maps;
    let _reconcile_contract: fn(
        &dyn BeatMap,
        &dyn BeatMap,
        &AlignmentPlan,
        PresentationFrontier,
    ) -> Result<PlanTransition, SyncError> = reconcile_maps;
    let _transition_contract: fn(&PlanTransition) = observe_plan_transition;
}

#[kithara::test]
fn asset_host_and_group_fake_satisfy_one_object_safe_contract() {
    let meter = Meter::new(4).expect("invariant: fixture meter is valid");
    let (asset, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let asset_snapshot = publisher
        .publish(AssetMapUpdate::new(
            asset.snapshot().stamp(),
            MapState::Building,
            vec![
                MapSegment::new(
                    observed(0.0, 0),
                    observed(48_000.0, 2),
                    metered(BeatEvidence::Interpolated, meter),
                )
                .expect("invariant: fixture asset topology is valid"),
            ],
        ))
        .expect("invariant: fixture asset map is valid");
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
    let group_id = map_id();
    let group_axis = MapAxis::Asset(AssetAxis::new(sample_rate(), 48_001));
    let group_snapshot = BeatMapSnapshot::try_from((
        group_id,
        BeatMapRevision::first()
            .checked_next()
            .expect("invariant: the second group-map revision exists"),
        MapState::Building,
        SegmentSet::new(
            group_axis,
            vec![
                MapSegment::new(
                    observed(0.0, 0),
                    observed(48_000.0, 2),
                    metered(BeatEvidence::Interpolated, meter),
                )
                .expect("invariant: fixture group topology is valid"),
            ],
        )
        .expect("invariant: fixture group segment set is valid"),
    ))
    .expect("invariant: fixture group snapshot is valid");
    assert_ne!(group_snapshot.id(), asset_snapshot.id());
    assert_ne!(group_snapshot.stamp(), asset_snapshot.stamp());
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
        MapState::Unavailable(MapUnavailable::AxisMismatch),
        MapState::Unavailable(MapUnavailable::NoMeter),
    ] {
        assert_eq!(
            BeatMapSnapshot::try_from((
                map_id(),
                BeatMapRevision::first(),
                state,
                segments.clone(),
            )),
            Err(BeatMapSnapshotError::InvalidState { axis, state })
        );
    }

    let host_axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(2)));
    let host_segments = SegmentSet::new(host_axis, Vec::new())
        .expect("invariant: an empty host fixture segment set is valid");
    assert_eq!(
        BeatMapSnapshot::try_from((
            map_id(),
            BeatMapRevision::first(),
            MapState::Complete,
            host_segments,
        )),
        Err(BeatMapSnapshotError::InvalidState {
            axis: host_axis,
            state: MapState::Complete,
        })
    );
}

#[kithara::test]
fn empty_host_segment_snapshot_reports_a_host_native_gap() {
    let axis = MapAxis::Host(HostAxis::new(sample_rate(), HostEpoch::new(3)));
    let snapshot = BeatMapSnapshot::try_from((
        map_id(),
        BeatMapRevision::first(),
        MapState::Building,
        SegmentSet::new(axis, Vec::new())
            .expect("invariant: an empty host fixture segment set is valid"),
    ))
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
    let snapshot = BeatMapSnapshot::unavailable(map_id(), revision, axis);
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
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 96_001);
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
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
        ))
        .expect("invariant: touching fixture segments are valid");
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
    let (asset, mut publisher) = AssetBeatMap::new(map_id(), source_rate, 44_101);
    let asset_snapshot = publisher
        .publish(AssetMapUpdate::new(
            asset.snapshot().stamp(),
            MapState::Complete,
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
        ))
        .expect("invariant: source-native map is valid");
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
        asset_snapshot.axis(),
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
fn marker_beyond_asset_extent_is_rejected_without_publication() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_000);
    let base = map.snapshot();
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: segment is valid before bounded-axis validation");

    let result = publisher.publish(AssetMapUpdate::new(
        base.stamp(),
        MapState::Building,
        vec![segment],
    ));

    assert!(matches!(
        result,
        Err(AssetMapPublishError::InvalidSegments(
            SegmentError::OutsideExtent { index: 0 }
        ))
    ));
    assert_eq!(map.snapshot(), base);
}

#[kithara::test]
fn large_asset_extent_uses_exact_integer_boundary_semantics() {
    const FIRST_INEXACT_U64: u64 = 9_007_199_254_740_993;
    const LAST_REPRESENTABLE_BELOW: f64 = 9_007_199_254_740_992.0;
    const FIRST_REPRESENTABLE_ABOVE: f64 = 9_007_199_254_740_994.0;
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), FIRST_INEXACT_U64);
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
    let snapshot = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Building,
            vec![valid],
        ))
        .expect("the last representable frame below the extent must remain valid");
    let outside = MapSegment::new(
        observed(0.0, 0),
        observed(FIRST_REPRESENTABLE_ABOVE, 2),
        facts,
    )
    .expect("invariant: outside segment is valid before extent validation");

    let result = publisher.publish(AssetMapUpdate::new(
        snapshot.stamp(),
        MapState::Building,
        vec![outside],
    ));

    assert!(matches!(
        result,
        Err(AssetMapPublishError::InvalidSegments(
            SegmentError::OutsideExtent { index: 0 }
        ))
    ));
    assert_eq!(map.snapshot(), snapshot);
}

#[kithara::test]
fn segment_with_unrepresentable_tempo_is_rejected_without_publication() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 1);
    let base = map.snapshot();
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(f64::MIN_POSITIVE, 1),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: tiny-span segment is ordered before topology validation");

    let result = publisher.publish(AssetMapUpdate::new(
        base.stamp(),
        MapState::Building,
        vec![segment],
    ));

    assert!(matches!(
        result,
        Err(AssetMapPublishError::InvalidSegments(
            SegmentError::InvalidTempo { index: 0 }
        ))
    ));
    assert_eq!(map.snapshot(), base);
}

#[kithara::test]
fn bounded_asset_map_rejects_incompatible_states_without_publication() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let base = map.snapshot();
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: fixture segment is valid");

    for state in [
        MapState::Live,
        MapState::Unavailable(MapUnavailable::AxisMismatch),
        MapState::Unavailable(MapUnavailable::NoMeter),
    ] {
        let result = publisher.publish(AssetMapUpdate::new(
            base.stamp(),
            state,
            vec![segment.clone()],
        ));

        assert!(matches!(
            result,
            Err(AssetMapPublishError::InvalidState { state: rejected }) if rejected == state
        ));
        assert_eq!(map.snapshot(), base);
    }
}

#[kithara::test]
fn complete_asset_map_cannot_return_to_building() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 48_001);
    let segment = MapSegment::new(
        observed(0.0, 0),
        observed(48_000.0, 2),
        metered(
            BeatEvidence::Interpolated,
            Meter::new(4).expect("invariant: fixture meter is valid"),
        ),
    )
    .expect("invariant: fixture segment is valid");
    let complete = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Complete,
            vec![segment.clone()],
        ))
        .expect("invariant: complete map publication is valid");

    let result = publisher.publish(AssetMapUpdate::new(
        complete.stamp(),
        MapState::Building,
        vec![segment],
    ));

    assert!(matches!(
        result,
        Err(AssetMapPublishError::InvalidTransition {
            from: MapState::Complete,
            to: MapState::Building,
        })
    ));
    assert_eq!(map.snapshot(), complete);
}

#[kithara::test]
fn complete_asset_map_can_refine_but_cannot_change_coverage() {
    let (map, mut publisher) = AssetBeatMap::new(map_id(), sample_rate(), 96_001);
    let facts = metered(
        BeatEvidence::Interpolated,
        Meter::new(4).expect("invariant: fixture meter is valid"),
    );
    let complete = publisher
        .publish(AssetMapUpdate::new(
            map.snapshot().stamp(),
            MapState::Complete,
            vec![
                MapSegment::new(observed(0.0, 0), observed(48_000.0, 2), facts)
                    .expect("invariant: initial complete segment is valid"),
            ],
        ))
        .expect("invariant: initial complete map is valid");
    let expanded = MapSegment::new(observed(0.0, 0), observed(96_000.0, 4), facts)
        .expect("invariant: expanded segment is valid alone");

    let result = publisher.publish(AssetMapUpdate::new(
        complete.stamp(),
        MapState::Complete,
        vec![expanded],
    ));

    assert!(matches!(result, Err(AssetMapPublishError::CoverageChanged)));
    assert_eq!(map.snapshot(), complete);

    let reshaped = MapSegment::new(observed(0.0, 0), observed(48_000.0, 3), facts)
        .expect("invariant: beat-domain expansion is valid alone");
    let result = publisher.publish(AssetMapUpdate::new(
        complete.stamp(),
        MapState::Complete,
        vec![reshaped],
    ));

    assert!(matches!(result, Err(AssetMapPublishError::CoverageChanged)));
    assert_eq!(map.snapshot(), complete);

    let refined = publisher
        .publish(AssetMapUpdate::new(
            complete.stamp(),
            MapState::Complete,
            vec![
                MapSegment::new(observed(0.0, 0), observed(24_000.0, 1), facts)
                    .expect("invariant: first refined segment is valid"),
                MapSegment::new(observed(24_000.0, 1), observed(48_000.0, 2), facts)
                    .expect("invariant: second refined segment is valid"),
            ],
        ))
        .expect("same-coverage refinement must remain publishable");
    assert!(refined.revision() > complete.revision());
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
