//! Audio pipeline library with decoding, effects, and resampling.
//!
//! - [`Audio`] — generic audio pipeline running in a separate thread
//! - [`AudioConfig`] — pipeline configuration
//! - [`ResamplerQuality`] - sample rate conversion quality
//! - `Audio` implements [`PcmReader`] for pull-based PCM consumers
//!
//! See the crate `README.md` for usage and `CONTEXT.md` for threading model and architecture.

#![forbid(unsafe_code)]
#![cfg_attr(all(rtsan, not(rtsan_standalone)), feature(sanitize))]

pub mod analysis;
mod audio;
mod blob;
pub mod effects;
mod exports;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
mod musical;
mod pipeline;
mod region;
pub(crate) mod renderer;
mod runtime;
mod traits;
mod waveform;

pub use audio::{Audio, SeekHandle};
pub use blob::frame::BlobError;
pub use effects::{
    eq::{EqBandConfig, EqEffect, FilterKind, IsolatorEq, generate_log_spaced_bands},
    limiter::{LimiterError, PeakLimiter},
    timestretch::StretchControls,
};
pub use exports::*;
pub use kithara_resampler::{
    NoResamplerBackend, ResamplerBackend, ResamplerOptions, ResamplerQuality,
};
pub use musical::{
    AlignmentCursor, AlignmentPlan, AlignmentPlanError, AlignmentPlanRevision, AlignmentRequest,
    AlignmentSource, AlignmentTransition, AssetAxis, AssetBeatMap, AssetFrame,
    AssetMapPublishError, AssetMapPublisher, AssetMapUpdate, Beat, BeatAlignment, BeatEstimate,
    BeatEvidence, BeatMap, BeatMapId, BeatMapIdAllocationError, BeatMapRevision, BeatMapSnapshot,
    BeatMapSnapshotError, BeatMarker, BeatOrdinal, BeatsPerMinute, CoordinateError,
    FrameUncertainty, HostAxis, HostBeatMap, HostEpoch, LoadGeneration, MapAxis,
    MapCoordinateError, MapPoint, MapPosition, MapQuery, MapRegion, MapSegment, MapStamp, MapState,
    MapUnavailable, Meter, MeterError, MeterFacts, PlanSpan, PlanSpanSlot, PlanTransition,
    PlannedRenderSpan, PresentationFrontier, ReconcileCause, RenderFrontier, RenderPlan,
    SegmentDraft, SegmentEndpoint, SegmentError, SegmentFacts, SegmentSet, SessionAnchor,
    SessionBeat, SessionFrame, SourceFrameRange, SyncAdmission, SyncApplied, SyncCapability,
    SyncError, SyncGroup, SyncGroupSnapshot, SyncGroupTopologyError, SyncIntent, SyncMember,
    SyncMemberKind, SyncMemberSnapshot, SyncOperation, SyncOperationId, SyncRejected,
    SyncStatusSnapshot, TopologyOperation, TopologyRevision, TopologyStamp, TransportOperation,
    TransportRevision,
};
pub use pipeline::{
    config::{AudioConfig, AudioDecoderConfig, ConsumerWakeMode, DecoderResamplerSettings},
    fetch::{EpochValidator, Fetch},
};
pub use region::{ActiveRegion, RegionPlan, RegionPlanError};
pub use renderer::{
    AudioWorkerHandle, AudioWorkerSource, EngineLoad, EngineLoadSnapshot, PreloadGate, ServiceClass,
};
pub use traits::{
    AudioEffect, ChunkOutcome, DecodeError, DecodeResult, PcmControl, PcmRead, PcmReader,
    PcmSession, PendingReason, ReadOutcome, SeekBegin, SeekOutcome,
};
pub use waveform::{AnalysisParams, BeatGrid, Bucket, GridSegment, bucket::Waveform};
