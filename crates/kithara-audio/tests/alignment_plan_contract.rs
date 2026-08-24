use std::{num::NonZeroU32, ops::Range};

use kithara_audio::{
    AlignmentCursor, AlignmentPlan, AlignmentPlanError, AlignmentPlanRevision, AlignmentRequest,
    AlignmentSource, AlignmentTransition, AssetBeatMap, Beat, BeatAlignment, BeatMap, BeatMapId,
    BeatMapRevision, HostBeatMap, HostEpoch, LoadGeneration, MapPoint, PlanSpan, PlanSpanSlot,
    PlannedRenderSpan, PresentationFrontier, RenderFrontier, RenderPlan, SessionAnchor,
    SessionBeat, SessionFrame, SourceFrameRange, SyncOperationId, TopologyRevision, TopologyStamp,
    TransportRevision,
};
use kithara_stretch::{
    ElasticCapabilities, ElasticConfig, ElasticLatency, ElasticRateEnvelope, ElasticSpanConfig,
};
use kithara_test_utils::kithara;

const BLOCK_FRAMES: usize = 4;
const FRAME_LIMIT: usize = 64;

fn map_id() -> BeatMapId {
    BeatMapId::allocate().expect("invariant: test map identity space is available")
}

fn sample_rate() -> NonZeroU32 {
    NonZeroU32::new(48_000).expect("invariant: sample rate is non-zero")
}

fn host_map() -> HostBeatMap {
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: host beat is finite"),
        2.0,
        sample_rate(),
    )
    .expect("invariant: host relation is valid");
    HostBeatMap::new(
        map_id(),
        BeatMapRevision::first(),
        HostEpoch::new(1),
        anchor,
        None,
    )
}

fn alignment_context() -> (BeatAlignment, TopologyStamp) {
    let host = host_map();
    let host_snapshot = host.snapshot();
    let source_frames = u64::try_from(FRAME_LIMIT)
        .expect("invariant: fixture source extent fits the asset frame domain");
    let (asset, _) = AssetBeatMap::new(map_id(), sample_rate(), source_frames);
    let asset_snapshot = asset.snapshot();
    let zero = Beat::new(0.0).expect("invariant: zero beat is finite");
    let alignment = BeatAlignment::new(
        MapPoint::new(asset_snapshot.stamp(), zero),
        MapPoint::new(host_snapshot.stamp(), zero),
    );
    let topology = TopologyStamp::new(host.id(), TopologyRevision::first());
    (alignment, topology)
}

fn request(
    alignment: BeatAlignment,
    topology: TopologyStamp,
    source: AlignmentSource,
    activation: i64,
    transition: AlignmentTransition,
) -> AlignmentRequest {
    request_for_operation(
        SyncOperationId::first(),
        alignment,
        topology,
        source,
        activation,
        transition,
    )
}

fn request_for_operation(
    operation: SyncOperationId,
    alignment: BeatAlignment,
    topology: TopologyStamp,
    source: AlignmentSource,
    activation: i64,
    transition: AlignmentTransition,
) -> AlignmentRequest {
    AlignmentRequest::builder()
        .operation(operation)
        .load(LoadGeneration::first())
        .topology(topology)
        .transport(TransportRevision::first())
        .alignment(alignment)
        .source(source)
        .activation(SessionFrame::new(activation))
        .transition(transition)
        .build()
}

fn capabilities() -> ElasticCapabilities {
    let config = ElasticConfig::try_from((sample_rate().get(), 2, FRAME_LIMIT, FRAME_LIMIT))
        .expect("invariant: elastic engine shape is valid");
    let rate_envelope = ElasticRateEnvelope::try_from(0.5..=2.0)
        .expect("invariant: static planner rate envelope is valid");
    ElasticCapabilities::new(config, ElasticLatency::new(0, 0), rate_envelope)
}

fn span_config() -> ElasticSpanConfig {
    ElasticSpanConfig::try_from((1.0e-6, 1.0, 1.0))
        .expect("invariant: finite positive exact-span policy")
}

fn source(range: Range<u64>) -> SourceFrameRange {
    SourceFrameRange::try_from(range).expect("invariant: fixture source range is ordered")
}

