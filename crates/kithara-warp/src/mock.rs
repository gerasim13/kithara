use std::{iter, num::NonZeroU32};

use kithara_signal::{SessionEpoch, SessionFrame};
use num_traits::ToPrimitive;

use crate::{
    AssetAxis, AssetExtent, AssetFrame, Beat, BeatAlignment, BeatEvidence, BeatGridId,
    BeatGridRevision, BeatGridSnapshot, BeatGridState, BeatMarker, BeatOrdinal, FrameUncertainty,
    MapAxis, MapPoint, MapPosition, MapSegment, SegmentFacts, SegmentSet, SessionAnchor,
    SessionBeat, WarpMap, WarpMapRevision, WarpPlan,
};

mod consts {
    pub(super) const SECONDS_PER_MINUTE: f64 = 60.0;
}

/// Beats a fixture recording is analysed over.
pub const BEATS: i64 = 400;

/// Frames one beat of `bpm` occupies at `sample_rate`.
#[must_use]
pub fn beat_frames(bpm: f64, sample_rate: NonZeroU32) -> f64 {
    f64::from(sample_rate.get()) * consts::SECONDS_PER_MINUTE / bpm
}

/// One analysed recording at a steady `bpm`, on its own asset axis.
///
/// # Panics
///
/// Panics when `bpm` does not give finite, increasing beat frames.
#[must_use]
pub fn asset_grid(bpm: f64, sample_rate: NonZeroU32) -> BeatGridSnapshot {
    let last = beat_frames(bpm, sample_rate) * BEATS.to_f64().unwrap_or_default();
    let marker = |frame: f64, ordinal: i64| {
        BeatMarker::new(
            MapPosition::Asset(AssetFrame::new(frame).expect("invariant: fixture frame is finite")),
            Some(BeatOrdinal::new(ordinal)),
            BeatEvidence::Observed,
            FrameUncertainty::new(0.0).expect("zero uncertainty"),
        )
    };
    let segment = MapSegment::new(
        marker(0.0, 0),
        marker(last, BEATS),
        SegmentFacts::new(
            BeatEvidence::Observed,
            FrameUncertainty::new(0.0).expect("zero uncertainty"),
            None,
        ),
    )
    .expect("invariant: fixture markers form an increasing relation");
    let axis = AssetAxis::new(
        sample_rate,
        AssetExtent::Bounded(last.ceil().to_u64().unwrap_or_default() + 1),
    );
    BeatGridSnapshot::segments(
        BeatGridId::allocate().expect("invariant: fixture grid id can be allocated"),
        BeatGridRevision::first(),
        BeatGridState::Complete,
        SegmentSet::new(MapAxis::Asset(axis), vec![segment])
            .expect("invariant: fixture segments are valid"),
    )
    .expect("invariant: complete geometry is valid on the asset axis")
}

/// One live session grid running at a steady `bpm`.
///
/// # Panics
///
/// Panics when `bpm` is not a finite, invertible tempo.
#[must_use]
pub fn session_grid(bpm: f64, sample_rate: NonZeroU32) -> BeatGridSnapshot {
    let anchor = SessionAnchor::new(
        SessionFrame::new(0),
        SessionBeat::new(0.0).expect("invariant: fixture beat is finite"),
        bpm / consts::SECONDS_PER_MINUTE,
        sample_rate,
    )
    .expect("invariant: fixture tempo is invertible");
    BeatGridSnapshot::session(
        BeatGridId::allocate().expect("invariant: fixture grid id can be allocated"),
        BeatGridRevision::first(),
        SessionEpoch::new(0),
        anchor,
        None,
    )
}

/// A plan carrying one recording projected onto a host running at `host_bpm`.
///
/// # Panics
///
/// Panics when either tempo cannot form a grid or the projection does not resolve.
#[must_use]
pub fn projected_plan(source_bpm: f64, host_bpm: f64, sample_rate: NonZeroU32) -> WarpPlan {
    let source = asset_grid(source_bpm, sample_rate);
    let target = session_grid(host_bpm, sample_rate);
    let beat = Beat::new(0.0).expect("invariant: fixture cue is finite");
    let alignment = BeatAlignment::new(
        MapPoint::new(source.stamp(), beat),
        MapPoint::new(target.stamp(), beat),
    );
    let map = WarpMap::projected(source, target, alignment, WarpMapRevision::first())
        .expect("invariant: an asset grid projects onto a live session grid");
    WarpPlan::new(map, SessionFrame::new(0)).expect("invariant: initial projection resolves")
}

