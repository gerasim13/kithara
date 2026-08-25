use std::{cmp::Ordering, num::NonZeroU16};

use kithara_platform::sync::Arc;

use super::{
    AssetFrame, AxisKind, Beat, BeatOrdinal, FrameUncertainty, MapAxis, MapPosition, SessionFrame,
};

const SECONDS_PER_MINUTE: f64 = 60.0;

/// How a musical estimate was established.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum BeatEvidence {
    /// Declared by an authoritative virtual or transport relation.
    Declared,
    /// Directly supported by analysed signal evidence.
    Observed,
    /// Inferred between observed anchors.
    Interpolated,
    /// Extended beyond observed anchors.
    Extrapolated,
}

/// Tempo derived from one validated position-to-beat relation.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, derive_more::Into)]
pub struct BeatsPerMinute(f64);

impl BeatsPerMinute {
    pub(crate) fn new(value: f64) -> Option<Self> {
        (value.is_finite() && value > 0.0).then_some(Self(value))
    }
}

/// A beats-per-bar relation anchored to one canonical downbeat ordinal.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, derive_more::Display)]
#[display("{beats_per_bar}@{downbeat}")]
#[non_exhaustive]
pub struct Meter {
    beats_per_bar: NonZeroU16,
    downbeat: BeatOrdinal,
}

impl Meter {
    /// Creates a meter from its beats-per-bar count.
    ///
    /// # Errors
    ///
    /// Returns [`MeterError`] when `beats_per_bar` is zero.
    pub const fn new(beats_per_bar: u16) -> Result<Self, MeterError> {
        Self::with_downbeat(beats_per_bar, BeatOrdinal::new(0))
    }

    /// Creates a meter anchored to an explicit canonical downbeat.
    ///
    /// # Errors
    ///
    /// Returns [`MeterError`] when `beats_per_bar` is zero.
    pub const fn with_downbeat(
        beats_per_bar: u16,
        downbeat: BeatOrdinal,
    ) -> Result<Self, MeterError> {
        match NonZeroU16::new(beats_per_bar) {
            Some(beats_per_bar) => Ok(Self {
                beats_per_bar,
                downbeat,
            }),
            None => Err(MeterError),
        }
    }

    /// Returns the number of beats in one bar.
    #[must_use]
    pub const fn beats_per_bar(self) -> u16 {
        self.beats_per_bar.get()
    }

    /// Returns the beat ordinal defining bar phase for this meter region.
    #[must_use]
    pub const fn downbeat(self) -> BeatOrdinal {
        self.downbeat
    }
}

/// A meter cannot contain zero beats per bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("meter must contain at least one beat per bar")]
pub struct MeterError;

/// An endpoint supplied by an analyzer before segment validation.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct BeatMarker {
    position: MapPosition,
    ordinal: Option<BeatOrdinal>,
    evidence: BeatEvidence,
    uncertainty: FrameUncertainty,
}

impl BeatMarker {
    /// Creates a marker with optional exact musical identity.
    ///
    /// `None` preserves an observed timestamp whose musical span is not yet
    /// known. Such a marker cannot enter a validated [`SegmentSet`].
    #[must_use]
    pub const fn new(
        position: MapPosition,
        ordinal: Option<BeatOrdinal>,
        evidence: BeatEvidence,
        uncertainty: FrameUncertainty,
    ) -> Self {
        Self {
            position,
            ordinal,
            evidence,
            uncertainty,
        }
    }

    /// Returns the map-native marker position.
    #[must_use]
    pub const fn position(self) -> MapPosition {
        self.position
    }

    /// Returns the explicit musical ordinal, when known.
    #[must_use]
    pub const fn ordinal(self) -> Option<BeatOrdinal> {
        self.ordinal
    }
}

/// Evidence shared by the interior of one map segment.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SegmentFacts {
    evidence: BeatEvidence,
    uncertainty: FrameUncertainty,
    meter: Option<MeterFacts>,
}

/// Optional meter-lane fact carried independently from beat geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MeterFacts {
    meter: Meter,
    evidence: BeatEvidence,
    uncertainty: FrameUncertainty,
}

impl SegmentFacts {
    /// Creates the beat-geometry facts used inside a segment.
    ///
    /// `meter` is independent because tempo-only analysis must not fabricate
    /// downbeat or meter evidence.
    #[must_use]
    pub const fn new(
        evidence: BeatEvidence,
        uncertainty: FrameUncertainty,
        meter: Option<MeterFacts>,
    ) -> Self {
        Self {
            evidence,
            uncertainty,
            meter,
        }
    }
}

