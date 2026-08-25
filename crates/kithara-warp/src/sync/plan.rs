use std::ops::Range;

use bon::Builder;

use super::{
    AlignmentPlanRevision, BeatAlignment, LoadGeneration, PresentationFrontier, RenderFrontier,
    SyncOperationId, TopologyStamp, TransportRevision,
};
use crate::SessionFrame;

/// Exact half-open decoded source-frame range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct SourceFrameRange {
    start: u64,
    end: u64,
}

impl SourceFrameRange {
    /// Returns the inclusive source-frame boundary.
    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    /// Returns the exclusive source-frame boundary.
    #[must_use]
    pub const fn end(self) -> u64 {
        self.end
    }

    fn len(self) -> u64 {
        self.end - self.start
    }
}

impl TryFrom<Range<u64>> for SourceFrameRange {
    type Error = AlignmentPlanError;

    fn try_from(range: Range<u64>) -> Result<Self, Self::Error> {
        if range.start > range.end {
            return Err(AlignmentPlanError::InvalidSourceRange {
                start: range.start,
                end: range.end,
            });
        }
        Ok(Self {
            start: range.start,
            end: range.end,
        })
    }
}

/// Source transport state from which an alignment is requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AlignmentSource {
    /// PCM has not become audible and may be positioned before playback.
    Prepared,
    /// PCM is already audible at the stated exact presentation frontier.
    Audible(PresentationFrontier),
}

/// Continuity rule selected by the synchronization operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AlignmentTransition {
    /// Prepare exact state before any affected PCM is audible.
    Immediate,
    /// Preserve the audible frontier and converge without a hard relocation.
    Continuous,
    /// Relocate exactly because the user explicitly requested a hard align.
    Snap,
}

/// Facts required to align one map with another at a stamped render boundary.
#[derive(Clone, Copy, Debug, PartialEq, Builder)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct AlignmentRequest {
    /// Synchronization operation being planned.
    operation: SyncOperationId,
    /// Stable-deck load generation being planned.
    load: LoadGeneration,
    /// Exact group topology being planned.
    topology: TopologyStamp,
    /// Exact session transport revision being planned.
    transport: TransportRevision,
    /// Stamped source-to-target beat correspondence.
    alignment: BeatAlignment,
    /// Whether the source is prepared or already audible.
    source: AlignmentSource,
    /// Exact output frame at which the plan may take effect.
    activation: SessionFrame,
    /// Required continuity behavior.
    transition: AlignmentTransition,
}

impl AlignmentRequest {
    /// Returns the synchronization operation being planned.
    #[must_use]
    pub const fn operation(self) -> SyncOperationId {
        self.operation
    }

    /// Returns the stable-deck load generation being planned.
    #[must_use]
    pub const fn load(self) -> LoadGeneration {
        self.load
    }

    /// Returns the exact group topology being planned.
    #[must_use]
    pub const fn topology(self) -> TopologyStamp {
        self.topology
    }

    /// Returns the exact session transport revision being planned.
    #[must_use]
    pub const fn transport(self) -> TransportRevision {
        self.transport
    }

    /// Returns the source-to-target beat correspondence.
    #[must_use]
    pub const fn alignment(self) -> BeatAlignment {
        self.alignment
    }

    /// Returns whether PCM is prepared or already audible.
    #[must_use]
    pub const fn source(self) -> AlignmentSource {
        self.source
    }

    /// Returns the exact output frame at which the plan may take effect.
    #[must_use]
    pub const fn activation(self) -> SessionFrame {
        self.activation
    }

    /// Returns the required continuity behavior.
    #[must_use]
    pub const fn transition(self) -> AlignmentTransition {
        self.transition
    }
}

/// One immutable finite map-to-map render plan.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AlignmentPlan {
    request: AlignmentRequest,
    revision: AlignmentPlanRevision,
    source: SourceFrameRange,
    output: Range<SessionFrame>,
    cursor: AlignmentCursor,
}

/// Result of reconciling a newer map observation with an active plan.
#[derive(Debug, PartialEq)]
#[must_use]
#[non_exhaustive]
pub enum PlanTransition {
    /// The active plan remains valid without an audible correction.
    Unchanged,
    /// A continuity-preserving successor must replace the active plan.
    Replace { plan: Box<AlignmentPlan> },
}

