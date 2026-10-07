use std::num::{NonZeroU16, NonZeroU32};

use kithara_host::api::Tempo;
use kithara_signal::SessionFrame;
use kithara_warp::{SessionAnchor, SessionBeat};

use crate::LinkError;

/// Host tempo over session time, with continuous beats at every tempo step.
#[derive(Clone, Debug)]
pub struct TempoTrajectory {
    steps: Vec<TempoStep>,
    beats_per_bar: NonZeroU16,
    sample_rate: NonZeroU32,
}

/// One tempo change and the beat reached when it takes effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoStep {
    pub frame: SessionFrame,
    pub beat: SessionBeat,
    pub tempo: Tempo,
}

impl TempoTrajectory {
    /// Starts the trajectory at an anchor, extrapolating its tempo backwards.
    #[must_use]
    pub fn new(start: TempoStep, beats_per_bar: NonZeroU16, sample_rate: NonZeroU32) -> Self {
        Self {
            steps: vec![start],
            beats_per_bar,
            sample_rate,
        }
    }

    /// Inserts a tempo step and reanchors later steps without changing their frames.
    ///
    /// # Errors
    /// Returns a refusal for an occupied frame or a frame at or before the anchor.
    pub fn push(&mut self, frame: SessionFrame, tempo: Tempo) -> Result<(), LinkError> {
        if frame <= self.steps[0].frame {
            return Err(LinkError::BeforeAnchor { frame });
        }
        let index = self.steps.partition_point(|step| step.frame < frame);
        if self
            .steps
            .get(index)
            .is_some_and(|step| step.frame == frame)
        {
            return Err(LinkError::Occupied { frame });
        }
        let beat = self.beat_at(frame);
        self.steps.insert(index, TempoStep { frame, beat, tempo });
        self.reanchor(index + 1);
        Ok(())
    }

    /// Removes a refused step, retaining the initial anchor and continuous beats.
    pub fn withdraw(&mut self, frame: SessionFrame) {
        if let Some(index) = self.steps.iter().position(|step| step.frame == frame)
            && index != 0
        {
            self.steps.remove(index);
            self.reanchor(index);
        }
    }

    /// The continuous beat at a session frame.
    #[must_use]
    pub fn beat_at(&self, frame: SessionFrame) -> SessionBeat {
        let index = self
            .steps
            .partition_point(|step| step.frame <= frame)
            .saturating_sub(1);
        match self.anchor(self.steps[index]).beat_at(frame) {
            Ok(beat) => beat,
            Err(error) => panic!("session beat is not representable: {error}"),
        }
    }

    /// The session frame at a beat, rounded to the nearest output frame.
    ///
    /// # Panics
    /// Panics if the requested beat lies outside the representable session axis.
    #[must_use]
    pub fn frame_at(&self, beat: SessionBeat) -> SessionFrame {
        match self.try_frame_at(beat) {
            Some(frame) => frame,
            None => panic!("session frame is not representable"),
        }
    }

    pub(crate) fn try_frame_at(&self, beat: SessionBeat) -> Option<SessionFrame> {
        let index = self
            .steps
            .partition_point(|step| step.beat <= beat)
            .saturating_sub(1);
        self.anchor(self.steps[index]).frame_at(beat).ok()
    }

    /// The piecewise-constant tempo active on a frame.
    #[must_use]
    pub fn tempo_at(&self, frame: SessionFrame) -> Tempo {
        let index = self
            .steps
            .partition_point(|step| step.frame <= frame)
            .saturating_sub(1);
        self.steps[index].tempo
    }

    /// Bar lines on or after `from`, while their frames are representable.
    pub fn downbeats(&self, from: SessionFrame) -> impl Iterator<Item = SessionFrame> + '_ {
        self.markers(from, f64::from(self.beats_per_bar.get()))
    }

    /// Beat lines on or after `from`, while their frames are representable.
    pub fn beats(&self, from: SessionFrame) -> impl Iterator<Item = SessionFrame> + '_ {
        self.markers(from, 1.0)
    }

    /// Reanchors a restarted route while continuing the beat count at its stop.
    pub fn reaxis(&mut self, stopped: SessionFrame, start: SessionFrame) {
        let _ = (stopped, start);
        todo!("Reanchor the new route axis and reconcile pending tempo steps (spec §4.8)")
    }

    pub(crate) fn beats_per_bar(&self) -> f64 {
        f64::from(self.beats_per_bar.get())
    }

    fn anchor(&self, step: TempoStep) -> SessionAnchor {
        match SessionAnchor::new(
            step.frame,
            step.beat,
            step.tempo.beats_per_second(),
            self.sample_rate,
        ) {
            Ok(anchor) => anchor,
            Err(error) => unreachable!("validated tempo and sample rate define an anchor: {error}"),
        }
    }

    fn reanchor(&mut self, from: usize) {
        for index in from..self.steps.len() {
            self.steps[index].beat = match self
                .anchor(self.steps[index - 1])
                .beat_at(self.steps[index].frame)
            {
                Ok(beat) => beat,
                Err(error) => panic!("session beat is not representable: {error}"),
            };
        }
    }

    fn markers(&self, from: SessionFrame, stride: f64) -> impl Iterator<Item = SessionFrame> + '_ {
        let first = (f64::from(self.beat_at(from)) / stride).floor() * stride;
        std::iter::successors(Some(first), move |beat| {
            let next = beat + stride;
            (next.is_finite() && next > *beat).then_some(next)
        })
        .map_while(|value| {
            let beat = SessionBeat::new(value).ok()?;
            self.try_frame_at(beat)
        })
        .filter(move |frame| *frame >= from)
    }
}

#[cfg(test)]
mod tests {
    use kithara_test_utils::kithara;

    use super::*;

    #[kithara::test]
    fn tempo_steps_round_trip_and_withdraw_reanchors_their_successors() {
        let mut host = TempoTrajectory::new(
            TempoStep {
                frame: SessionFrame::new(0),
                beat: SessionBeat::default(),
                tempo: Tempo::DEFAULT,
            },
            NonZeroU16::new(4).expect("nonzero meter"),
            NonZeroU32::new(48_000).expect("nonzero rate"),
        );
        host.push(
            SessionFrame::new(96_000),
            Tempo::new(60.0).expect("valid tempo"),
        )
        .expect("new frame");
        host.push(
            SessionFrame::new(192_000),
            Tempo::new(180.0).expect("valid tempo"),
        )
        .expect("new frame");
        for frame in [-48_000, 0, 24_000, 96_000, 120_000, 192_000, 240_000] {
            let frame = SessionFrame::new(frame);
            assert_eq!(host.frame_at(host.beat_at(frame)), frame);
        }
        assert_eq!(f64::from(host.beat_at(SessionFrame::new(192_000))), 6.0);
        assert_eq!(
            host.downbeats(SessionFrame::new(1))
                .take(2)
                .collect::<Vec<_>>(),
            vec![SessionFrame::new(96_000), SessionFrame::new(224_000)]
        );
        assert_eq!(
            host.beats(SessionFrame::new(1)).next(),
            Some(SessionFrame::new(24_000))
        );
        assert!(
            host.push(SessionFrame::new(96_000), Tempo::DEFAULT)
                .is_err()
        );
        host.withdraw(SessionFrame::new(96_000));
        assert_eq!(f64::from(host.beat_at(SessionFrame::new(192_000))), 8.0);
        host.withdraw(SessionFrame::new(0));
        assert_eq!(host.tempo_at(SessionFrame::new(0)), Tempo::DEFAULT);
    }
}
