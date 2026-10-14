use kithara_beat::BeatGridModel;
use kithara_play::{Outbox, PlayError, Position, Track, TrackSnapshot, TrackStatus};
use kithara_signal::{AudioSpec, SessionFrame};
use kithara_sync::{SourceGrid, speed_curve};
use kithara_warp::{AssetAxis, AssetExtent};

use super::{Linked, LinkedPlayer};

pub(super) fn cue_position(track: &TrackSnapshot) -> Position {
    if matches!(track.status, TrackStatus::Ended { .. }) {
        Position::ZERO
    } else {
        track.position
    }
}

impl<P> Linked<P> {
    pub(super) fn earliest<S>(&self, out: &Outbox<'_, S>) -> Result<SessionFrame, PlayError>
    where
        P: Track<S>,
    {
        let pass = out.pass().ok_or(PlayError::Untimed)?;
        Ok(pass.now + self.lead(pass.delivery).unwrap_or(pass.delivery))
    }

    pub(super) fn planned<S>(&self, at: SessionFrame) -> Result<(Position, f64), PlayError>
    where
        P: Track<S>,
    {
        self.inner.planned(at, self.host.sample_rate())
    }

    pub(super) fn jump_frame<S>(&self, out: &Outbox<'_, S>) -> Result<SessionFrame, PlayError>
    where
        P: Track<S>,
    {
        Ok(self.earliest(out)? + self.inner.snapshot().as_ref().declick)
    }

    pub(super) fn source<S>(&self, grid: &BeatGridModel) -> Result<SourceGrid, PlayError>
    where
        P: Track<S>,
    {
        let rate = self.host.sample_rate();
        let extent = match self.inner.snapshot().as_ref().duration {
            Some(duration) => AssetExtent::Bounded(
                u64::try_from(
                    AudioSpec::new(1, rate)
                        .frames_for(duration)
                        .map_err(|_| PlayError::Untimed)?
                        .get(),
                )
                .map_err(|_| PlayError::Untimed)?,
            ),
            None => AssetExtent::Unknown,
        };
        SourceGrid::new(grid, AssetAxis::new(rate, extent)).ok_or(PlayError::NotReady)
    }

    pub(super) fn curve(
        &self,
        grid: &SourceGrid,
        position: Position,
        at: SessionFrame,
        landing: Option<(Position, SessionFrame)>,
    ) -> Result<kithara_warp::SpeedCurve, PlayError> {
        speed_curve(&self.host, grid, position, at, landing).ok_or_else(|| {
            PlayError::Internal("Host speed law is not representable on the track axis".into())
        })
    }
}
