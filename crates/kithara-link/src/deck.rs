use kithara_command::Seq;
use kithara_play::{HostedDeck, Outbox, PlayError};
use kithara_signal::{FrameCount, SessionFrame};

use crate::{GridAnswer, LinkedPlayer, TempoTrajectory};

/// Object-safe synchronization face of a deck held by the Host owner.
pub trait LinkedDeck<S>: HostedDeck<S> {
    /// Enables explicit phase alignment or disables future Host retimes.
    ///
    /// # Errors
    /// Returns the refusal of an alignment batch.
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError>;
    /// Delivers the planned Host trajectory to every active synchronized track.
    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>);
    /// Delivers analysis to the track owning the exact item and load.
    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>);
    /// Whether this deck follows the Host's tempo.
    fn synced(&self) -> bool;
    /// Maximum lane lead among sounding synchronized tracks.
    fn lead(&self) -> Option<FrameCount>;
    /// Minimum available room among lanes that would receive a retime.
    fn lane_room(&self) -> usize;
}

impl<S, D: HostedDeck<S> + LinkedPlayer<S>> LinkedDeck<S> for D {
    fn sync(&mut self, on: bool, out: &mut Outbox<'_, S>) -> Result<Option<Seq>, PlayError> {
        LinkedPlayer::sync(self, on, out)
    }

    fn retime(&mut self, trajectory: &TempoTrajectory, at: SessionFrame, out: &mut Outbox<'_, S>) {
        LinkedPlayer::retime(self, trajectory, at, out);
    }

    fn grid(&mut self, answer: GridAnswer, out: &mut Outbox<'_, S>) {
        LinkedPlayer::grid(self, answer, out);
    }

    fn synced(&self) -> bool {
        LinkedPlayer::synced(self)
    }

    fn lead(&self) -> Option<FrameCount> {
        LinkedPlayer::lead(self)
    }

    fn lane_room(&self) -> usize {
        LinkedPlayer::lane_room(self)
    }
}
