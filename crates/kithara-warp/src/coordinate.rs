use std::{cmp::Ordering, num::NonZeroU32};

use num_traits::cast::ToPrimitive;

use super::{MapStamp, SessionFrame};

/// A value cannot represent a musical-map coordinate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum MapCoordinateError {
    /// The supplied coordinate is `NaN` or infinite.
    #[error("coordinate must be finite")]
    NonFinite,
    /// An asset coordinate was below the start of the asset.
    #[error("asset frame must not be negative")]
    NegativeAssetFrame,
    /// An uncertainty was below zero.
    #[error("frame uncertainty must not be negative")]
    NegativeUncertainty,
    /// An integral ordinal cannot be represented exactly as a continuous beat.
    #[error("beat ordinal cannot be represented exactly")]
    InexactBeatOrdinal,
    /// A signed host frame cannot be represented exactly as a scalar.
    #[error("host frame cannot be represented exactly")]
    InexactHostFrame,
}

/// A continuous frame coordinate in decoded asset-native audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, derive_more::Into)]
pub struct AssetFrame(f64);

impl AssetFrame {
    pub(crate) const ZERO: Self = Self(0.0);

    /// Creates a finite, non-negative asset-frame coordinate.
    ///
    /// # Errors
    ///
    /// Returns [`MapCoordinateError`] for a non-finite or negative value.
    pub fn new(value: f64) -> Result<Self, MapCoordinateError> {
        if !value.is_finite() {
            return Err(MapCoordinateError::NonFinite);
        }
        if value < 0.0 {
            return Err(MapCoordinateError::NegativeAssetFrame);
        }
        Ok(Self(value))
    }
}

/// A continuous beat coordinate in one musical map.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, derive_more::Into)]
pub struct Beat(f64);

impl Beat {
    /// Creates a finite beat coordinate. Negative beats are valid.
    ///
    /// # Errors
    ///
    /// Returns [`MapCoordinateError::NonFinite`] for a non-finite value.
    pub const fn new(value: f64) -> Result<Self, MapCoordinateError> {
        if value.is_finite() {
            Ok(Self(value))
        } else {
            Err(MapCoordinateError::NonFinite)
        }
    }
}

/// An exact integral beat identity carried by a sparse marker.
///
/// Ordinal zero is the canonical downbeat for maps using the default meter
/// origin. Pickups therefore use negative ordinals.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    derive_more::Display,
    derive_more::Into,
)]
#[display("{_0}")]
#[into(i64)]
#[repr(transparent)]
pub struct BeatOrdinal(i64);

impl BeatOrdinal {
    /// Creates an exact musical ordinal. Negative ordinals are valid.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }
}

impl TryFrom<BeatOrdinal> for Beat {
    type Error = MapCoordinateError;

    fn try_from(ordinal: BeatOrdinal) -> Result<Self, Self::Error> {
        let value = ordinal
            .0
            .to_f64()
            .ok_or(MapCoordinateError::InexactBeatOrdinal)?;
        if value.to_i64() == Some(ordinal.0) {
            Ok(Self(value))
        } else {
            Err(MapCoordinateError::InexactBeatOrdinal)
        }
    }
}

/// Maximum absolute error measured in the map's native frame axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, derive_more::Into)]
pub struct FrameUncertainty(f64);

impl FrameUncertainty {
    pub(crate) const ZERO: Self = Self(0.0);

    /// Creates a finite, non-negative uncertainty.
    ///
    /// # Errors
    ///
    /// Returns [`MapCoordinateError`] for a non-finite or negative value.
    pub fn new(value: f64) -> Result<Self, MapCoordinateError> {
        if !value.is_finite() {
            return Err(MapCoordinateError::NonFinite);
        }
        if value < 0.0 {
            return Err(MapCoordinateError::NegativeUncertainty);
        }
        Ok(Self(value))
    }
}

/// Monotonic generation of the session-frame axis used by a host map.
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
    derive_more::Into,
)]
#[display("{_0}")]
#[into(u64)]
#[repr(transparent)]
pub struct HostEpoch(u64);

impl HostEpoch {
    /// Creates a host epoch.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// The stable bounded coordinate axis of an analysed asset.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct AssetAxis {
    /// Returns the sample rate defining asset frames.
    #[field(get, copy)]
    sample_rate: NonZeroU32,
    /// Returns the exclusive decoded-asset frame bound.
    #[field(get, copy)]
    frame_count: u64,
}

