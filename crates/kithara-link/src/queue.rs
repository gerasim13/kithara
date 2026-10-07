use kithara_command::Seq;
use kithara_play::{Outbox, PlayError, TrackFactory};
use kithara_queue::Queue;
use kithara_signal::{FrameCount, SessionFrame};

use crate::{GridAnswer, LinkedFactory, LinkedPlayer, TempoTrajectory};

impl<S, F: TrackFactory<S>> LinkedPlayer<S> for Queue<S, LinkedFactory<F>> {
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        self.factory_mut().set_synced(on);
        let mut sent = None;
        for track in self.tracks_mut() {
            let seq = track.sync(on, out)?;
            if seq.is_some() {
                sent = seq;
            }
        }
        Ok(sent)
    }

    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        self.factory_mut().set_trajectory(trajectory);
        for track in self.tracks_mut() {
            track.retime(trajectory, at, out);
        }
    }

    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>) {
        for track in self.tracks_mut() {
            track.grid(answer.clone(), out);
        }
    }

    fn synced(&self) -> bool {
        match self.current_track() {
            Some(track) => track.synced(),
            None => todo!(
                "Read the empty deck's mode from LinkedFactory through the queue owner seam (spec §4.6)"
            ),
        }
    }

    fn lead(&self) -> Option<FrameCount> {
        todo!(
            "Maximum lead over every sounding SYNC track, including both sides of a crossfade; the contract provides only tracks_mut and current_track (spec §3.4/§4.6)"
        )
    }

    fn lane_room(&self) -> usize {
        todo!(
            "Minimum Sender::available over every lane a retime sends to; immutable active-track access is needed here (spec §4.6 step 2)"
        )
    }
}