impl MeterFacts {
    /// Creates a meter fact with its own provenance and uncertainty.
    #[must_use]
    pub const fn new(meter: Meter, evidence: BeatEvidence, uncertainty: FrameUncertainty) -> Self {
        Self {
            meter,
            evidence,
            uncertainty,
        }
    }

    pub(crate) const fn into_parts(self) -> (Meter, BeatEvidence, FrameUncertainty) {
        (self.meter, self.evidence, self.uncertainty)
    }
}

/// Which segment endpoint failed validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SegmentEndpoint {
    /// The segment start.
    Start,
    /// The segment end.
    End,
}

/// A segment or segment collection is not a valid musical topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SegmentError {
    /// A timestamp lacks the ordinal needed to preserve musical distance.
    #[error("{endpoint:?} marker has no explicit musical ordinal")]
    MissingOrdinal { endpoint: SegmentEndpoint },
    /// Segment endpoints use different coordinate axes.
    #[error("segment endpoints must use the same coordinate axis")]
    MixedAxes,
    /// A musical ordinal cannot be represented exactly by the query axis.
    #[error("{endpoint:?} marker ordinal cannot be represented exactly")]
    InexactOrdinal { endpoint: SegmentEndpoint },
    /// The segment position span does not move forward.
    #[error("segment end position must follow its start position")]
    NonIncreasingPosition,
    /// The segment beat span does not move forward.
    #[error("segment end ordinal must follow its start ordinal")]
    NonIncreasingBeat,
    /// A segment does not match the snapshot axis.
    #[error("segment {index} does not match the snapshot coordinate axis")]
    AxisMismatch { index: usize },
    /// A segment reaches or exceeds the bounded asset extent.
    #[error("segment {index} is outside the bounded asset extent")]
    OutsideExtent { index: usize },
    /// The segment slope cannot produce a finite positive tempo.
    #[error("segment {index} has an unrepresentable tempo")]
    InvalidTempo { index: usize },
    /// A segment is not ordered after its predecessor.
    #[error("segment {index} begins before its predecessor")]
    OutOfOrder { index: usize },
    /// A segment overlaps its predecessor.
    #[error("segment {index} overlaps its predecessor")]
    Overlap { index: usize },
    /// Musical coordinates reverse between adjacent segments.
    #[error("segment {index} reverses the musical coordinate axis")]
    BeatOrder { index: usize },
    /// Adjacent segments touch in only one of the two coordinate spaces.
    #[error("segment {index} has a non-invertible boundary with its predecessor")]
    NonInvertibleBoundary { index: usize },
}

/// One validated affine relation between map positions and musical beats.
#[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct MapSegment {
    #[field(get, vis = "pub(crate)", copy)]
    start_position: MapPosition,
    #[field(get, vis = "pub(crate)", copy)]
    end_position: MapPosition,
    #[field(get, vis = "pub(crate)", copy)]
    start_beat: Beat,
    #[field(get, vis = "pub(crate)", copy)]
    end_beat: Beat,
    start_evidence: BeatEvidence,
    end_evidence: BeatEvidence,
    start_uncertainty: FrameUncertainty,
    end_uncertainty: FrameUncertainty,
    facts: SegmentFacts,
}

impl MapSegment {
    /// Validates one position-to-beat relation between two sparse markers.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] when the markers lack musical ordinals, mix
    /// axes, or do not advance in both coordinate spaces.
    pub fn new(
        start: BeatMarker,
        end: BeatMarker,
        facts: SegmentFacts,
    ) -> Result<Self, SegmentError> {
        let start_ordinal = start.ordinal.ok_or(SegmentError::MissingOrdinal {
            endpoint: SegmentEndpoint::Start,
        })?;
        let end_ordinal = end.ordinal.ok_or(SegmentError::MissingOrdinal {
            endpoint: SegmentEndpoint::End,
        })?;
        if start.position.kind() != end.position.kind() {
            return Err(SegmentError::MixedAxes);
        }
        if start.position.partial_cmp(&end.position) != Some(Ordering::Less) {
            return Err(SegmentError::NonIncreasingPosition);
        }
        if start_ordinal >= end_ordinal {
            return Err(SegmentError::NonIncreasingBeat);
        }
        let start_beat =
            Beat::try_from(start_ordinal).map_err(|_| SegmentError::InexactOrdinal {
                endpoint: SegmentEndpoint::Start,
            })?;
        let end_beat = Beat::try_from(end_ordinal).map_err(|_| SegmentError::InexactOrdinal {
            endpoint: SegmentEndpoint::End,
        })?;
        Ok(Self {
            start_position: start.position,
            end_position: end.position,
            start_beat,
            end_beat,
            start_evidence: start.evidence,
            end_evidence: end.evidence,
            start_uncertainty: start.uncertainty,
            end_uncertainty: end.uncertainty,
            facts,
        })
    }

