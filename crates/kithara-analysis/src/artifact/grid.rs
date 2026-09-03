use bon::Builder;
use kithara_warp::{
    AssetFrame, BeatEvidence, BeatGridId, BeatGridRevision, BeatGridSnapshot,
    BeatGridSnapshotError, BeatGridState, BeatMarker, BeatOrdinal, BeatsPerMinute,
    FrameUncertainty, MapAxis, MapCoordinateError, MapPosition, MapSegment, Meter, MeterFacts,
    SegmentError, SegmentFacts, SegmentSet,
};
use num_traits::cast::ToPrimitive;

use super::BeatArtifact;

/// Caller-owned facts required to publish an analysis artifact as a beat grid.
#[derive(Builder, Clone, Copy, Debug)]
#[builder(state_mod(vis = "pub"))]
#[non_exhaustive]
pub struct BeatGridConfig<'a> {
    artifact: &'a BeatArtifact,
    id: BeatGridId,
    revision: BeatGridRevision,
    axis: kithara_warp::AssetAxis,
    uncertainty: FrameUncertainty,
}

/// An analysis artifact cannot form a validated beat grid.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum BeatGridError {
    /// At least two beats are required to establish one affine relation.
    #[error("beat artifact must contain at least two beats")]
    InsufficientBeats,
    /// Beat markers must advance in source-frame order.
    #[error("beat markers must be strictly increasing")]
    NonIncreasingBeats,
    /// A source frame or inferred ordinal cannot be represented exactly.
    #[error("beat marker coordinate cannot be represented exactly")]
    UnrepresentableCoordinate,
    /// A downbeat marker is not present in the beat-marker lane.
    #[error("downbeat at source frame {frame} is not a beat marker")]
    DownbeatWithoutBeat { frame: u64 },
    /// The artifact tempo is not finite and positive.
    #[error("beat artifact tempo must be finite and positive")]
    InvalidTempo,
    /// A marker lies beyond the declared source extent.
    #[error("beat marker at frame {frame} exceeds source extent {extent}")]
    OutsideExtent { frame: u64, extent: u64 },
    /// A grid coordinate is invalid.
    #[error(transparent)]
    Coordinate(#[from] MapCoordinateError),
    /// Sparse beat segments violate grid topology.
    #[error(transparent)]
    Segment(#[from] SegmentError),
    /// The completed snapshot is invalid.
    #[error(transparent)]
    Snapshot(#[from] BeatGridSnapshotError),
}

impl TryFrom<BeatGridConfig<'_>> for BeatGridSnapshot {
    type Error = BeatGridError;

    fn try_from(config: BeatGridConfig<'_>) -> Result<Self, Self::Error> {
        let artifact = config.artifact;
        let _tempo =
            BeatsPerMinute::try_from(artifact.bpm()).map_err(|_| BeatGridError::InvalidTempo)?;
        if artifact.beats().len() < 2 {
            return Err(BeatGridError::InsufficientBeats);
        }
        validate_extent(artifact.beats(), config.axis.frame_count())?;
        validate_extent(artifact.downbeats(), config.axis.frame_count())?;

        let mut ordinals = infer_ordinals(artifact.beats(), config.axis, artifact.bpm())?;
        let downbeat_indexes = downbeat_indexes(artifact.beats(), artifact.downbeats())?;
        if let Some(&anchor) = downbeat_indexes.first() {
            let shift: i64 = ordinals[anchor].into();
            for ordinal in &mut ordinals {
                let value: i64 = (*ordinal).into();
                *ordinal = BeatOrdinal::new(
                    value
                        .checked_sub(shift)
                        .ok_or(BeatGridError::UnrepresentableCoordinate)?,
                );
            }
        }
        let meter = meter_facts(
            &ordinals,
            &downbeat_indexes,
            artifact.downbeat_confidence(),
            config.uncertainty,
        );
        let facts = SegmentFacts::new(BeatEvidence::Interpolated, config.uncertainty, meter);
        let mut segments: Vec<MapSegment> = Vec::with_capacity(artifact.beats().len() - 1);
        for index in 0..artifact.beats().len() - 1 {
            let start = marker(
                artifact.beats()[index],
                ordinals[index],
                artifact.beat_confidence()[index],
                config.uncertainty,
            )?;
            let end = marker(
                artifact.beats()[index + 1],
                ordinals[index + 1],
                artifact.beat_confidence()[index + 1],
                config.uncertainty,
            )?;
            segments.push(MapSegment::new(start, end, facts)?);
        }
        let segments = SegmentSet::new(MapAxis::Asset(config.axis), segments)?;
        Self::segments(
            config.id,
            config.revision,
            BeatGridState::Complete,
            segments,
        )
        .map_err(Into::into)
    }
}

fn validate_extent(markers: &[u64], extent: u64) -> Result<(), BeatGridError> {
    if let Some(&frame) = markers.iter().find(|&&frame| frame > extent) {
        return Err(BeatGridError::OutsideExtent { frame, extent });
    }
    Ok(())
}

fn infer_ordinals(
    beats: &[u64],
    axis: kithara_warp::AssetAxis,
    bpm: f64,
) -> Result<Vec<BeatOrdinal>, BeatGridError> {
    let frames_per_beat = f64::from(axis.sample_rate().get()) * 60.0 / bpm;
    let mut ordinals: Vec<BeatOrdinal> = Vec::with_capacity(beats.len());
    ordinals.push(BeatOrdinal::new(0));
    for pair in beats.windows(2) {
        let delta = pair[1]
            .checked_sub(pair[0])
            .filter(|delta| *delta > 0)
            .ok_or(BeatGridError::NonIncreasingBeats)?;
        let delta = exact_f64(delta)?;
        let step = (delta / frames_per_beat)
            .round()
            .to_i64()
            .filter(|step| *step > 0)
            .ok_or(BeatGridError::UnrepresentableCoordinate)?;
        let previous: i64 = ordinals
            .last()
            .copied()
            .ok_or(BeatGridError::UnrepresentableCoordinate)?
            .into();
        let next = previous
            .checked_add(step)
            .ok_or(BeatGridError::UnrepresentableCoordinate)?;
        ordinals.push(BeatOrdinal::new(next));
    }
    Ok(ordinals)
}

fn downbeat_indexes(beats: &[u64], downbeats: &[u64]) -> Result<Vec<usize>, BeatGridError> {
    downbeats
        .iter()
        .map(|&frame| {
            beats
                .binary_search(&frame)
                .map_err(|_| BeatGridError::DownbeatWithoutBeat { frame })
        })
        .collect()
}

fn meter_facts(
    ordinals: &[BeatOrdinal],
    downbeat_indexes: &[usize],
    confidence: &[Option<f32>],
    uncertainty: FrameUncertainty,
) -> Option<MeterFacts> {
    let mut spans = downbeat_indexes.windows(2).map(|pair| {
        let start: i64 = ordinals[pair[0]].into();
        let end: i64 = ordinals[pair[1]].into();
        end.checked_sub(start)
    });
    let beats_per_bar = spans.next().flatten()?;
    if !spans.all(|span| span == Some(beats_per_bar)) {
        return None;
    }
    let beats_per_bar = u16::try_from(beats_per_bar).ok()?;
    let meter = Meter::new(beats_per_bar).ok()?;
    let evidence = if confidence.iter().all(Option::is_some) {
        BeatEvidence::Observed
    } else {
        BeatEvidence::Extrapolated
    };
    Some(MeterFacts::new(meter, evidence, uncertainty))
}

fn marker(
    frame: u64,
    ordinal: BeatOrdinal,
    confidence: Option<f32>,
    uncertainty: FrameUncertainty,
) -> Result<BeatMarker, BeatGridError> {
    let position = AssetFrame::new(exact_f64(frame)?)?;
    let evidence = if confidence.is_some() {
        BeatEvidence::Observed
    } else {
        BeatEvidence::Extrapolated
    };
    Ok(BeatMarker::new(
        MapPosition::Asset(position),
        Some(ordinal),
        evidence,
        uncertainty,
    ))
}

fn exact_f64(value: u64) -> Result<f64, BeatGridError> {
    value
        .to_f64()
        .filter(|scalar| scalar.to_u64() == Some(value))
        .ok_or(BeatGridError::UnrepresentableCoordinate)
}
