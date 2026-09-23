use std::ops::Range;

use kithara_signal::SessionFrame;
use kithara_warp::{
    AssetFrame, Beat, BeatAlignment, BeatEvidence, BeatGridQuery, BeatGridSnapshot,
    BeatGridUnavailable, BeatOrdinal, GridProjectionError, MapPoint, MapPosition, MapRegion, Meter,
    PresentationFrontier, WarpMap, WarpMapRevision, WarpPlan, WarpPlanError,
};
use num_traits::ToPrimitive;

use crate::{AlignmentSource, SyncError};

/// Why a member cannot be placed on the group's beats now.
#[derive(Debug)]
pub(super) enum Missing {
    /// A later grid revision may publish the coverage the placement needs.
    Coverage(MapRegion),
    /// The placement is refused on the current facts.
    Refused(SyncError),
}

impl From<SyncError> for Missing {
    fn from(error: SyncError) -> Self {
        Self::Refused(error)
    }
}

impl From<GridProjectionError> for Missing {
    fn from(error: GridProjectionError) -> Self {
        Self::Refused(SyncError::Projection(Box::new(error)))
    }
}

/// The member beat and group beat that sound together, and the session frame
/// at which they do.
#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub(super) alignment: BeatAlignment,
    pub(super) activation: SessionFrame,
}

/// Places `member` on the first admissible `owner` beat inside `window`.
///
/// A member whose grid proves its bars enters on a downbeat in its own bar
/// phase; one whose grid proves only beats enters on a whole beat. A silent
/// member starts on its first such beat at or after its cue; an audible one
/// keeps playing and is carried to the member beat its live stream reaches no
/// earlier than the activation.
pub(super) fn place(
    owner: &BeatGridSnapshot,
    member: &BeatGridSnapshot,
    source: AlignmentSource,
    window: &Range<SessionFrame>,
) -> Result<Placement, Missing> {
    let (owner_beat, member_beat) = match source {
        AlignmentSource::Prepared(cue) => {
            let cue = MapPosition::Asset(cue);
            let first = whole_beat(member, member_beat_at_or_next(member, cue)?)?;
            let member_meter = bar_of(member, first, cue)?;
            let member_beat = next_downbeat(member, first, member_meter)?;
            let lower = MapPosition::Session(window.start);
            let reachable = whole_beat(owner, owner_beat_at(owner, lower)?)?;
            let owner_meter = owner_bar(owner, reachable, member_meter, lower)?;
            let owner_beat = matching_phase(reachable, owner_meter, member_beat, member_meter)
                .ok_or_else(|| outside(owner))?;
            (owner_beat, member_beat)
        }
        AlignmentSource::Audible(frontier) => {
            if let Some(given) = frontier.warp_map() {
                return Err(Missing::Refused(SyncError::UnknownWarpMap {
                    member_id: member.id(),
                    given,
                }));
            }
            let past_frontier = i64::from(frontier.output())
                .checked_add(1)
                .map(SessionFrame::new)
                .ok_or_else(|| outside(owner))?;
            let lower = MapPosition::Session(window.start.max(past_frontier));
            let reachable = whole_beat(owner, owner_beat_at(owner, lower)?)?;
            let heard = MapPosition::Asset(asset_frame(member, frontier.source())?);
            let heard_beat = whole_beat(member, member_beat_at_or_next(member, heard)?)?;
            let member_meter = bar_of(member, heard_beat, heard)?;
            let owner_meter = owner_bar(owner, reachable, member_meter, lower)?;
            let owner_beat = next_downbeat(owner, reachable, owner_meter)?;
            let activation = session_frame(owner, owner_beat, lower)?;
            let live = MapPosition::Asset(live_source(owner, member, frontier, activation)?);
            let live_beat = whole_beat(member, member_beat_at_or_next(member, live)?)?;
            let member_beat = matching_phase(live_beat, member_meter, owner_beat, owner_meter)
                .ok_or_else(|| outside(member))?;
            (owner_beat, member_beat)
        }
    };
    let end = window.end;
    let activation = session_frame(owner, owner_beat, MapPosition::Session(end))?;
    if activation >= end {
        return Err(Missing::Refused(SyncError::NoAdmissibleBoundary {
            member_id: member.id(),
            first: activation,
            end,
        }));
    }
    Ok(Placement {
        alignment: BeatAlignment::new(
            MapPoint::new(member.stamp(), member_beat),
            MapPoint::new(owner.stamp(), owner_beat),
        ),
        activation,
    })
}