    /// Returns the inclusive position region represented by this segment.
    #[must_use]
    pub const fn region(&self) -> MapRegion {
        MapRegion {
            start: self.start_position,
            end: self.end_position,
        }
    }

    pub(crate) const fn kind(&self) -> AxisKind {
        self.start_position.kind()
    }

    pub(crate) fn contains_position(&self, position: MapPosition) -> bool {
        position.kind() == self.kind()
            && position >= self.start_position
            && position <= self.end_position
    }

    pub(crate) fn contains_beat(&self, beat: Beat) -> bool {
        beat >= self.start_beat && beat <= self.end_beat
    }

    pub(crate) fn beat_at(
        &self,
        position: MapPosition,
    ) -> Option<(Beat, BeatEvidence, FrameUncertainty)> {
        if !self.contains_position(position) {
            return None;
        }
        if position == self.start_position {
            return Some((self.start_beat, self.start_evidence, self.start_uncertainty));
        }
        if position == self.end_position {
            return Some((self.end_beat, self.end_evidence, self.end_uncertainty));
        }
        let start = f64::try_from(self.start_position).ok()?;
        let end = f64::try_from(self.end_position).ok()?;
        let fraction = (f64::try_from(position).ok()? - start) / (end - start);
        let beat = (f64::from(self.end_beat) - f64::from(self.start_beat))
            .mul_add(fraction, f64::from(self.start_beat));
        Beat::new(beat)
            .ok()
            .map(|value| (value, self.facts.evidence, self.facts.uncertainty))
    }

    pub(crate) fn position_at(
        &self,
        beat: Beat,
    ) -> Option<(MapPosition, BeatEvidence, FrameUncertainty)> {
        if !self.contains_beat(beat) {
            return None;
        }
        if beat == self.start_beat {
            return Some((
                self.start_position,
                self.start_evidence,
                self.start_uncertainty,
            ));
        }
        if beat == self.end_beat {
            return Some((self.end_position, self.end_evidence, self.end_uncertainty));
        }
        let start = f64::from(self.start_beat);
        let end = f64::from(self.end_beat);
        let fraction = (f64::from(beat) - start) / (end - start);
        let start_position = f64::try_from(self.start_position).ok()?;
        let position = (f64::try_from(self.end_position).ok()? - start_position)
            .mul_add(fraction, start_position);
        MapPosition::on_axis(self.kind(), position)
            .map(|value| (value, self.facts.evidence, self.facts.uncertainty))
    }

    pub(crate) fn tempo_at(
        &self,
        axis: MapAxis,
        position: MapPosition,
    ) -> Option<(BeatsPerMinute, BeatEvidence, FrameUncertainty)> {
        if !self.contains_position(position) {
            return None;
        }
        self.tempo(axis)
            .map(|tempo| (tempo, self.facts.evidence, self.facts.uncertainty))
    }

    fn tempo(&self, axis: MapAxis) -> Option<BeatsPerMinute> {
        let frames =
            f64::try_from(self.end_position).ok()? - f64::try_from(self.start_position).ok()?;
        let beats = f64::from(self.end_beat) - f64::from(self.start_beat);
        let bpm = beats * f64::from(axis.sample_rate().get()) * SECONDS_PER_MINUTE / frames;
        BeatsPerMinute::new(bpm)
    }

    pub(crate) fn meter_at(&self, beat: Beat) -> Option<(Meter, BeatEvidence, FrameUncertainty)> {
        if !self.contains_beat(beat) {
            return None;
        }
        let facts = self.facts.meter?;
        Some((facts.meter, facts.evidence, facts.uncertainty))
    }
}

/// An inclusive map-native position region.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapRegion {
    start: MapPosition,
    end: MapPosition,
}

impl MapRegion {
    /// Returns the first position in the region.
    #[must_use]
    pub const fn start(self) -> MapPosition {
        self.start
    }

    /// Returns the last position in the region.
    #[must_use]
    pub const fn end(self) -> MapPosition {
        self.end
    }

    pub(crate) const fn point(position: MapPosition) -> Self {
        Self {
            start: position,
            end: position,
        }
    }

    pub(crate) const fn between(start: MapPosition, end: MapPosition) -> Self {
        Self { start, end }
    }
}

