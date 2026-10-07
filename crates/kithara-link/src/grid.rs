use kithara_beat::{BeatGridModel, Meter};
use kithara_command::Seq;
use kithara_events::TrackId;
use kithara_play::Position;

/// A validated analysis grid, kept in media seconds rather than session frames.
#[derive(Clone, Debug)]
pub struct TrackGrid {
    model: BeatGridModel,
}

impl From<BeatGridModel> for TrackGrid {
    fn from(model: BeatGridModel) -> Self {
        Self { model }
    }
}

impl TrackGrid {
    /// The analyzed track tempo in beats per minute.
    #[must_use]
    pub fn bpm(&self) -> f64 {
        self.model.as_raw().bpm
    }

    /// The proven track meter, absent for beat-only phase alignment.
    #[must_use]
    pub fn meter(&self) -> Option<Meter> {
        self.model.as_raw().meter
    }

    /// The last bar line at or before a position, or the last beat without meter.
    #[must_use]
    pub fn downbeat_at_or_before(&self, position: Position) -> Option<Position> {
        self.boundaries()
            .take_while(|seconds| *seconds <= position.as_secs_f64())
            .last()
            .and_then(|seconds| Position::try_from_secs_f64(seconds).ok())
    }

    /// The first bar line at or after a position, or the first beat without meter.
    #[must_use]
    pub fn first_downbeat_at_or_after(&self, position: Position) -> Option<Position> {
        self.boundaries()
            .find(|seconds| *seconds >= position.as_secs_f64())
            .and_then(|seconds| Position::try_from_secs_f64(seconds).ok())
    }

    /// Whether the observed beat range brackets a position and proves its phase.
    #[must_use]
    pub fn covers(&self, position: Position) -> bool {
        let raw = self.model.as_raw();
        match (raw.beats.first(), raw.beats.last()) {
            (Some(first), Some(last)) => {
                (first.at..=last.at).contains(&position.as_secs_f64())
                    && self.downbeat_at_or_before(position).is_some()
            }
            _ => false,
        }
    }

    fn boundaries(&self) -> impl Iterator<Item = f64> + '_ {
        let raw = self.model.as_raw();
        raw.downbeats
            .iter()
            .filter(|_| raw.meter.is_some())
            .map(|beat| beat.at)
            .chain(
                raw.beats
                    .iter()
                    .filter(|_| raw.meter.is_none())
                    .map(|beat| beat.at),
            )
    }
}

/// One analysis answer, scoped to the item and exact load that requested it.
#[derive(Clone, Debug)]
pub struct GridAnswer {
    pub item: TrackId,
    pub load: Seq,
    pub model: Result<BeatGridModel, GridRefusal>,
}

/// Analysis cannot supply a usable grid for this load.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
#[error("beat-grid analysis refused: {reason}")]
pub struct GridRefusal {
    pub reason: String,
}