impl AssetAxis {
    /// Creates a bounded asset-native coordinate axis.
    #[must_use]
    pub const fn new(sample_rate: NonZeroU32, frame_count: u64) -> Self {
        Self {
            sample_rate,
            frame_count,
        }
    }

    pub(crate) fn contains(self, frame: AssetFrame) -> bool {
        frame
            .0
            .floor()
            .to_u64()
            .is_some_and(|whole_frame| whole_frame < self.frame_count)
    }
}

/// The signed live coordinate axis of a session host.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, fieldwork::Fieldwork)]
#[fieldwork(get)]
#[non_exhaustive]
pub struct HostAxis {
    /// Returns the output sample rate defining host frames.
    #[field(get, copy)]
    sample_rate: NonZeroU32,
    /// Returns the generation of the signed session-frame axis.
    #[field(get, copy)]
    epoch: HostEpoch,
}

impl HostAxis {
    /// Creates a signed live host coordinate axis.
    #[must_use]
    pub const fn new(sample_rate: NonZeroU32, epoch: HostEpoch) -> Self {
        Self { sample_rate, epoch }
    }
}

/// The coordinate axis carried by one map snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum MapAxis {
    /// A bounded decoded-asset axis.
    Asset(AssetAxis),
    /// A signed live session-host axis.
    Host(HostAxis),
}

impl MapAxis {
    pub(crate) const fn kind(self) -> AxisKind {
        match self {
            Self::Asset(_) => AxisKind::Asset,
            Self::Host(_) => AxisKind::Host,
        }
    }

    /// Returns the sample rate defining this map-native frame axis.
    #[must_use]
    pub const fn sample_rate(self) -> NonZeroU32 {
        match self {
            Self::Asset(axis) => axis.sample_rate,
            Self::Host(axis) => axis.sample_rate,
        }
    }
}

/// A position tagged with its map-native coordinate axis.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum MapPosition {
    /// A position in decoded asset-native frames.
    Asset(AssetFrame),
    /// A position in signed session-host frames.
    Host(SessionFrame),
}

impl MapPosition {
    pub(crate) const fn kind(self) -> AxisKind {
        match self {
            Self::Asset(_) => AxisKind::Asset,
            Self::Host(_) => AxisKind::Host,
        }
    }

    pub(crate) fn on_axis(kind: AxisKind, value: f64) -> Option<Self> {
        match kind {
            AxisKind::Asset => AssetFrame::new(value).ok().map(Self::Asset),
            AxisKind::Host => value
                .round()
                .to_i64()
                .map(SessionFrame::new)
                .map(Self::Host),
        }
    }
}

impl TryFrom<MapPosition> for f64 {
    type Error = MapCoordinateError;

    fn try_from(position: MapPosition) -> Result<Self, Self::Error> {
        match position {
            MapPosition::Asset(frame) => Ok(Self::from(frame)),
            MapPosition::Host(frame) => {
                let integer = i64::from(frame);
                let scalar = integer
                    .to_f64()
                    .ok_or(MapCoordinateError::InexactHostFrame)?;
                if scalar.to_i64() == Some(integer) {
                    Ok(scalar)
                } else {
                    Err(MapCoordinateError::InexactHostFrame)
                }
            }
        }
    }
}

impl PartialOrd for MapPosition {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (*self, *other) {
            (Self::Asset(left), Self::Asset(right)) => left.partial_cmp(&right),
            (Self::Host(left), Self::Host(right)) => left.partial_cmp(&right),
            _ => None,
        }
    }
}

impl From<AssetFrame> for MapPosition {
    fn from(value: AssetFrame) -> Self {
        Self::Asset(value)
    }
}

impl From<SessionFrame> for MapPosition {
    fn from(value: SessionFrame) -> Self {
        Self::Host(value)
    }
}

/// A coordinate value tied to one exact map identity and revision.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct MapPoint<T> {
    stamp: MapStamp,
    value: T,
}

impl<T> MapPoint<T> {
    /// Stamps `value` for use with one immutable map snapshot.
    #[must_use]
    pub const fn new(stamp: MapStamp, value: T) -> Self {
        Self { stamp, value }
    }

    /// Returns the map identity and revision carried by this point.
    #[must_use]
    pub const fn stamp(&self) -> MapStamp {
        self.stamp
    }

    /// Returns the stamped value.
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AxisKind {
    Asset,
    Host,
}