fn output(end: i64) -> Range<SessionFrame> {
    SessionFrame::new(0)..SessionFrame::new(end)
}

fn plan(request: AlignmentRequest, revision: AlignmentPlanRevision, end: u64) -> AlignmentPlan {
    AlignmentPlan::identity(
        request,
        revision,
        source(0..end),
        output(i64::try_from(end).expect("invariant: fixture end fits the session clock")),
        capabilities(),
        span_config(),
    )
    .expect("invariant: identity plan fixture is valid")
}

fn ready(span: PlanSpan<'_>) -> &PlannedRenderSpan {
    match span {
        PlanSpan::Ready(span) => span,
        _ => panic!("expected a ready render span"),
    }
}

fn frontier(source: u64, output: i64) -> RenderFrontier {
    RenderFrontier::builder()
        .source(source)
        .output(SessionFrame::new(output))
        .build()
}

fn presented(source: u64, output: i64) -> PresentationFrontier {
    PresentationFrontier::builder()
        .source(source)
        .output(SessionFrame::new(output))
        .build()
}

#[kithara::test]
fn public_consumers_can_read_alignment_request_and_plan_coverage() {
    let (alignment, topology) = alignment_context();
    let operation = SyncOperationId::first();
    let load = LoadGeneration::first();
    let transport = TransportRevision::first();
    let source_state = AlignmentSource::Prepared;
    let activation = SessionFrame::new(2);
    let transition = AlignmentTransition::Immediate;
    let request = AlignmentRequest::builder()
        .operation(operation)
        .load(load)
        .topology(topology)
        .transport(transport)
        .alignment(alignment)
        .source(source_state)
        .activation(activation)
        .transition(transition)
        .build();

    assert_eq!(request.operation(), operation);
    assert_eq!(request.load(), load);
    assert_eq!(request.topology(), topology);
    assert_eq!(request.transport(), transport);
    assert_eq!(request.alignment(), alignment);
    assert_eq!(request.source(), source_state);
    assert_eq!(request.activation(), activation);
    assert_eq!(request.transition(), transition);

    let source_coverage = source(0..8);
    let output_coverage = activation..SessionFrame::new(10);
    let plan = AlignmentPlan::identity(
        request,
        AlignmentPlanRevision::first(),
        source_coverage,
        output_coverage.clone(),
        capabilities(),
        span_config(),
    )
    .expect("invariant: readable identity plan fixture is valid");

    assert_eq!(plan.request(), request);
    assert_eq!(plan.source(), source_coverage);
    assert_eq!(plan.output(), &output_coverage);
}

#[kithara::test]
fn identity_plan_requires_its_exact_activation_as_the_first_output_frame() {
    let (alignment, topology) = alignment_context();
    let activation = SessionFrame::new(2);
    let error = AlignmentPlan::identity(
        request(
            alignment,
            topology,
            AlignmentSource::Prepared,
            2,
            AlignmentTransition::Immediate,
        ),
        AlignmentPlanRevision::first(),
        source(0..8),
        output(8),
        capabilities(),
        span_config(),
    )
    .expect_err("an activation inside plan coverage would be rounded to a block boundary");

    assert!(matches!(
        error,
        AlignmentPlanError::ActivationBoundaryMismatch {
            activation: given,
            output_start,
        } if given == activation && output_start == SessionFrame::new(0)
    ));
}

