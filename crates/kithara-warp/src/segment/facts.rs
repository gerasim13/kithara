use std::num::NonZeroU16;

use crate::{BeatOrdinal, FrameUncertainty, MapPosition};

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

impl TryFrom<f64> for BeatsPerMinute {
    type Error = BeatsPerMinuteError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value.is_finite() && value > 0.0 {
            Ok(Self(value))
        } else {
            Err(BeatsPerMinuteError)
        }
    }
}

/// A tempo must be finite and greater than zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("tempo must be finite and greater than zero")]
pub struct BeatsPerMinuteError;

/// A beats-per-bar relation anchored to one canonical downbeat ordinal.
#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    fieldwork::Fieldwork,
)]
#[display("{beats_per_bar}@{downbeat}")]
#[fieldwork(opt_in, get, with)]
#[non_exhaustive]
pub struct Meter {
    /// Returns the beat ordinal defining bar phase for this meter region.
    #[field(get, copy, with)]
    downbeat: BeatOrdinal,
    beats_per_bar: NonZeroU16,
}

impl Meter {
    /// Creates a meter from its beats-per-bar count.
    ///
    /// # Errors
    ///
    /// Returns [`MeterError`] when `beats_per_bar` is zero.
    pub const fn new(beats_per_bar: u16) -> Result<Self, MeterError> {
        match NonZeroU16::new(beats_per_bar) {
            Some(beats_per_bar) => Ok(Self {
                beats_per_bar,
                downbeat: BeatOrdinal::new(0),
            }),
            None => Err(MeterError),
        }
    }

    /// Returns the number of beats in one bar.
    #[must_use]
    pub const fn beats_per_bar(self) -> u16 {
        self.beats_per_bar.get()
    }
}

/// A meter cannot contain zero beats per bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("meter must contain at least one beat per bar")]
pub struct MeterError;

/// An endpoint supplied by an analyzer before segment validation.
#[derive(Clone, Copy, Debug, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(opt_in, get)]
#[non_exhaustive]
pub struct BeatMarker {
    pub(super) evidence: BeatEvidence,
    pub(super) uncertainty: FrameUncertainty,
    /// Returns the grid-native marker position.
    #[field(get, copy)]
    pub(super) position: MapPosition,
    /// Returns the explicit musical ordinal, when known.
    #[field(get, copy)]
    pub(super) ordinal: Option<BeatOrdinal>,
}

impl BeatMarker {
    /// Creates a marker with optional exact musical identity.
    ///
    /// `None` preserves an observed timestamp whose musical span is not yet
    /// known. Such a marker cannot enter a validated [`crate::SegmentSet`].
    #[must_use]
    pub const fn new(
        position: MapPosition,
        ordinal: Option<BeatOrdinal>,
        evidence: BeatEvidence,
        uncertainty: FrameUncertainty,
    ) -> Self {
        Self {
            evidence,
            uncertainty,
            position,
            ordinal,
        }
    }
}

/// Evidence shared by the interior of one grid segment.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SegmentFacts {
    pub(super) evidence: BeatEvidence,
    pub(super) uncertainty: FrameUncertainty,
    pub(super) meter: Option<MeterFacts>,
}

/// Optional meter-lane fact carried independently from beat geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MeterFacts {
    pub(super) evidence: BeatEvidence,
    pub(super) uncertainty: FrameUncertainty,
    pub(super) meter: Meter,
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
            evidence,
            uncertainty,
            meter,
        }
    }

    pub(crate) const fn into_parts(self) -> (Meter, BeatEvidence, FrameUncertainty) {
        (self.meter, self.evidence, self.uncertainty)
    }
}
