use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};

/// Whether the producer may still revise the grid it published.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BeatGridState {
    Provisional,
    Final,
}

/// One beat of the grid: where it falls, which beat it is, and how sure
/// whoever placed it was.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct GridBeat {
    /// Media seconds from the start of the track.
    pub at: f64,
    /// The beat's own number, stable across revisions and across the gaps a
    /// sparse grid leaves. Never a position in [`RawBeatGrid::beats`].
    pub ordinal: i64,
    /// Absent where nothing observed the beat and it was placed by fitting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// One bar line, named by the beat it falls on.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct GridDownbeat {
    /// Media seconds, equal to the time the named beat carries.
    pub at: f64,
    pub beat_ordinal: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

/// The bar the grid claims, stated only where its phase is provable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Meter {
    pub beats_per_bar: NonZeroU16,
    pub origin_beat_ordinal: i64,
}

/// A beat grid as it arrives over the wire: a document nobody has checked yet.
///
/// Reading a [`BeatGridModel`](super::BeatGridModel) goes through this, so no
/// deserialization reaches a validated grid without the checks.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RawBeatGrid {
    pub schema_version: u32,
    pub model_id: String,
    pub revision: u64,
    pub state: BeatGridState,
    /// Media seconds the track is known to run for; absent where the length is
    /// not known, which is a different answer from zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    pub bpm: f64,
    #[serde(default)]
    pub beats: Vec<GridBeat>,
    #[serde(default)]
    pub downbeats: Vec<GridDownbeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<Meter>,
}