#[kithara::test]
fn peek_is_pure_and_render_commit_advances_only_the_rendered_frontier() {
    let (alignment, topology) = alignment_context();
    let revision = AlignmentPlanRevision::first();
    let plan = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        revision,
        12,
    );
    let mut cursor: AlignmentCursor = plan.cursor();
    let initial = cursor.frontier();
    let retained = source(0..12);
    let mut first_slot = PlanSpanSlot::new();
    let mut repeated_slot = PlanSpanSlot::new();

    let first = ready(
        plan.next_span(&cursor, BLOCK_FRAMES, retained.clone(), &mut first_slot)
            .expect("invariant: the first identity span is plannable"),
    );
    let repeated = ready(
        plan.next_span(&cursor, BLOCK_FRAMES, retained.clone(), &mut repeated_slot)
            .expect("invariant: peeking twice remains plannable"),
    );

    assert_eq!(first, repeated);
    assert_eq!(cursor.frontier(), initial);

    let rendered = plan
        .commit_rendered(&mut cursor, first)
        .expect("invariant: a complete first render span commits");
    assert_eq!(rendered, frontier(4, 4));
    assert_eq!(cursor.frontier(), rendered);

    let second = ready(
        plan.next_span(&cursor, BLOCK_FRAMES, retained, &mut first_slot)
            .expect("invariant: the second identity span is plannable"),
    );
    assert_eq!(
        plan.commit_rendered(&mut cursor, second)
            .expect("invariant: a complete second render span commits"),
        frontier(8, 8),
        "renderer progress is not an audible acknowledgement",
    );
    assert_eq!(cursor.frontier(), frontier(8, 8));
}

#[kithara::test]
fn missing_future_source_is_pending_without_progress() {
    let (alignment, topology) = alignment_context();
    let plan = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Audible(presented(0, 0)),
            0,
            AlignmentTransition::Immediate,
        ),
        AlignmentPlanRevision::first(),
        12,
    );
    let cursor = plan.cursor();
    let initial = cursor.frontier();
    let mut slot = PlanSpanSlot::new();

    let required = match plan
        .next_span(&cursor, BLOCK_FRAMES, source(0..2), &mut slot)
        .expect("missing future source is a non-terminal plan state")
    {
        PlanSpan::Pending { required } => required,
        _ => panic!("expected a pending source range"),
    };

    assert_eq!(required, source(0..4));
    assert_eq!(cursor.frontier(), initial);
}

#[kithara::test]
fn empty_retained_source_is_pending_without_progress() {
    let (alignment, topology) = alignment_context();
    let plan = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        AlignmentPlanRevision::first(),
        12,
    );
    let cursor = plan.cursor();
    let initial = cursor.frontier();
    let retained = SourceFrameRange::try_from(0..0)
        .expect("an empty retained window is a valid decoder state");
    let mut slot = PlanSpanSlot::new();

    let required = match plan
        .next_span(&cursor, BLOCK_FRAMES, retained, &mut slot)
        .expect("an empty retained window is a non-terminal plan state")
    {
        PlanSpan::Pending { required } => required,
        _ => panic!("expected a pending source range"),
    };

    assert_eq!(required, source(0..4));
    assert_eq!(cursor.frontier(), initial);
}

#[kithara::test]
fn evicted_required_source_is_behind_window_without_progress() {
    let (alignment, topology) = alignment_context();
    let plan = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Audible(presented(0, 0)),
            0,
            AlignmentTransition::Immediate,
        ),
        AlignmentPlanRevision::first(),
        12,
    );
    let cursor = plan.cursor();
    let initial = cursor.frontier();
    let mut slot = PlanSpanSlot::new();

    let error = plan
        .next_span(&cursor, BLOCK_FRAMES, source(1..12), &mut slot)
        .expect_err("evicted required source cannot be recovered by the plan");

    assert!(matches!(error, AlignmentPlanError::BehindWindow { .. }));
    assert_eq!(cursor.frontier(), initial);
}

#[kithara::test]
fn cursor_from_another_plan_revision_is_stale() {
    let (alignment, topology) = alignment_context();
    let first_revision = AlignmentPlanRevision::first();
    let second_revision = first_revision
        .checked_next()
        .expect("invariant: fixture plan revision can advance");
    let first = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        first_revision,
        12,
    );
    let second = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        second_revision,
        12,
    );
    let cursor = first.cursor();
    let initial = cursor.frontier();
    let mut slot = PlanSpanSlot::new();

    let error = second
        .next_span(&cursor, BLOCK_FRAMES, source(0..12), &mut slot)
        .expect_err("a cursor cannot cross immutable plan revisions");

    assert!(matches!(error, AlignmentPlanError::StaleCursor { .. }));
    assert_eq!(cursor.frontier(), initial);
}

