use num_traits::cast::ToPrimitive;

use crate::{BeatArtifact, coverage::FrameRange};

/// Whether a beat artifact can still change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BeatState {
    Provisional,
    Final,
}

/// The beat artifact of one snapshot.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct BeatSnapshot {
    artifact: BeatArtifact,
    state: BeatState,
    confidence: Option<f32>,
    unanalysed: Vec<FrameRange>,
}

impl BeatSnapshot {
    #[must_use]
    pub fn new(artifact: BeatArtifact, state: BeatState, unanalysed: Vec<FrameRange>) -> Self {
        Self {
            confidence: artifact_confidence(&artifact),
            artifact,
            state,
            unanalysed,
        }
    }

    #[must_use]
    pub const fn artifact(&self) -> &BeatArtifact {
        &self.artifact
    }

    /// Mean confidence over the markers the detector actually reported.
    /// `None` when it reported none, since zero is a different answer.
    /// Independent of [`state`](Self::state).
    #[must_use]
    pub const fn confidence(&self) -> Option<f32> {
        self.confidence
    }

    #[must_use]
    pub const fn state(&self) -> BeatState {
        self.state
    }

    /// Source ranges the pass could not analyse, so the artifact claims nothing
    /// about them.
    #[must_use]
    pub fn unanalysed(&self) -> &[FrameRange] {
        &self.unanalysed
    }
}

fn artifact_confidence(artifact: &BeatArtifact) -> Option<f32> {
    let mut sum = 0.0_f64;
    let mut count = 0_u32;
    for confidence in artifact
        .beat_confidence()
        .iter()
        .chain(artifact.downbeat_confidence().iter())
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

    use super::{BeatSnapshot, BeatState};
    use crate::BeatArtifact;

    fn snapshot(beats: Vec<(u64, Option<f32>)>, state: BeatState) -> BeatSnapshot {
        BeatSnapshot::new(
            BeatArtifact::new(120.0, beats, Vec::new()),
            state,
            Vec::new(),
        )
    }

    #[kithara::test(native, flash(false))]
    fn an_artifact_reports_the_mean_of_what_was_detected() {
        let snapshot = snapshot(
            vec![(0, Some(0.4)), (100, Some(0.8)), (200, None)],
            BeatState::Provisional,
        );

        let confidence = snapshot.confidence().expect("detected markers average");
        assert!(
            (confidence - 0.6).abs() < 1e-6,
            "the extrapolated marker is not averaged in: {confidence}"
        );
    }

    #[kithara::test(native, flash(false))]
    fn an_artifact_with_nothing_detected_reports_nothing() {
        assert_eq!(
            snapshot(vec![(0, None), (100, None)], BeatState::Provisional).confidence(),
            None,
            "an artifact built entirely by extrapolation claims nothing"
        );
        assert_eq!(
            snapshot(Vec::new(), BeatState::Final).confidence(),
            None,
            "an empty artifact claims nothing"
        );
    }

    #[kithara::test(native, flash(false))]
    fn a_final_grid_of_weak_markers_is_less_sure_than_a_provisional_strong_one() {
        let weak = snapshot(vec![(0, Some(0.2)), (100, Some(0.3))], BeatState::Final);
        let strong = snapshot(
            vec![(0, Some(0.9)), (100, Some(0.95))],
            BeatState::Provisional,
        );

        assert_eq!(weak.state(), BeatState::Final);
        assert_eq!(strong.state(), BeatState::Provisional);
        assert!(
            weak.confidence() < strong.confidence(),
            "confidence follows the markers, not the state"
        );
    }
}