/// Immutable allocation-free frame-span protocol for a resident renderer.
/// The caller owns mutable progress and reusable span storage.
pub trait RenderPlan {
    /// Peeks the next exact render span without advancing `cursor`.
    ///
    /// # Errors
    ///
    /// Returns [`AlignmentPlanError`] for stale progress, evicted input,
    /// exhausted finite coverage, or arithmetic failure.
    fn next_span<'a>(
        &self,
        cursor: &AlignmentCursor,
        output_frames: usize,
        retained: SourceFrameRange,
        slot: &'a mut PlanSpanSlot,
    ) -> Result<PlanSpan<'a>, AlignmentPlanError>;

    /// Advances renderer-local progress after the complete span reached the
    /// final output ring. This records rendered progress, not presentation.
    ///
    /// # Errors
    ///
    /// Returns [`AlignmentPlanError`] for a stale or discontinuous span.
    fn commit_rendered(
        &self,
        cursor: &mut AlignmentCursor,
        span: &PlannedRenderSpan,
    ) -> Result<RenderFrontier, AlignmentPlanError>;
}

impl AlignmentPlan {
    /// Creates a one-to-one correction without application-owned playback rate.
    /// Transitions that require working synchronization are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`AlignmentPlanError`] when any exact planning input is invalid.
    pub fn identity(
        request: AlignmentRequest,
        revision: AlignmentPlanRevision,
        source: SourceFrameRange,
        output: Range<SessionFrame>,
    ) -> Result<Self, AlignmentPlanError> {
        if request.transition != AlignmentTransition::Immediate {
            return Err(AlignmentPlanError::NonIdentityTransition {
                transition: request.transition,
            });
        }
        let output_frames = output_frames(&output)?;
        let source_frames = usize::try_from(source.len())
            .map_err(|_| AlignmentPlanError::FrameArithmeticOverflow)?;
        if source_frames != output_frames {
            return Err(AlignmentPlanError::IdentityLengthMismatch {
                source_frames,
                output_frames,
            });
        }
        if request.activation() != output.start {
            return Err(AlignmentPlanError::ActivationBoundaryMismatch {
                activation: request.activation(),
                output_start: output.start,
            });
        }
        let presentation = PresentationFrontier::builder()
            .source(source.start)
            .output(output.start)
            .build();
        if let AlignmentSource::Audible(audible) = request.source
            && audible != presentation
        {
            return Err(AlignmentPlanError::AudibleFrontierMismatch {
                expected: presentation,
                given: audible,
            });
        }
        let frontier = RenderFrontier::builder()
            .source(source.start)
            .output(output.start)
            .build();
        let cursor = AlignmentCursor {
            frontier,
            request,
            revision,
        };
        Ok(Self {
            request,
            revision,
            source,
            output,
            cursor,
        })
    }

    /// Returns a fresh renderer-local cursor at the plan's first boundary.
    #[must_use]
    pub const fn cursor(&self) -> AlignmentCursor {
        self.cursor
    }

    /// Returns the immutable plan revision.
    #[must_use]
    pub const fn revision(&self) -> AlignmentPlanRevision {
        self.revision
    }

    /// Returns the complete stamped request compiled into this plan.
    #[must_use]
    pub const fn request(&self) -> AlignmentRequest {
        self.request
    }

    /// Returns the finite decoded-source coverage of this plan.
    #[must_use]
    pub const fn source(&self) -> SourceFrameRange {
        self.source
    }

    /// Returns the finite session-output coverage of this plan.
    #[must_use]
    pub const fn output(&self) -> &Range<SessionFrame> {
        &self.output
    }
}