#[kithara::test]
fn cursor_from_another_operation_with_the_same_plan_revision_is_stale() {
    let (alignment, topology) = alignment_context();
    let first_operation = SyncOperationId::first();
    let second_operation = first_operation
        .checked_next()
        .expect("invariant: fixture operation identity can advance");
    let revision = AlignmentPlanRevision::first();
    let first = plan(
        request_for_operation(
            first_operation,
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        revision,
        12,
    );
    let second = plan(
        request_for_operation(
            second_operation,
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        revision,
        12,
    );

    let mut slot = PlanSpanSlot::new();
    let error = second
        .next_span(&first.cursor(), BLOCK_FRAMES, source(0..12), &mut slot)
        .expect_err("plan revision alone cannot authorize another operation");

    assert_eq!(
        error,
        AlignmentPlanError::StaleRequest {
            expected_operation: second_operation,
            given_operation: first_operation,
            expected_load: second.request().load(),
            given_load: first.request().load(),
            expected_topology: second.request().topology(),
            given_topology: first.request().topology(),
            expected_transport: second.request().transport(),
            given_transport: first.request().transport(),
        }
    );
}

#[kithara::test]
fn renderer_commit_reports_the_foreign_span_revision_in_the_given_field() {
    let (alignment, topology) = alignment_context();
    let first_revision = AlignmentPlanRevision::first();
    let second_revision = first_revision
        .checked_next()
        .expect("invariant: fixture plan revision can advance");
    let request = request(
        alignment,
        topology,
        AlignmentSource::Prepared,
        0,
        AlignmentTransition::Immediate,
    );
    let first = plan(request, first_revision, 12);
    let second = plan(request, second_revision, 12);
    let mut cursor = first.cursor();
    let mut slot = PlanSpanSlot::new();
    let span = ready(
        second
            .next_span(&second.cursor(), BLOCK_FRAMES, source(0..12), &mut slot)
            .expect("invariant: the foreign plan can prepare its own first span"),
    );

    let error = first
        .commit_rendered(&mut cursor, span)
        .expect_err("a renderer cursor cannot commit another plan revision");

    assert_eq!(
        error,
        AlignmentPlanError::StaleCursor {
            expected: first_revision,
            given: second_revision,
        }
    );
}

#[kithara::test]
fn identity_plan_rejects_alignment_transitions() {
    let (alignment, topology) = alignment_context();
    for transition in [AlignmentTransition::Continuous, AlignmentTransition::Snap] {
        let error = AlignmentPlan::identity(
            request(
                alignment,
                topology,
                AlignmentSource::Audible(presented(0, 0)),
                0,
                transition,
            ),
            AlignmentPlanRevision::first(),
            source(0..8),
            output(8),
            capabilities(),
            span_config(),
        )
        .expect_err("identity correction cannot implement a sync transition");

        assert!(matches!(
            error,
            AlignmentPlanError::NonIdentityTransition { .. }
        ));
    }
}

#[kithara::test]
fn finite_coverage_distinguishes_exhaustion_from_completion() {
    let (alignment, topology) = alignment_context();
    let plan = plan(
        request(
            alignment,
            topology,
            AlignmentSource::Prepared,
            0,
            AlignmentTransition::Immediate,
        ),
        AlignmentPlanRevision::first(),
        8,
    );
    let retained = source(0..8);
    let mut cursor = plan.cursor();
    let initial = cursor.frontier();
    let mut slot = PlanSpanSlot::new();

    let error = plan
        .next_span(&cursor, 9, retained.clone(), &mut slot)
        .expect_err("a block extending beyond finite coverage is exhausted");
    assert!(matches!(error, AlignmentPlanError::PlanExhausted { .. }));
    assert_eq!(cursor.frontier(), initial);

    let final_span = ready(
        plan.next_span(&cursor, 8, retained.clone(), &mut slot)
            .expect("the exact remaining coverage is plannable"),
    );
    plan.commit_rendered(&mut cursor, final_span)
        .expect("the exact remaining coverage commits");

    assert!(matches!(
        plan.next_span(&cursor, 1, retained, &mut slot)
            .expect("the exact terminal frontier is not an error"),
        PlanSpan::Complete
    ));
}