/// An immutable ordered collection of non-overlapping map segments.
///
/// When two segments touch in both coordinate spaces, their shared seam belongs
/// to the following segment. Forward and inverse queries therefore select the
/// same meter, evidence, and uncertainty at a relation change.
#[derive(Clone, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
pub struct SegmentSet {
    #[field(get, vis = "pub(crate)", copy)]
    axis: MapAxis,
    segments: Arc<[MapSegment]>,
}

impl SegmentSet {
    /// Validates ordered, non-overlapping segments for `axis`.
    ///
    /// # Errors
    ///
    /// Returns [`SegmentError`] for an axis or extent mismatch, unrepresentable
    /// tempo, ordering error, overlap, reversed musical coordinates, or a seam
    /// that touches in only one coordinate space.
    pub fn new(axis: MapAxis, segments: Vec<MapSegment>) -> Result<Self, SegmentError> {
        for (index, segment) in segments.iter().enumerate() {
            if segment.kind() != axis.kind() {
                return Err(SegmentError::AxisMismatch { index });
            }
            if let MapAxis::Asset(asset) = axis {
                let inside = match segment.end_position() {
                    MapPosition::Asset(frame) => asset.contains(frame),
                    MapPosition::Host(_) => false,
                };
                if !inside {
                    return Err(SegmentError::OutsideExtent { index });
                }
            }
            if segment.tempo(axis).is_none() {
                return Err(SegmentError::InvalidTempo { index });
            }
            let Some(previous) = index.checked_sub(1).and_then(|value| segments.get(value)) else {
                continue;
            };
            match segment
                .start_position()
                .partial_cmp(&previous.start_position())
            {
                Some(Ordering::Less) | None => return Err(SegmentError::OutOfOrder { index }),
                _ => {}
            }
            if segment.start_position() < previous.end_position() {
                return Err(SegmentError::Overlap { index });
            }
            if segment.start_beat() < previous.end_beat() {
                return Err(SegmentError::BeatOrder { index });
            }
            let position_touches = segment.start_position() == previous.end_position();
            let beat_touches = segment.start_beat() == previous.end_beat();
            if position_touches != beat_touches {
                return Err(SegmentError::NonInvertibleBoundary { index });
            }
        }
        Ok(Self {
            axis,
            segments: segments.into(),
        })
    }

    pub(crate) fn empty(axis: MapAxis) -> Self {
        Self {
            axis,
            segments: Arc::from([]),
        }
    }

    /// Returns all validated segments in coordinate order.
    #[must_use]
    pub fn segments(&self) -> &[MapSegment] {
        &self.segments
    }

    pub(crate) fn by_position(&self, position: MapPosition) -> Option<&MapSegment> {
        let upper = self
            .segments
            .partition_point(|segment| segment.start_position() <= position);
        upper
            .checked_sub(1)
            .and_then(|index| self.segments.get(index))
            .filter(|segment| segment.contains_position(position))
    }

    pub(crate) fn by_beat(&self, beat: Beat) -> Option<&MapSegment> {
        let upper = self
            .segments
            .partition_point(|segment| segment.start_beat() <= beat);
        upper
            .checked_sub(1)
            .and_then(|index| self.segments.get(index))
            .filter(|segment| segment.contains_beat(beat))
    }

    pub(crate) fn uncovered_region(&self, position: MapPosition) -> MapRegion {
        let upper = self
            .segments
            .partition_point(|segment| segment.start_position() <= position);
        let previous = upper
            .checked_sub(1)
            .and_then(|index| self.segments.get(index));
        let next = self.segments.get(upper);
        match (previous, next) {
            (Some(previous), Some(next)) => {
                MapRegion::between(previous.end_position(), next.start_position())
            }
            _ => MapRegion::point(position),
        }
    }

    pub(crate) fn uncovered_region_by_beat(&self, beat: Beat) -> MapRegion {
        let upper = self
            .segments
            .partition_point(|segment| segment.start_beat() <= beat);
        let previous = upper
            .checked_sub(1)
            .and_then(|index| self.segments.get(index));
        let next = self.segments.get(upper);
        match (previous, next) {
            (Some(previous), Some(next)) => {
                MapRegion::between(previous.end_position(), next.start_position())
            }
            (Some(previous), None) => MapRegion::point(previous.end_position()),
            (None, Some(next)) => MapRegion::point(next.start_position()),
            (None, None) => MapRegion::point(self.zero_position()),
        }
    }

    fn zero_position(&self) -> MapPosition {
        match self.axis {
            MapAxis::Asset(_) => MapPosition::Asset(AssetFrame::ZERO),
            MapAxis::Host(_) => MapPosition::Host(SessionFrame::new(0)),
        }
    }
}