/// Carries a prepared `alignment` onto the successor `owner` grid.
///
/// The member and group beats that sound together stay the same; only the
/// session frame at which the group reaches its beat moves. `None` when that
/// frame leaves the launch `window` the preparation was asked for.
pub(super) fn carry(
    owner: &BeatGridSnapshot,
    member: &BeatGridSnapshot,
    alignment: BeatAlignment,
    window: &Range<SessionFrame>,
) -> Result<Option<Placement>, Missing> {
    let owner_beat = *alignment.target().value();
    let activation = session_frame(owner, owner_beat, MapPosition::Session(window.start))?;
    if !window.contains(&activation) {
        return Ok(None);
    }
    Ok(Some(Placement {
        alignment: BeatAlignment::new(
            MapPoint::new(member.stamp(), *alignment.source().value()),
            MapPoint::new(owner.stamp(), owner_beat),
        ),
        activation,
    }))
}

/// Freezes `placement` as map revision `revision` and its activation plan.
pub(super) fn project(
    owner: &BeatGridSnapshot,
    member: &BeatGridSnapshot,
    placement: Placement,
    revision: WarpMapRevision,
) -> Result<(BeatAlignment, WarpPlan), Missing> {
    let map = WarpMap::projected(member.clone(), owner.clone(), placement.alignment, revision)?;
    let at = MapPosition::Session(placement.activation);
    match WarpPlan::new(map, placement.activation) {
        Ok(plan) => Ok((placement.alignment, plan)),
        Err(WarpPlanError::Source(query)) => {
            resolve(member, query, at).and_then(|_| Err(outside(member).into()))
        }
        Err(WarpPlanError::Rate(query)) => {
            resolve(member, query, at).and_then(|_| Err(outside(member).into()))
        }
        Err(_) => Err(outside(member).into()),
    }
}

/// Carries a grid refusal out as the reason a placement cannot be made.
///
/// `at` names the coverage a grid without any geometry would have to publish.
fn resolve<T>(
    grid: &BeatGridSnapshot,
    query: BeatGridQuery<T>,
    at: MapPosition,
) -> Result<T, Missing> {
    match query {
        BeatGridQuery::Resolved(value) => Ok(value),
        BeatGridQuery::Uncovered { required } => Err(Missing::Coverage(required)),
        BeatGridQuery::Unavailable(BeatGridUnavailable::NoGeometry) => {
            Err(Missing::Coverage(MapRegion::point(at)))
        }
        BeatGridQuery::Stale { expected, given } => {
            Err(Missing::Refused(SyncError::StaleGridRevision {
                current: expected,
                given,
            }))
        }
        _ => Err(Missing::Refused(outside(grid))),
    }
}

fn outside(grid: &BeatGridSnapshot) -> SyncError {
    SyncError::OutsideGrid { grid_id: grid.id() }
}

fn owner_beat_at(owner: &BeatGridSnapshot, position: MapPosition) -> Result<Beat, Missing> {
    let beat = resolve(
        owner,
        owner.beat_at(MapPoint::new(owner.stamp(), position)),
        position,
    )?;
    Ok(*beat.value().value())
}

fn member_beat_at_or_next(
    member: &BeatGridSnapshot,
    position: MapPosition,
) -> Result<Beat, Missing> {
    let query = member.beat_at_or_next(MapPoint::new(member.stamp(), position));
    let beat = resolve(member, query, position)?;
    Ok(*beat.value().value())
}

/// The bar a member proves at `beat`; `None` when it proves only beats.
fn bar_of(
    member: &BeatGridSnapshot,
    beat: Beat,
    at: MapPosition,
) -> Result<Option<Meter>, Missing> {
    match member.meter_at(MapPoint::new(member.stamp(), beat)) {
        BeatGridQuery::Resolved(meter) if meter.evidence() != BeatEvidence::Extrapolated => {
            Ok(Some(*meter.value()))
        }
        BeatGridQuery::Resolved(_) | BeatGridQuery::Unavailable(BeatGridUnavailable::NoMeter) => {
            Ok(None)
        }
        refusal => resolve(member, refusal, at).map(|_| None),
    }
}

/// The owner bar a member with `member_meter` enters in.
///
/// An owner that publishes no meter counts bars of the member's length from
/// its session origin.
fn owner_bar(
    owner: &BeatGridSnapshot,
    beat: Beat,
    member_meter: Option<Meter>,
    at: MapPosition,
) -> Result<Option<Meter>, Missing> {
    let Some(member_meter) = member_meter else {
        return Ok(None);
    };
    match owner.meter_at(MapPoint::new(owner.stamp(), beat)) {
        BeatGridQuery::Resolved(meter) => Ok(Some(*meter.value())),
        BeatGridQuery::Unavailable(BeatGridUnavailable::NoMeter) => {
            Ok(Some(member_meter.with_downbeat(BeatOrdinal::new(0))))
        }
        refusal => resolve(owner, refusal, at).map(|_| None),
    }
}