/// A recording marked by `(start, frames_per_beat, count)` spans, on an asset axis of
/// `frames` or just past the last marked beat.
///
/// # Panics
///
/// Panics when `spans` is empty, adjacent spans do not share their boundary, or the
/// marked beats do not form a valid model.
#[must_use]
pub fn asset_grid_over(
    spans: &[(f64, f64, i64)],
    frames: Option<u64>,
    sample_rate: NonZeroU32,
) -> BeatGridSnapshot {
    let rate = f64::from(sample_rate.get());
    let (first, _, _) = spans.first().expect("fixture has a marked span");
    let (_, last_spacing, _) = spans.last().expect("fixture has a marked span");
    let origin = kithara_beat::GridBeat {
        at: *first / rate,
        ordinal: 0,
        confidence: Some(1.0),
    };
    let mut end = *first;
    let mut ordinal = 0;
    let span_ends = spans.iter().map(|&(start, frames_per_beat, count)| {
        assert_eq!(start, end, "fixture marked spans share their boundary");
        end = count
            .to_f64()
            .unwrap_or_default()
            .mul_add(frames_per_beat, start);
        ordinal += count;
        kithara_beat::GridBeat {
            at: end / rate,
            ordinal,
            confidence: Some(1.0),
        }
    });
    let model = kithara_beat::BeatGridModel::try_from(kithara_beat::RawBeatGrid {
        schema_version: kithara_beat::SCHEMA_VERSION,
        model_id: "warp-spans".to_owned(),
        revision: 1,
        state: kithara_beat::BeatGridState::Final,
        duration: None,
        bpm: rate * consts::SECONDS_PER_MINUTE / last_spacing,
        beats: iter::once(origin).chain(span_ends).collect(),
        downbeats: Vec::new(),
        meter: None,
    })
    .expect("fixture marked beats form a valid model");
    let axis = AssetAxis::new(
        sample_rate,
        AssetExtent::Bounded(frames.unwrap_or_else(|| end.ceil().to_u64().unwrap_or_default() + 1)),
    );
    BeatGridSnapshot::model(
        BeatGridId::allocate().expect("invariant: fixture grid id can be allocated"),
        BeatGridRevision::first(),
        &model,
        axis,
    )
    .expect("fixture model materializes on its declared axis")
}

/// A live session grid whose beats are spaced by frame count.
///
/// # Panics
///
/// Panics when the spacing is not a finite, invertible tempo.
#[must_use]
pub fn session_grid_spaced(frames_per_beat: f64, sample_rate: NonZeroU32) -> BeatGridSnapshot {
    session_grid(
        f64::from(sample_rate.get()) * consts::SECONDS_PER_MINUTE / frames_per_beat,
        sample_rate,
    )
}

/// `source` projected onto `target` with beat zero of each aligned.
///
/// # Panics
///
/// Panics when the projection does not resolve.
#[must_use]
pub fn plan_over(source: BeatGridSnapshot, target: BeatGridSnapshot) -> WarpPlan {
    let beat = Beat::new(0.0).expect("fixture cue");
    let alignment = BeatAlignment::new(
        MapPoint::new(source.stamp(), beat),
        MapPoint::new(target.stamp(), beat),
    );
    let map = WarpMap::projected(source, target, alignment, WarpMapRevision::first())
        .expect("fixture projection");
    WarpPlan::new(map, SessionFrame::new(0)).expect("fixture activation")
}

/// Marked `spans` projected onto a session grid spaced by `host_frames_per_beat`.
///
/// # Panics
///
/// Panics under the conditions of [`asset_grid_over`] and [`plan_over`].
#[must_use]
pub fn spaced_plan(
    spans: &[(f64, f64, i64)],
    host_frames_per_beat: f64,
    sample_rate: NonZeroU32,
) -> WarpPlan {
    plan_over(
        asset_grid_over(spans, None, sample_rate),
        session_grid_spaced(host_frames_per_beat, sample_rate),
    )
}

/// Explicit replacement-map alignment at the already emitted boundary.
///
/// # Panics
///
/// Panics when either beat is not finite or the projection does not resolve.
#[must_use]
pub fn plan_over_at(
    source: BeatGridSnapshot,
    target: BeatGridSnapshot,
    source_beat: f64,
    target_beat: f64,
    output: SessionFrame,
) -> WarpPlan {
    let alignment = BeatAlignment::new(
        MapPoint::new(source.stamp(), Beat::new(source_beat).expect("source cue")),
        MapPoint::new(target.stamp(), Beat::new(target_beat).expect("target cue")),
    );
    let revision = WarpMapRevision::first()
        .checked_next()
        .expect("replacement revision");
    let map =
        WarpMap::projected(source, target, alignment, revision).expect("replacement projection");
    WarpPlan::new(map, output).expect("replacement activation")
}
