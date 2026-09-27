#![cfg(not(target_arch = "wasm32"))]
#![forbid(unsafe_code)]

use kithara::{
    signal::{SessionEpoch, SessionFrame},
    warp::{
        AssetAxis, AssetExtent, AssetFrame, Beat, BeatAlignment, BeatEvidence, BeatGridId,
        BeatGridRevision, BeatGridSnapshot, BeatGridState, BeatMarker, BeatOrdinal,
        FrameUncertainty, GridSegment, MapAxis, MapPoint, MapPosition, MapSegment,
        PresentationFrontier, RegionPlan, RegionPlanError, RenderContext, SegmentFacts, SegmentSet,
        SessionAnchor, SessionBeat, StretchControls, Warp, WarpConfig, WarpMap, WarpMapRevision,
        WarpPlan,
    },
};
use kithara_test_utils::bufpool as test_pools;

#[cfg(feature = "playback")]
mod real_track;
mod region;
#[path = "grids.rs"]
pub mod test_grids;
