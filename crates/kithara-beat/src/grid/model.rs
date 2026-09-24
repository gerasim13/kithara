use serde::{Deserialize, Serialize};

use super::{
    error::BeatGridError,
    raw::{GridBeat, GridDownbeat, RawBeatGrid},
};

/// The wire schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// A beat grid whose times, ordinals, anchors and meter have been checked.
///
/// The only way in is [`RawBeatGrid`], so a value of this type states a grid
/// that holds together rather than one that merely parsed.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(into = "RawBeatGrid", try_from = "RawBeatGrid")]
pub struct BeatGridModel(RawBeatGrid);

impl BeatGridModel {
    /// The checked document itself. A validated grid adds no second way to
    /// read a grid: it states that this document holds together.
    #[must_use]
    pub const fn as_raw(&self) -> &RawBeatGrid {
        &self.0
    }
}

impl From<BeatGridModel> for RawBeatGrid {
    fn from(model: BeatGridModel) -> Self {
        model.0
    }
}

impl TryFrom<RawBeatGrid> for BeatGridModel {
    type Error = BeatGridError;

    fn try_from(raw: RawBeatGrid) -> Result<Self, Self::Error> {
        check_header(&raw)?;
        check_beats(&raw)?;
        check_downbeats(&raw)?;
        check_meter(&raw)?;
        Ok(Self(raw))
    }
}

fn check_header(raw: &RawBeatGrid) -> Result<(), BeatGridError> {
    if raw.schema_version != SCHEMA_VERSION {
        return Err(BeatGridError::Schema {
            expected: SCHEMA_VERSION,
            found: raw.schema_version,
        });
    }
    if raw.model_id.is_empty() {
        return Err(BeatGridError::ModelId);
    }
    if !raw.bpm.is_finite() || raw.bpm <= 0.0 {
        return Err(BeatGridError::Bpm { bpm: raw.bpm });
    }
    match raw.duration {
        Some(duration) if !duration.is_finite() || duration < 0.0 => {
            Err(BeatGridError::Time { seconds: duration })
        }
        _ => Ok(()),
    }
}

fn check_beats(raw: &RawBeatGrid) -> Result<(), BeatGridError> {
    let mut previous: Option<GridBeat> = None;
    for beat in &raw.beats {
        check_marker(beat.at, beat.ordinal, beat.confidence, raw.duration)?;
        if previous.is_some_and(|before| beat.at <= before.at || beat.ordinal <= before.ordinal) {
            return Err(BeatGridError::Order {
                ordinal: beat.ordinal,
                seconds: beat.at,
            });
        }
        previous = Some(*beat);
    }
    Ok(())
}

fn check_downbeats(raw: &RawBeatGrid) -> Result<(), BeatGridError> {
    let mut previous: Option<GridDownbeat> = None;
    for downbeat in &raw.downbeats {
        check_marker(
            downbeat.at,
            downbeat.beat_ordinal,
            downbeat.confidence,
            raw.duration,
        )?;
        if previous.is_some_and(|before| {
            downbeat.at <= before.at || downbeat.beat_ordinal <= before.beat_ordinal
        }) {
            return Err(BeatGridError::Order {
                ordinal: downbeat.beat_ordinal,
                seconds: downbeat.at,
            });
        }
        let anchored = raw
            .beats
            .binary_search_by_key(&downbeat.beat_ordinal, |beat| beat.ordinal)
            .is_ok_and(|index| raw.beats[index].at == downbeat.at);
        if !anchored {
            return Err(BeatGridError::Anchor {
                ordinal: downbeat.beat_ordinal,
            });
        }
        previous = Some(*downbeat);
    }
    Ok(())
}

fn check_meter(raw: &RawBeatGrid) -> Result<(), BeatGridError> {
    let Some(meter) = raw.meter else {
        return Ok(());
    };
    let bar = i64::from(meter.beats_per_bar.get());
    raw.downbeats
        .iter()
        .find(|downbeat| {
            downbeat
                .beat_ordinal
                .saturating_sub(meter.origin_beat_ordinal)
                .rem_euclid(bar)
                != 0
        })
        .map_or(Ok(()), |downbeat| {
            Err(BeatGridError::Meter {
                beats_per_bar: meter.beats_per_bar.get(),
                ordinal: downbeat.beat_ordinal,
            })
        })
}

fn check_marker(
    at: f64,
    ordinal: i64,
    confidence: Option<f32>,
    duration: Option<f64>,
) -> Result<(), BeatGridError> {
    if !at.is_finite() || at < 0.0 {
        return Err(BeatGridError::Time { seconds: at });
    }
    if let Some(confidence) = confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(BeatGridError::Confidence { confidence });
    }
    match duration {
        Some(duration) if at > duration => Err(BeatGridError::PastDuration {
            ordinal,
            duration,
            seconds: at,
        }),
        _ => Ok(()),
    }
}
