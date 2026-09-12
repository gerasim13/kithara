use kithara_warp::{
    AssetFrame, Beat, BeatAlignment, BeatGridId, BeatGridQuery, BeatGridSnapshot, BeatGridState,
    MapPoint, MapPosition, MapRegion, Meter, PresentationFrontier, SessionBeat, SessionFrame,
    SyncOperationId, WarpMapRevision,
};
use num_traits::ToPrimitive;

/// A warp map admitted for one grid member and awaiting the renderer's
/// acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreparedSync {
    pub(crate) operation: SyncOperationId,
    pub(crate) warp_map: WarpMapRevision,
    pub(crate) activation: SessionFrame,
    pub(crate) activation_beat: SessionBeat,
    pub(crate) source: u64,
    pub(crate) target: BeatGridId,
}

/// The beat alignment of one grid member onto its owner's grid and the
/// owner-grid frame on which it becomes audible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MemberAlignment {
    pub(super) alignment: BeatAlignment,
    pub(super) activation: SessionFrame,
    pub(super) activation_beat: SessionBeat,
    pub(super) source: u64,
}

#[derive(Clone, Copy)]
pub(super) struct AlignmentPolicy {
    pub(super) playback_rate: Option<kithara_warp::RateTarget>,
    pub(super) align_downbeat: bool,
    pub(super) require_future_source: bool,
    pub(super) source_cue: Option<Beat>,
}

/// Aligns the member's beat under the frontier's source frame onto the next
/// whole owner beat after the frontier's output frame.
///
/// Returns the region whose geometry is still missing when either grid cannot
/// answer.
pub(super) fn align_member(
    owner: &BeatGridSnapshot,
    member: &BeatGridSnapshot,
    previous: Option<BeatAlignment>,
    frontier: PresentationFrontier,
    preparation_source: u64,
    policy: AlignmentPolicy,
) -> Result<MemberAlignment, MapRegion> {
    let AlignmentPolicy {
        playback_rate,
        align_downbeat,
        require_future_source,
        source_cue,
    } = policy;
    let source = MapPosition::Asset(
        preparation_source
            .to_f64()
            .and_then(|frame| AssetFrame::new(frame).ok())
            .unwrap_or_default(),
    );
    let frontier_output = MapPosition::Session(frontier.output());
    if member.state() != BeatGridState::Complete {
        return Err(MapRegion::point(source));
    }
    let member_origin = MapPoint::new(member.stamp(), source);
    let BeatGridQuery::Resolved(member_beat) = member.beat_at_or_next(member_origin) else {
        return Err(MapRegion::point(source));
    };
    let member_frontier_beat = *member_beat.value().value();
    let mut member_beat = source_cue.unwrap_or(
        whole_beat(member_frontier_beat, require_future_source)
            .ok_or_else(|| MapRegion::point(source))?,
    );
    let member_meter = (align_downbeat || source_cue.is_some())
        .then(|| member.meter_at(MapPoint::new(member.stamp(), member_beat)))
        .and_then(|query| match query {
            BeatGridQuery::Resolved(meter) => Some(*meter.value()),
            _ => None,
        });
    if source_cue.is_none()
        && let Some(meter) = member_meter
    {
        member_beat =
            next_downbeat(member_beat, meter, false).ok_or_else(|| MapRegion::point(source))?;
    }
    let BeatGridQuery::Resolved(position) =
        member.position_at(MapPoint::new(member.stamp(), member_beat))
    else {
        return Err(MapRegion::point(source));
    };
    let MapPosition::Asset(source_frame) = *position.value().value() else {
        return Err(MapRegion::point(source));
    };
    let source_frame = f64::from(source_frame)
        .round()
        .to_u64()
        .ok_or_else(|| MapRegion::point(source))?;
    let earliest_output = reachable_output(
        owner,
        previous,
        frontier,
        playback_rate,
        member_beat,
        source_frame,
    )
    .ok_or_else(|| MapRegion::point(frontier_output))?;
    let output = MapPosition::Session(earliest_output);
    let BeatGridQuery::Resolved(owner_beat) = owner.beat_at(MapPoint::new(owner.stamp(), output))
    else {
        return Err(MapRegion::point(output));
    };
    let mut owner_beat =
        whole_beat(*owner_beat.value().value(), false).ok_or_else(|| MapRegion::point(output))?;
    if let Some(member_meter) = member_meter {
        let owner_meter = match owner.meter_at(MapPoint::new(owner.stamp(), owner_beat)) {
            BeatGridQuery::Resolved(owner_meter) => *owner_meter.value(),
            _ => Meter::new(member_meter.beats_per_bar()).map_err(|_| MapRegion::point(output))?,
        };
        owner_beat = if source_cue.is_some() {
            matching_phase(owner_beat, owner_meter, member_beat, member_meter)
        } else {
            next_downbeat(owner_beat, owner_meter, false)
        }
        .ok_or_else(|| MapRegion::point(output))?;
    }
    let target = MapPoint::new(owner.stamp(), owner_beat);
    let BeatGridQuery::Resolved(position) = owner.position_at(target) else {
        return Err(MapRegion::point(output));
    };
    let MapPosition::Session(activation) = *position.value().value() else {
        return Err(MapRegion::point(output));
    };
    Ok(MemberAlignment {
        alignment: BeatAlignment::new(MapPoint::new(member.stamp(), member_beat), target),
        activation,
        activation_beat: SessionBeat::new(f64::from(owner_beat))
            .map_err(|_| MapRegion::point(output))?,
        source: source_frame,
    })
}