impl RenderPlan for AlignmentPlan {
    fn next_span<'a>(
        &self,
        cursor: &AlignmentCursor,
        output_frames: usize,
        retained: SourceFrameRange,
        slot: &'a mut PlanSpanSlot,
    ) -> Result<PlanSpan<'a>, AlignmentPlanError> {
        slot.ready = None;
        if cursor.revision != self.revision {
            return Err(AlignmentPlanError::StaleCursor {
                expected: self.revision,
                given: cursor.revision,
            });
        }
        if cursor.request != self.request {
            return Err(AlignmentPlanError::StaleRequest {
                expected_operation: self.request.operation(),
                given_operation: cursor.request.operation(),
                expected_load: self.request.load(),
                given_load: cursor.request.load(),
                expected_topology: self.request.topology(),
                given_topology: cursor.request.topology(),
                expected_transport: self.request.transport(),
                given_transport: cursor.request.transport(),
            });
        }
        if cursor.frontier.output() == self.output.end
            && cursor.frontier.source() == self.source.end
        {
            return Ok(PlanSpan::Complete);
        }
        if output_frames == 0 {
            return Err(AlignmentPlanError::EmptyOutputSpan);
        }
        let output_start = cursor.frontier.output();
        let output_end = advance_output(output_start, output_frames)?;
        let source_start = cursor.frontier.source();
        let source_end = source_start
            .checked_add(
                u64::try_from(output_frames)
                    .map_err(|_| AlignmentPlanError::FrameArithmeticOverflow)?,
            )
            .ok_or(AlignmentPlanError::FrameArithmeticOverflow)?;
        if output_end > self.output.end || source_end > self.source.end {
            return Err(AlignmentPlanError::PlanExhausted {
                requested_output_frames: output_frames,
            });
        }
        let required = SourceFrameRange {
            start: source_start,
            end: source_end,
        };
        if required.start < retained.start {
            return Err(AlignmentPlanError::BehindWindow { required, retained });
        }
        if required.end > retained.end {
            return Ok(PlanSpan::Pending { required });
        }
        let ready = slot.ready.insert(PlannedRenderSpan {
            request: self.request,
            plan: self.revision,
            output: output_start..output_end,
            source: required,
        });
        Ok(PlanSpan::Ready(ready))
    }

    fn commit_rendered(
        &self,
        cursor: &mut AlignmentCursor,
        span: &PlannedRenderSpan,
    ) -> Result<RenderFrontier, AlignmentPlanError> {
        cursor.commit_rendered(span)
    }
}

/// Reusable caller-owned storage for one allocation-free render-span peek.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct PlanSpanSlot {
    ready: Option<PlannedRenderSpan>,
}

impl PlanSpanSlot {
    /// Creates an empty render-span slot.
    #[must_use]
    pub const fn new() -> Self {
        Self { ready: None }
    }
}

/// Pure-peek outcome for the next render block.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
#[non_exhaustive]
pub enum PlanSpan<'a> {
    /// One exact block is ready in the caller's reusable slot.
    Ready(&'a PlannedRenderSpan),
    /// The decoder has not retained the complete required range yet.
    Pending { required: SourceFrameRange },
    /// The finite plan has reached its exact terminal frontier.
    Complete,
}

/// One bounded exact-span request ready for the RT renderer.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct PlannedRenderSpan {
    request: AlignmentRequest,
    plan: AlignmentPlanRevision,
    output: Range<SessionFrame>,
    source: SourceFrameRange,
}

impl PlannedRenderSpan {
    /// Returns the exact session output-frame range.
    #[must_use]
    pub const fn output(&self) -> &Range<SessionFrame> {
        &self.output
    }

    /// Returns the required half-open decoded source-frame range.
    #[must_use]
    pub const fn source(&self) -> SourceFrameRange {
        self.source
    }
}

/// The only mutable renderer-local progress through one immutable alignment plan.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct AlignmentCursor {
    frontier: RenderFrontier,
    request: AlignmentRequest,
    revision: AlignmentPlanRevision,
}

impl AlignmentCursor {
    /// Returns the exact committed source/output boundary.
    #[must_use]
    pub const fn frontier(&self) -> RenderFrontier {
        self.frontier
    }

    /// Advances only renderer-local progress after a complete final-ring push.
    /// This is rendered progress, not callback presentation proof.
    ///
    /// # Errors
    ///
    /// Returns [`AlignmentPlanError`] for a stale or discontinuous span.
    fn commit_rendered(
        &mut self,
        span: &PlannedRenderSpan,
    ) -> Result<RenderFrontier, AlignmentPlanError> {
        if span.plan != self.revision {
            return Err(AlignmentPlanError::StaleCursor {
                expected: self.revision,
                given: span.plan,
            });
        }
        if span.request != self.request {
            return Err(AlignmentPlanError::StaleRequest {
                expected_operation: self.request.operation(),
                given_operation: span.request.operation(),
                expected_load: self.request.load(),
                given_load: span.request.load(),
                expected_topology: self.request.topology(),
                given_topology: span.request.topology(),
                expected_transport: self.request.transport(),
                given_transport: span.request.transport(),
            });
        }
        if span.output.start != self.frontier.output()
            || span.source.start != self.frontier.source()
        {
            return Err(AlignmentPlanError::CursorFrontierMismatch {
                expected: self.frontier,
                source_range: span.source,
                output: span.output.clone(),
            });
        }
        self.frontier = RenderFrontier::builder()
            .source(span.source.end)
            .output(span.output.end)
            .build();
        Ok(self.frontier)
    }
}