fn session_frame(
    owner: &BeatGridSnapshot,
    beat: Beat,
    at: MapPosition,
) -> Result<SessionFrame, Missing> {
    let position = resolve(
        owner,
        owner.position_at(MapPoint::new(owner.stamp(), beat)),
        at,
    )?;
    let MapPosition::Session(frame) = *position.value().value() else {
        return Err(Missing::Refused(outside(owner)));
    };
    Ok(frame)
}

fn asset_frame(member: &BeatGridSnapshot, frame: u64) -> Result<AssetFrame, Missing> {
    frame
        .to_f64()
        .and_then(|frame| AssetFrame::new(frame).ok())
        .ok_or_else(|| Missing::Refused(outside(member)))
}

/// The recording frame an unmapped audible member reaches at `activation`.
///
/// The live stream advances one recording second per session second, so the
/// span crosses the resampler once.
fn live_source(
    owner: &BeatGridSnapshot,
    member: &BeatGridSnapshot,
    frontier: PresentationFrontier,
    activation: SessionFrame,
) -> Result<AssetFrame, Missing> {
    let span = i64::from(activation)
        .checked_sub(i64::from(frontier.output()))
        .and_then(|span| span.to_f64())
        .ok_or_else(|| Missing::Refused(outside(owner)))?;
    let ratio =
        f64::from(member.axis().sample_rate().get()) / f64::from(owner.axis().sample_rate().get());
    let advanced = frontier
        .source()
        .to_f64()
        .map(|source| source + (span * ratio).ceil())
        .ok_or_else(|| Missing::Refused(outside(member)))?;
    AssetFrame::new(advanced).map_err(|_| Missing::Refused(outside(member)))
}

fn whole_beat(grid: &BeatGridSnapshot, beat: Beat) -> Result<Beat, Missing> {
    Beat::new(f64::from(beat).ceil()).map_err(|_| Missing::Refused(outside(grid)))
}

/// The first downbeat at or after the whole `beat`; every beat is one when
/// the grid proves no bar.
fn next_downbeat(
    grid: &BeatGridSnapshot,
    beat: Beat,
    meter: Option<Meter>,
) -> Result<Beat, Missing> {
    let Some(meter) = meter else {
        return Ok(beat);
    };
    let ordinal = f64::from(beat)
        .to_i64()
        .ok_or_else(|| Missing::Refused(outside(grid)))?;
    let distance =
        (i64::from(meter.downbeat()) - ordinal).rem_euclid(i64::from(meter.beats_per_bar()));
    ordinal
        .checked_add(distance)
        .and_then(|ordinal| Beat::try_from(BeatOrdinal::new(ordinal)).ok())
        .ok_or_else(|| Missing::Refused(outside(grid)))
}

/// The first beat at or after `beat` whose bar phase equals that of `other`.
///
/// A side without a proven bar counts every beat as its downbeat, so only the
/// fractional beat phase carries over.
fn matching_phase(
    beat: Beat,
    meter: Option<Meter>,
    other: Beat,
    other_meter: Option<Meter>,
) -> Option<Beat> {
    let phase = bar_phase(beat, meter)?;
    let other_phase = bar_phase(other, other_meter)?;
    let length = meter.map_or(1.0, |meter| f64::from(meter.beats_per_bar()));
    Beat::new(f64::from(beat) + (other_phase - phase).rem_euclid(length)).ok()
}

fn bar_phase(beat: Beat, meter: Option<Meter>) -> Option<f64> {
    let Some(meter) = meter else {
        return Some(f64::from(beat).rem_euclid(1.0));
    };
    let downbeat = Beat::try_from(meter.downbeat()).ok()?;
    Some((f64::from(beat) - f64::from(downbeat)).rem_euclid(f64::from(meter.beats_per_bar())))
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_warp::{Beat, BeatOrdinal, Meter};

    use super::matching_phase;

    fn pickup(source: f64) -> f64 {
        let source_meter = Meter::new(4)
            .expect("four beats per bar")
            .with_downbeat(BeatOrdinal::new(1));
        let host_meter = Meter::new(4).expect("four beats per bar");
        let source = Beat::new(source).expect("source beat");
        let host_frontier = Beat::new(1.0).expect("first eligible host beat");

        let target = matching_phase(host_frontier, Some(host_meter), source, Some(source_meter))
            .expect("pickup phase resolves");
        f64::from(target)
    }

    #[kithara::test]
    fn pickup_track_start_keeps_its_weak_beat_phase() {
        assert_eq!(pickup(0.0), 3.0);
    }

    #[kithara::test]
    fn pickup_track_start_preserves_fractional_beat_phase() {
        assert_eq!(pickup(0.5), 3.5);
    }
}