fn reachable_output(
    owner: &BeatGridSnapshot,
    previous: Option<BeatAlignment>,
    frontier: PresentationFrontier,
    playback_rate: Option<kithara_warp::RateTarget>,
    member_beat: Beat,
    source: u64,
) -> Option<SessionFrame> {
    if frontier.warp_map().is_some()
        && let Some(previous) = previous
    {
        let beat = f64::from(*previous.target().value()) + f64::from(member_beat)
            - f64::from(*previous.source().value());
        let beat = Beat::new(beat).ok()?;
        let BeatGridQuery::Resolved(position) =
            owner.position_at(MapPoint::new(owner.stamp(), beat))
        else {
            return None;
        };
        let MapPosition::Session(output) = *position.value().value() else {
            return None;
        };
        return Some(output.max(frontier.output()));
    }
    let Some(playback_rate) = playback_rate else {
        return Some(frontier.output());
    };
    let rate = f64::from(playback_rate.speed());
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }
    let source_frames = source.saturating_sub(frontier.source()).to_f64()?;
    let output_frames = (source_frames / rate).ceil().to_i64()?;
    Some(SessionFrame::new(
        i64::from(frontier.output()).checked_add(output_frames)?,
    ))
}

fn whole_beat(beat: Beat, strictly_after: bool) -> Option<Beat> {
    let beat = f64::from(beat);
    let mut whole = beat.ceil();
    if strictly_after && whole == beat {
        whole += 1.0;
    }
    Beat::new(whole).ok()
}

fn next_downbeat(beat: Beat, meter: Meter, strictly_after: bool) -> Option<Beat> {
    let ordinal = f64::from(beat).to_i64()?;
    let downbeat = i64::from(meter.downbeat());
    let beats_per_bar = i64::from(meter.beats_per_bar());
    let phase = (ordinal - downbeat).rem_euclid(beats_per_bar);
    let mut distance = (beats_per_bar - phase).rem_euclid(beats_per_bar);
    if strictly_after && distance == 0 {
        distance = beats_per_bar;
    }
    let ordinal = ordinal.checked_add(distance)?;
    Beat::try_from(kithara_warp::BeatOrdinal::new(ordinal)).ok()
}

fn matching_phase(
    owner_beat: Beat,
    owner_meter: Meter,
    member_beat: Beat,
    member_meter: Meter,
) -> Option<Beat> {
    let member_downbeat = Beat::try_from(member_meter.downbeat()).ok()?;
    let owner_downbeat = Beat::try_from(owner_meter.downbeat()).ok()?;
    let member_phase = (f64::from(member_beat) - f64::from(member_downbeat))
        .rem_euclid(f64::from(member_meter.beats_per_bar()));
    let owner_phase = (f64::from(owner_beat) - f64::from(owner_downbeat))
        .rem_euclid(f64::from(owner_meter.beats_per_bar()));
    let distance = (member_phase - owner_phase).rem_euclid(f64::from(owner_meter.beats_per_bar()));
    Beat::new(f64::from(owner_beat) + distance).ok()
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;
    use kithara_warp::{Beat, BeatOrdinal, Meter};

    use super::matching_phase;

    #[kithara::test]
    fn pickup_track_start_keeps_its_weak_beat_phase() {
        let source_meter = Meter::new(4)
            .expect("four beats per bar")
            .with_downbeat(BeatOrdinal::new(1));
        let host_meter = Meter::new(4).expect("four beats per bar");
        let source = Beat::new(0.0).expect("source beat zero");
        let host_frontier = Beat::new(1.0).expect("first eligible host beat");

        let target = matching_phase(host_frontier, host_meter, source, source_meter)
            .expect("pickup phase resolves");

        assert_eq!(f64::from(target), 3.0);
    }

    #[kithara::test]
    fn pickup_track_start_preserves_fractional_beat_phase() {
        let source_meter = Meter::new(4)
            .expect("four beats per bar")
            .with_downbeat(BeatOrdinal::new(1));
        let host_meter = Meter::new(4).expect("four beats per bar");
        let source = Beat::new(0.5).expect("fractional source beat");
        let host_frontier = Beat::new(1.0).expect("first eligible host beat");

        let target = matching_phase(host_frontier, host_meter, source, source_meter)
            .expect("pickup phase resolves");

        assert_eq!(f64::from(target), 3.5);
    }
}
