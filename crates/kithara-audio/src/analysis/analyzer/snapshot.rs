use num_traits::cast::ToPrimitive;

use crate::{coverage::FrameRange, waveform::BeatGrid};

/// Whether a grid can still change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GridState {
    Provisional,
    Final,
}

/// The beat artifact of one snapshot.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BeatSnapshot {
    grid: BeatGrid,
    state: GridState,
    unanalysed: Vec<FrameRange>,
    confidence: Option<f32>,
}

impl BeatSnapshot {
    #[must_use]
    pub fn new(grid: BeatGrid, state: GridState, unanalysed: Vec<FrameRange>) -> Self {
        Self {
            confidence: grid_confidence(&grid),
            grid,
            state,
            unanalysed,
        }
    }

    /// Mean confidence over the markers the detector actually reported.
    /// `None` when it reported none, since zero is a different answer.
    /// Independent of [`state`](Self::state).
    #[must_use]
    pub const fn confidence(&self) -> Option<f32> {
        self.confidence
    }

    #[must_use]
    pub const fn grid(&self) -> &BeatGrid {
        &self.grid
    }

    #[must_use]
    pub const fn state(&self) -> GridState {
        self.state
    }

    /// Source ranges the pass could not analyse, so the grid claims nothing
    /// about them.
    #[must_use]
    pub fn unanalysed(&self) -> &[FrameRange] {
        &self.unanalysed
    }
}

fn grid_confidence(grid: &BeatGrid) -> Option<f32> {
    let mut sum = 0.0_f64;
    let mut count = 0_u32;
    for confidence in grid
        .beat_confidence()
        .iter()
        .chain(grid.downbeat_confidence().iter())
        .flatten()
    {
        sum += f64::from(*confidence);
        count = count.saturating_add(1);
    }
    if count == 0 {
        return None;
    }
    (sum / f64::from(count)).to_f32()
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::{BeatSnapshot, GridState};
    use crate::waveform::BeatGrid;

    fn snapshot(beats: Vec<(u64, Option<f32>)>, state: GridState) -> BeatSnapshot {
        BeatSnapshot::new(
            BeatGrid::new(120.0, beats, Vec::new(), Vec::new()),
            state,
            Vec::new(),
        )
    }

    #[kithara::test(native, flash(false))]
    fn a_grid_reports_the_mean_of_what_was_detected() {
        let snapshot = snapshot(
            vec![(0, Some(0.4)), (100, Some(0.8)), (200, None)],
            GridState::Provisional,
        );

        let confidence = snapshot.confidence().expect("detected markers average");
        assert!(
            (confidence - 0.6).abs() < 1e-6,
            "the extrapolated marker is not averaged in: {confidence}"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_grid_with_nothing_detected_reports_nothing() {
        assert_eq!(
            snapshot(vec![(0, None), (100, None)], GridState::Provisional).confidence(),
            None,
            "a grid built entirely by extrapolation claims nothing"
        );
        assert_eq!(
            snapshot(Vec::new(), GridState::Final).confidence(),
            None,
            "an empty grid claims nothing"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_final_grid_of_weak_markers_is_less_sure_than_a_provisional_strong_one() {
        let weak = snapshot(vec![(0, Some(0.2)), (100, Some(0.3))], GridState::Final);
        let strong = snapshot(
            vec![(0, Some(0.9)), (100, Some(0.95))],
            GridState::Provisional,
        );

        assert_eq!(weak.state(), GridState::Final);
        assert_eq!(strong.state(), GridState::Provisional);
        assert!(
            weak.confidence() < strong.confidence(),
            "confidence follows the markers, not the state"
        );
    }
}