/// Exact alignment planning or cursor validation failed.
#[derive(Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum AlignmentPlanError {
    /// A decoded source range was reversed.
    #[error("decoded source range {start}..{end} must be ordered")]
    InvalidSourceRange { start: u64, end: u64 },
    /// A session output range was empty or reversed.
    #[error("session output range must be non-empty and ordered")]
    InvalidOutputRange,
    /// Identity correction requires equal source and output frame counts.
    #[error("identity plan has {source_frames} source frames and {output_frames} output frames")]
    IdentityLengthMismatch {
        source_frames: usize,
        output_frames: usize,
    },
    /// Identity correction cannot stand in for a real sync transition.
    #[error("identity plan cannot implement {transition:?} alignment")]
    NonIdentityTransition { transition: AlignmentTransition },
    /// An audible request did not start at the plan's first frontier.
    #[error("audible frontier is {given:?}, expected {expected:?}")]
    AudibleFrontierMismatch {
        expected: PresentationFrontier,
        given: PresentationFrontier,
    },
    /// The finite plan does not begin at its exact activation boundary.
    #[error("activation {activation:?} does not equal output start {output_start:?}")]
    ActivationBoundaryMismatch {
        activation: SessionFrame,
        output_start: SessionFrame,
    },
    /// Frame arithmetic overflowed a coordinate domain.
    #[error("alignment frame arithmetic overflowed")]
    FrameArithmeticOverflow,
    /// A render request contained no output frames.
    #[error("alignment render request must contain output frames")]
    EmptyOutputSpan,
    /// A cursor belongs to another immutable plan revision.
    #[error("cursor plan revision is {given}, expected {expected}")]
    StaleCursor {
        expected: AlignmentPlanRevision,
        given: AlignmentPlanRevision,
    },
    /// Renderer-local progress belongs to another stamped alignment request.
    #[error(
        "alignment request stamps are operation {given_operation}, load {given_load}, topology {given_topology:?}, transport {given_transport}; expected operation {expected_operation}, load {expected_load}, topology {expected_topology:?}, transport {expected_transport}"
    )]
    StaleRequest {
        expected_operation: SyncOperationId,
        given_operation: SyncOperationId,
        expected_load: LoadGeneration,
        given_load: LoadGeneration,
        expected_topology: TopologyStamp,
        given_topology: TopologyStamp,
        expected_transport: TransportRevision,
        given_transport: TransportRevision,
    },
    /// The requested block extends beyond finite plan coverage.
    #[error("requested {requested_output_frames} output frames beyond finite plan coverage")]
    PlanExhausted { requested_output_frames: usize },
    /// Required decoded input has already left the retained window.
    #[error("required source {required:?} is behind retained window {retained:?}")]
    BehindWindow {
        required: SourceFrameRange,
        retained: SourceFrameRange,
    },
    /// A rendered span did not begin at the cursor's committed frontier.
    #[error(
        "render span {source_range:?}/{output:?} does not begin at cursor frontier {expected:?}"
    )]
    CursorFrontierMismatch {
        expected: RenderFrontier,
        source_range: SourceFrameRange,
        output: Range<SessionFrame>,
    },
}

fn output_frames(output: &Range<SessionFrame>) -> Result<usize, AlignmentPlanError> {
    let start: i64 = output.start.into();
    let end: i64 = output.end.into();
    let frames = end
        .checked_sub(start)
        .filter(|frames| *frames > 0)
        .ok_or(AlignmentPlanError::InvalidOutputRange)?;
    usize::try_from(frames).map_err(|_| AlignmentPlanError::FrameArithmeticOverflow)
}

fn advance_output(
    start: SessionFrame,
    output_frames: usize,
) -> Result<SessionFrame, AlignmentPlanError> {
    let start: i64 = start.into();
    let frames =
        i64::try_from(output_frames).map_err(|_| AlignmentPlanError::FrameArithmeticOverflow)?;
    start
        .checked_add(frames)
        .map(SessionFrame::new)
        .ok_or(AlignmentPlanError::FrameArithmeticOverflow)
}
