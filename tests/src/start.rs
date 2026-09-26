//! Where a test opens a track. It reads only a [`BeatGridModel`], so a suite
//! whose build carries no analysis, such as the Android device suite, still
//! names its starts with it.

use kithara::beat::{BeatGridModel, GridBeat};

/// Where a test starts a track. A track with an analysed grid opens where the
/// music does, on a beat the grid names; a track without one opens at a
/// second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Start {
    /// Beat `beat` of bar `bar`, bars counted from the grid's first downbeat:
    /// beat 0 is the bar's downbeat, a later one enters on a weak beat.
    Bar { bar: usize, beat: i64 },
    /// Analysed beat `ordinal`, for a grid that states no bars.
    Beat(i64),
    /// A second of a track that has no analysed grid.
    Seconds(f64),
}

impl Start {
    /// The downbeat of bar `bar`.
    #[must_use]
    pub const fn bar(bar: usize) -> Self {
        Self::Bar { bar, beat: 0 }
    }

    /// The second this start opens the track at; `grid` is the track's
    /// analysed grid, if it has one.
    ///
    /// # Panics
    ///
    /// When a start on the grid meets a track without one, or [`Self::beat`]
    /// panics.
    #[must_use]
    pub fn seconds(self, grid: Option<&BeatGridModel>) -> f64 {
        match self {
            Self::Seconds(seconds) => seconds,
            on_grid => {
                let grid = grid.unwrap_or_else(|| panic!("{on_grid:?} needs a track with a grid"));
                on_grid.beat(grid).at
            }
        }
    }

    /// The analysed beat this start names in `grid`.
    ///
    /// # Panics
    ///
    /// When `grid` names no such beat: a bar past its downbeats, bars on a
    /// grid that states none, or a start at a second.
    #[must_use]
    pub fn beat(self, grid: &BeatGridModel) -> GridBeat {
        let raw = grid.as_raw();
        let ordinal = match self {
            Self::Bar { bar, beat } => {
                let downbeat = raw.downbeats.get(bar).unwrap_or_else(|| {
                    panic!(
                        "bar {bar} is past the {} downbeats the grid names",
                        raw.downbeats.len()
                    )
                });
                downbeat.beat_ordinal + beat
            }
            Self::Beat(ordinal) => ordinal,
            Self::Seconds(seconds) => panic!("a start at {seconds} s names no beat"),
        };
        *raw.beats
            .iter()
            .find(|beat| beat.ordinal == ordinal)
            .unwrap_or_else(|| panic!("{self:?} names beat {ordinal}, which the grid does not"))
    }
}

#[cfg(test)]
mod tests {
    use ::kithara::beat::{BeatGridState, GridDownbeat, RawBeatGrid, SCHEMA_VERSION};
    use kithara_test_utils::kithara;
    use num_traits::cast::AsPrimitive;

    use super::*;

    #[kithara::test]
    fn a_start_names_its_beat_from_the_bar_downbeats() {
        let beat = |ordinal: i64| GridBeat {
            at: f64_of(ordinal) * 0.5,
            ordinal,
            confidence: None,
        };
        let downbeat = |ordinal: i64| GridDownbeat {
            at: f64_of(ordinal) * 0.5,
            beat_ordinal: ordinal,
            confidence: None,
        };
        let grid = BeatGridModel::try_from(RawBeatGrid {
            schema_version: SCHEMA_VERSION,
            model_id: "bars".to_owned(),
            revision: 1,
            state: BeatGridState::Final,
            duration: None,
            bpm: 120.0,
            beats: (0..12).map(beat).collect(),
            downbeats: [2, 6, 10].into_iter().map(downbeat).collect(),
            meter: None,
        })
        .expect("downbeats on beats form a grid");

        assert_eq!(Start::bar(0).beat(&grid).ordinal, 2);
        assert_eq!(Start::bar(2).beat(&grid).at, 5.0);
        assert_eq!(Start::Bar { bar: 1, beat: 1 }.beat(&grid).ordinal, 7);
        assert_eq!(Start::Beat(3).beat(&grid).ordinal, 3);
        assert_eq!(Start::bar(1).seconds(Some(&grid)), 3.0);
        assert_eq!(Start::Seconds(1.25).seconds(None), 1.25);
    }

    fn f64_of(ordinal: i64) -> f64 {
        ordinal.as_()
    }
}
